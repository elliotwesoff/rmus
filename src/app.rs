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
    pub status_message: Option<StatusMessage>,
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
            status_message: None,
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
            KeyCode::Char(':') | KeyCode::Char('i') => {
                self.command_input.clear();
                self.mode = Mode::Command;
            }
            KeyCode::Esc => self.should_quit = true,
            _ => {}
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

    fn toggle_play_pause(&mut self) {
        let has_current = player::lock_status(&self.player_status).current.is_some();
        if has_current {
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
            "add" => {
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

fn clamp_index(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let max = (len - 1) as isize;
    let next = current as isize + delta;
    next.clamp(0, max) as usize
}
