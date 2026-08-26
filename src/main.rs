mod app;
mod db;
mod library;
mod player;
mod ui;

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use app::App;
use player::PlayerStatus;

fn main() -> io::Result<()> {
    let mut terminal = ratatui::try_init()?;
    let result = run(&mut terminal);

    if let Err(e) = ratatui::try_restore() {
        eprintln!("failed to restore terminal: {e}");
    }

    if let Err(e) = &result {
        eprintln!("rmus exited with an error: {e}");
    }
    result
}

fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let db = db::Db::open().map_err(|e| io::Error::other(e.to_string()))?;

    let player_status = Arc::new(Mutex::new(PlayerStatus::default()));
    let player_handle = player::spawn(player_status.clone());
    let mut app = App::new(player_handle, player_status, db);

    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key.code);
        }

        app.sync_player_status();

        if app.should_quit {
            // Must happen before returning so the worker thread's audio output is torn down
            // and no music keeps playing after the application exits.
            app.player.shutdown();
            break;
        }
    }

    Ok(())
}
