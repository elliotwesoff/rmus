use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, List, ListItem, Paragraph};

use crate::app::{App, Focus, Mode};
use crate::player;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(root[0]);

    draw_artists(frame, app, top[0]);
    draw_songs(frame, app, top[1]);
    draw_now_playing(frame, app, root[1]);
    draw_command(frame, app, root[2]);
}

fn focused_border_style(is_focused: bool) -> Style {
    if is_focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    }
}

fn draw_artists(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Artists;
    let items: Vec<ListItem> = app
        .library
        .artists
        .iter()
        .map(|a| ListItem::new(a.name.clone()))
        .collect();

    let block = Block::bordered()
        .title("Artists")
        .border_style(focused_border_style(focused));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let selected = (!app.library.artists.is_empty()).then_some(app.selected_artist);
    app.artist_list_state.select(selected);

    frame.render_stateful_widget(list, area, &mut app.artist_list_state);
}

fn draw_songs(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Songs;
    let now_playing = player::lock_status(&app.player_status).current.clone();
    // Border consumes one column on each side of the list.
    let inner_width = area.width.saturating_sub(2) as usize;

    // Songs are pre-sorted by album, so album boundaries are just runs of equal (album, year);
    // header rows are inserted inline, which shifts the selected song's row index within the
    // list, so we track that shifted index (`selected_row`) alongside it.
    let mut items: Vec<ListItem> = Vec::new();
    let mut selected_row: Option<usize> = None;

    if let Some(artist) = app.library.artists.get(app.selected_artist) {
        let mut last_album: Option<(&str, Option<u16>)> = None;

        for (idx, s) in artist.songs.iter().enumerate() {
            let album_key = (s.album.as_str(), s.year);
            if last_album != Some(album_key) {
                let header = match s.year {
                    Some(year) => format!("{} ({year})", s.album),
                    None => s.album.clone(),
                };
                items.push(ListItem::new(header).style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                ));
                last_album = Some(album_key);
            }

            if idx == app.selected_song {
                selected_row = Some(items.len());
            }

            let track = s
                .track_number
                .map(|t| format!("{t:02}."))
                .unwrap_or_else(|| "-- ".to_string());
            let is_playing = now_playing
                .as_ref()
                .is_some_and(|np| np.artist_idx == app.selected_artist && np.song_idx == idx);

            let title = format_title(s.title.clone(), area, 25); // 25 is len of track num, bitrate, time
            let marker = if is_playing { "▶ " } else { "  " };
            let left = format!("{marker}{track} {}", title);
            let bitrate = s
                .bitrate
                .map(|kbps| format!("{kbps} kbps"))
                .unwrap_or_else(|| "-- kbps".to_string());
            let right = format!("{bitrate} {:>5}", format_duration(s.duration));
            let padding = inner_width
                .saturating_sub(left.chars().count() + right.chars().count())
                .max(1);
            let text = format!("{left}{:padding$}{right}", "");

            let item = if is_playing {
                ListItem::new(text).style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ListItem::new(text)
            };
            items.push(item);
        }
    }

    let block = Block::bordered()
        .title("Songs")
        .border_style(focused_border_style(focused));

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    app.song_list_state.select(selected_row);

    frame.render_stateful_widget(list, area, &mut app.song_list_state);
}

fn draw_now_playing(frame: &mut Frame, app: &App, area: Rect) {
    let status = player::lock_status(&app.player_status);

    let (left, right) = match &status.current {
        Some(now) => {
            let title = format_title(now.title.clone(), area, 50);
            let state = if status.is_paused { "Paused" } else { "Playing" };
            let left = format!("[{state}] {} - {}", now.artist, title);
            let right = format!(
                "{}/{}  Vol {:>4}",
                format_duration(status.elapsed),
                format_duration(now.duration),
                format!("{:.0}%", status.volume * 100.0),
            );
            (left, right)
        }
        None => ("Nothing playing.".to_string(), String::new()),
    };
    drop(status);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(right.chars().count() as u16),
        ])
        .split(area);

    frame.render_widget(Paragraph::new(left), chunks[0]);
    frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), chunks[1]);
}

fn draw_command(frame: &mut Frame, app: &App, area: Rect) {
    match app.mode {
        Mode::Command => {
            let text = format!(":{}", app.command_input);
            frame.render_widget(Paragraph::new(text.clone()), area);
            frame.set_cursor_position(Position::new(
                area.x + text.len() as u16,
                area.y,
            ));
        }
        Mode::Search => {
            let text = format!("/{}", app.search_input);
            frame.render_widget(Paragraph::new(text.clone()), area);
            frame.set_cursor_position(Position::new(area.x + text.len() as u16, area.y));
        }
        Mode::ConfirmDelete => {
            let text = app.delete_prompt().unwrap_or_default();
            frame.render_widget(
                Paragraph::new(text).style(Style::default().fg(Color::Red)),
                area,
            );
        }
        Mode::Normal => {
            let (text, style) = match &app.status_message {
                Some(msg) if msg.is_error => (msg.text.clone(), Style::default().fg(Color::Red)),
                Some(msg) => (msg.text.clone(), Style::default()),
                None => (
                    "Press : to enter commands, / to search, d to delete, <esc> to quit."
                        .to_string(),
                    Style::default().fg(Color::DarkGray),
                ),
            };
            frame.render_widget(Paragraph::new(text).style(style), area);
        }
    }
}

fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

fn format_title(mut title: String, area: Rect, buffer_area: u16) -> String {
    let max_len = (area.width - buffer_area) as usize;

    if title.len() > max_len {
        title = format!("{}…", &title[0..max_len]);
    }

    title
}
