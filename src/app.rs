use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use ratatui::widgets::ListState;

use crate::db::{Db, DbError};
use crate::library::{self, Library};
use crate::player::{self, PlayerCommand, PlayerHandle, PlayerStatus, TrackRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Artists,
    Songs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Command,
    Search,
}

const VOLUME_STEP: f32 = 0.05;

pub struct StatusMessage {
    pub text: String,
    pub is_error: bool,
}

pub struct App {
    pub library: Library,
    pub focus: Focus,
    pub mode: Mode,
    pub selected_artist: usize,
    pub selected_song: usize,
    pub command_input: String,
    pub search_input: String,
    /// Indices (within whichever pane's list `search_focus` names) of items matching the most
    /// recent search, in list order.
    search_matches: Vec<usize>,
    search_match_pos: usize,
    search_focus: Focus,
    /// Selection to restore if the search is cancelled with `Esc`.
    search_origin: (usize, usize),
    pub status_message: Option<StatusMessage>,
    pub quit_prompted: bool,
    pub should_quit: bool,
    pub player: PlayerHandle,
    pub player_status: Arc<Mutex<PlayerStatus>>,
    /// Remaining songs (artist_idx, song_idx) queued to auto-play after the current track, all
    /// drawn from the same album as whatever was last explicitly played.
    queue: VecDeque<(usize, usize)>,
    db: Db,
    /// Scroll/selection state for the two list widgets, persisted across frames (rather than
    /// rebuilt fresh each render) so ratatui scrolls incrementally instead of re-snapping the
    /// selected row to the edge of the viewport every frame.
    pub artist_list_state: ListState,
    pub song_list_state: ListState,
}

impl App {
    pub fn new(player: PlayerHandle, player_status: Arc<Mutex<PlayerStatus>>, db: Db) -> Self {
        let mut app = Self {
            library: Library::default(),
            focus: Focus::Artists,
            mode: Mode::Normal,
            selected_artist: 0,
            selected_song: 0,
            command_input: String::new(),
            search_input: String::new(),
            search_matches: Vec::new(),
            search_match_pos: 0,
            search_focus: Focus::Artists,
            search_origin: (0, 0),
            status_message: None,
            quit_prompted: false,
            should_quit: false,
            player,
            player_status,
            queue: VecDeque::new(),
            db,
            artist_list_state: ListState::default(),
            song_list_state: ListState::default(),
        };

        if let Err(e) = app.reload_library() {
            app.set_error(e.to_string());
        }

        app
    }

    /// Reloads `self.library` from the database. This is the single function used to populate
    /// the in-memory library, whether at startup or after `add`/`clear` mutate the database, so
    /// the database is always the one source of truth for library contents.
    fn reload_library(&mut self) -> Result<(), DbError> {
        self.library = self.db.load_library()?;
        // Indices captured by a previous search may no longer point at the same items.
        self.search_matches.clear();
        Ok(())
    }

    /// Pulls any pending error out of the shared player status and surfaces it in the
    /// command pane, so playback failures on the worker thread reach the user. Also picks up
    /// natural end-of-track so the queue can advance.
    pub fn sync_player_status(&mut self) {
        let (err, finished) = {
            let mut status = player::lock_status(&self.player_status);
            (status.error.take(), std::mem::take(&mut status.just_finished))
        };

        if let Some(text) = err {
            self.status_message = Some(StatusMessage {
                text,
                is_error: true,
            });
        }

        if finished {
            self.play_next_in_queue();
        }
    }

    pub fn handle_key(&mut self, code: ratatui::crossterm::event::KeyCode) {
        use ratatui::crossterm::event::KeyCode;

        if self.mode == Mode::Command {
            match code {
                KeyCode::Enter => self.execute_command(),
                KeyCode::Esc => {
                    self.command_input.clear();
                    self.mode = Mode::Normal;
                }
                KeyCode::Backspace => {
                    self.command_input.pop();
                }
                KeyCode::Char(c) => self.command_input.push(c),
                _ => {}
            }
            return;
        }

        if self.mode == Mode::Search {
            match code {
                KeyCode::Enter => {
                    self.mode = Mode::Normal;
                    self.finish_search();
                }
                KeyCode::Esc => self.cancel_search(),
                KeyCode::Backspace => {
                    self.search_input.pop();
                    self.update_search();
                }
                KeyCode::Char(c) => {
                    self.search_input.push(c);
                    self.update_search();
                }
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Char('j') => self.move_selection(1),
            KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => self.move_to_start(),
            KeyCode::Char('G') | KeyCode::End => self.move_to_end(),
            KeyCode::Char('h') => self.focus = Focus::Artists,
            KeyCode::Char('l') => self.focus = Focus::Songs,
            KeyCode::Char('z') => self.play_relative(-1),
            KeyCode::Char('x') => self.player.send(PlayerCommand::Restart),
            KeyCode::Char('c') => self.toggle_play_pause(),
            KeyCode::Char('v') => self.play_relative(1),
            KeyCode::Up => self.player.send(PlayerCommand::AdjustVolume(VOLUME_STEP)),
            KeyCode::Down => self.player.send(PlayerCommand::AdjustVolume(-VOLUME_STEP)),
            KeyCode::Enter => self.play_selected(),
            KeyCode::Char(':') => {
                self.command_input.clear();
                self.mode = Mode::Command;
            }
            KeyCode::Char('/') => self.start_search(),
            KeyCode::Char('n') => self.jump_search(1),
            KeyCode::Char('N') => self.jump_search(-1),
            KeyCode::Esc => {
                if !self.command_input.is_empty() {
                    self.command_input.clear();
                    return;
                }

                if self.quit_prompted {
                    self.should_quit = true;
                    return;
                }

                self.set_error("Really quit? (esc again to quit)");
                self.quit_prompted = true;
            }
            _ => {}
        }

        if code != KeyCode::Esc {
            self.quit_prompted = false;
        }
        if !matches!(code, KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N')) {
            self.set_info("");
        }
    }

    /// Moves the cursor within whichever pane currently has focus (browsing only; does not
    /// affect what's playing).
    fn move_selection(&mut self, delta: isize) {
        match self.focus {
            Focus::Artists => {
                let len = self.library.artists.len();
                if len == 0 {
                    return;
                }
                self.selected_artist = clamp_index(self.selected_artist, delta, len);
                self.selected_song = 0;
            }
            Focus::Songs => {
                let Some(artist) = self.library.artists.get(self.selected_artist) else {
                    return;
                };
                let len = artist.songs.len();
                if len == 0 {
                    return;
                }
                self.selected_song = clamp_index(self.selected_song, delta, len);
            }
        }
    }

    /// Jumps the cursor within whichever pane currently has focus to the first item.
    fn move_to_start(&mut self) {
        match self.focus {
            Focus::Artists => {
                if !self.library.artists.is_empty() {
                    self.selected_artist = 0;
                    self.selected_song = 0;
                }
            }
            Focus::Songs => self.selected_song = 0,
        }
    }

    /// Jumps the cursor within whichever pane currently has focus to the last item.
    fn move_to_end(&mut self) {
        match self.focus {
            Focus::Artists => {
                let len = self.library.artists.len();
                if len > 0 {
                    self.selected_artist = len - 1;
                    self.selected_song = 0;
                }
            }
            Focus::Songs => {
                let Some(artist) = self.library.artists.get(self.selected_artist) else {
                    return;
                };
                if !artist.songs.is_empty() {
                    self.selected_song = artist.songs.len() - 1;
                }
            }
        }
    }

    fn is_playing(&mut self) -> bool {
        player::lock_status(&self.player_status).current.is_some()
    }

    fn toggle_play_pause(&mut self) {
        if self.is_playing() {
            self.player.send(PlayerCommand::TogglePause);
        } else {
            // Nothing is loaded yet: start playing whatever is currently selected.
            self.play_selected();
        }
    }

    /// Plays whatever is currently under the cursor, replacing any track already playing.
    fn play_selected(&mut self) {
        let (artist_idx, song_idx) = match self.focus {
            Focus::Songs => (self.selected_artist, self.selected_song),
            Focus::Artists => (self.selected_artist, 0),
        };
        self.play_track(artist_idx, song_idx);
    }

    /// Moves playback to the next/previous song within the artist of the currently playing
    /// track (not the browsing cursor). No-ops if nothing is currently playing, or if already
    /// at the first/last song of that artist.
    fn play_relative(&mut self, delta: isize) {
        let now = player::lock_status(&self.player_status).current.clone();
        let Some(now) = now else {
            return;
        };
        let Some(artist) = self.library.artists.get(now.artist_idx) else {
            return;
        };
        let len = artist.songs.len();
        if len == 0 {
            return;
        }
        let next_idx = clamp_index(now.song_idx, delta, len);
        if next_idx == now.song_idx {
            return;
        }
        self.play_track(now.artist_idx, next_idx);
    }

    /// Plays the given song and re-queues the rest of its album (the songs after it, in track
    /// order), replacing whatever was previously queued.
    fn play_track(&mut self, artist_idx: usize, song_idx: usize) {
        let Some(artist) = self.library.artists.get(artist_idx) else {
            self.set_info("No song selected.");
            return;
        };
        let Some(song) = artist.songs.get(song_idx) else {
            self.set_info("No song selected.");
            return;
        };

        self.player.send(PlayerCommand::Play(TrackRequest {
            path: song.path.clone(),
            artist: artist.name.clone(),
            title: song.title.clone(),
            duration: song.duration,
            artist_idx,
            song_idx,
        }));

        self.queue = artist
            .songs
            .iter()
            .enumerate()
            .skip(song_idx + 1)
            .take_while(|(_, s)| s.album == song.album && s.year == song.year)
            .map(|(idx, _)| (artist_idx, idx))
            .collect();
    }

    /// Advances to the next queued song when the current one has finished on its own. No-ops if
    /// the queue is empty (end of album).
    fn play_next_in_queue(&mut self) {
        if let Some((artist_idx, song_idx)) = self.queue.pop_front() {
            self.play_track(artist_idx, song_idx);
        }
    }

    /// Begins a search over whichever pane currently has focus. The pane searched, and the
    /// selection to restore on `Esc`, are pinned at this point so that later `n`/`N` cycling and
    /// cancellation are unaffected by focus changes made while typing.
    fn start_search(&mut self) {
        self.search_input.clear();
        self.search_matches.clear();
        self.search_match_pos = 0;
        self.search_focus = self.focus;
        self.search_origin = (self.selected_artist, self.selected_song);
        self.mode = Mode::Search;
    }

    /// Restores the pre-search selection and drops back to normal mode.
    fn cancel_search(&mut self) {
        self.selected_artist = self.search_origin.0;
        self.selected_song = self.search_origin.1;
        self.search_input.clear();
        self.search_matches.clear();
        self.mode = Mode::Normal;
    }

    /// Leaves search mode but keeps the current matches so `n`/`N` can keep cycling through them.
    fn finish_search(&mut self) {
        if self.search_input.is_empty() {
            return;
        }
        self.report_search_status();
    }

    /// Surfaces the current match position (or the lack of any match) in the status line.
    fn report_search_status(&mut self) {
        if self.search_matches.is_empty() {
            self.set_error(format!("no match: {}", self.search_input));
        } else {
            self.set_info(format!(
                "match {}/{} for \"{}\"",
                self.search_match_pos + 1,
                self.search_matches.len(),
                self.search_input
            ));
        }
    }

    /// Recomputes matches for the current search input against the pane the search started in,
    /// and jumps that pane's selection to the first match.
    fn update_search(&mut self) {
        self.search_matches = self.matching_indices(self.search_focus, &self.search_input);
        self.search_match_pos = 0;

        if let Some(&idx) = self.search_matches.first() {
            self.set_search_selection(idx);
        }
    }

    /// Indices of items in `focus`'s pane whose name matches `query`, best match first: an item
    /// starting with `query` ranks above one that merely contains it, which in turn ranks above a
    /// non-contiguous fuzzy match. Ties keep list order.
    fn matching_indices(&self, focus: Focus, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return Vec::new();
        }

        let mut matches: Vec<(usize, MatchKind)> = match focus {
            Focus::Artists => self
                .library
                .artists
                .iter()
                .enumerate()
                .filter_map(|(idx, a)| match_kind(query, &a.name).map(|kind| (idx, kind)))
                .collect(),
            Focus::Songs => {
                let Some(artist) = self.library.artists.get(self.selected_artist) else {
                    return Vec::new();
                };
                artist
                    .songs
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, s)| match_kind(query, &s.title).map(|kind| (idx, kind)))
                    .collect()
            }
        };

        matches.sort_by_key(|(_, kind)| *kind);
        matches.into_iter().map(|(idx, _)| idx).collect()
    }

    fn set_search_selection(&mut self, idx: usize) {
        match self.search_focus {
            Focus::Artists => {
                self.selected_artist = idx;
                self.selected_song = 0;
            }
            Focus::Songs => self.selected_song = idx,
        }
    }

    /// Cycles to the next (`delta = 1`) or previous (`delta = -1`) match from the last search,
    /// wrapping around, and re-focuses the pane that search was run against.
    fn jump_search(&mut self, delta: isize) {
        if self.search_matches.is_empty() {
            return;
        }
        let len = self.search_matches.len() as isize;
        self.search_match_pos = (self.search_match_pos as isize + delta).rem_euclid(len) as usize;
        self.focus = self.search_focus;
        let idx = self.search_matches[self.search_match_pos];
        self.set_search_selection(idx);
        self.report_search_status();
    }

    fn execute_command(&mut self) {
        let input = self.command_input.trim().to_string();
        self.command_input.clear();
        self.mode = Mode::Normal;
        if input.is_empty() {
            return;
        }

        let mut parts = input.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("").trim();

        match cmd {
            "add" | "a" => {
                if rest.is_empty() {
                    self.set_error("add requires a path argument".to_string());
                } else {
                    match library::scan(Path::new(rest)) {
                        Ok(entries) => {
                            let count = entries.len();
                            match self
                                .db
                                .upsert_entries(&entries)
                                .and_then(|()| self.reload_library())
                            {
                                Ok(()) => {
                                    self.set_info(format!("Imported {count} song(s) from {rest}"))
                                }
                                Err(e) => self.set_error(e.to_string()),
                            }
                        }
                        Err(e) => self.set_error(e.to_string()),
                    }
                }
            }
            "clear" => {
                self.player.send(PlayerCommand::Stop);
                self.queue.clear();
                self.selected_artist = 0;
                self.selected_song = 0;
                match self.db.clear().and_then(|()| self.reload_library()) {
                    Ok(()) => self.set_info("Library cleared."),
                    Err(e) => self.set_error(e.to_string()),
                }
            }
            "quit" => self.should_quit = true,
            other => self.set_error(format!("unknown command: {other}")),
        }
    }

    fn set_error(&mut self, text: impl Into<String>) {
        self.status_message = Some(StatusMessage {
            text: text.into(),
            is_error: true,
        });
    }

    fn set_info(&mut self, text: impl Into<String>) {
        self.status_message = Some(StatusMessage {
            text: text.into(),
            is_error: false,
        });
    }
}

/// Relevance of a search match: a match at the very start of the target ranks best, then any
/// contiguous substring match elsewhere. Declaration order doubles as rank order for the derived
/// `Ord`, so sorting by this ascending yields best-first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MatchKind {
    Prefix,
    Contains,
}

/// Ranks how `query` matches `target`, case-insensitively, or `None` if it doesn't match at all.
fn match_kind(query: &str, target: &str) -> Option<MatchKind> {
    let query = query.to_lowercase();
    let target = target.to_lowercase();

    if target.starts_with(&query) {
        Some(MatchKind::Prefix)
    } else if target.contains(&query) {
        Some(MatchKind::Contains)
    } else {
        None
    }
}

fn clamp_index(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = (len - 1) as isize;
    let next = current as isize + delta;
    next.clamp(0, max) as usize
}
