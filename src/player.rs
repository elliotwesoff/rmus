use std::fs::File;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player as RodioPlayer};

/// A song queued up to be played by the worker thread. `artist_idx`/`song_idx` identify the
/// song's position within the library so the UI can compute "next"/"previous" without the
/// worker thread needing to know about the library itself.
#[derive(Debug, Clone)]
pub struct TrackRequest {
    pub path: PathBuf,
    pub artist: String,
    pub title: String,
    pub duration: Duration,
    pub artist_idx: usize,
    pub song_idx: usize,
}

#[derive(Debug, Clone)]
pub struct NowPlaying {
    pub artist: String,
    pub title: String,
    pub duration: Duration,
    pub artist_idx: usize,
    pub song_idx: usize,
}

#[derive(Debug)]
pub struct PlayerStatus {
    pub current: Option<NowPlaying>,
    pub elapsed: Duration,
    pub is_paused: bool,
    pub volume: f32,
    pub error: Option<String>,
    /// Set when the current track ran to completion on its own (as opposed to being replaced or
    /// explicitly stopped), so the UI thread knows to advance the queue.
    pub just_finished: bool,
}

impl Default for PlayerStatus {
    fn default() -> Self {
        Self {
            current: None,
            elapsed: Duration::ZERO,
            is_paused: false,
            // Matches rodio's own default sink volume (1.0 = unfiltered), so the UI doesn't show
            // 0% before the worker thread's first status refresh arrives.
            volume: 1.0,
            error: None,
            just_finished: false,
        }
    }
}

/// Locks the shared status, recovering the inner data if a prior holder panicked instead of
/// propagating the poison and taking the whole application down with it.
pub fn lock_status(status: &Mutex<PlayerStatus>) -> MutexGuard<'_, PlayerStatus> {
    status.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub enum PlayerCommand {
    Play(TrackRequest),
    TogglePause,
    Restart,
    Stop,
    /// Adjusts the volume by `delta`, clamped to the [0.0, 1.0] range.
    AdjustVolume(f32),
    Shutdown,
}

pub struct PlayerHandle {
    tx: Sender<PlayerCommand>,
    join: Option<JoinHandle<()>>,
}

impl PlayerHandle {
    pub fn send(&self, cmd: PlayerCommand) {
        // The worker thread only ever disconnects during shutdown, at which point there is
        // nothing left to command; silently dropping the command is correct.
        let _ = self.tx.send(cmd);
    }

    pub fn shutdown(&mut self) {
        self.send(PlayerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            // If the worker thread panicked, there's nothing left to clean up on its side
            // (dropping its Engine already killed the audio callback) and nothing further we
            // can do from here; either way we must not panic ourselves while exiting.
            let _ = join.join();
        }
    }
}

pub fn spawn(status: Arc<Mutex<PlayerStatus>>) -> PlayerHandle {
    let (tx, rx) = mpsc::channel();
    let join = thread::spawn(move || run(rx, status));
    PlayerHandle {
        tx,
        join: Some(join),
    }
}

struct Engine {
    _sink_handle: MixerDeviceSink,
    player: RodioPlayer,
}

fn run(rx: Receiver<PlayerCommand>, status: Arc<Mutex<PlayerStatus>>) {
    let engine = match DeviceSinkBuilder::open_default_sink() {
        Ok(sink_handle) => {
            let player = RodioPlayer::connect_new(sink_handle.mixer());
            Some(Engine {
                _sink_handle: sink_handle,
                player,
            })
        }
        Err(err) => {
            lock_status(&status).error = Some(format!("audio output unavailable: {err}"));
            None
        }
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(PlayerCommand::Play(req)) => handle_play(&engine, &status, req),
            Ok(PlayerCommand::TogglePause) => handle_toggle_pause(&engine, &status),
            Ok(PlayerCommand::Restart) => handle_restart(&engine, &status),
            Ok(PlayerCommand::Stop) => handle_stop(&engine, &status),
            Ok(PlayerCommand::AdjustVolume(delta)) => handle_adjust_volume(&engine, delta),
            Ok(PlayerCommand::Shutdown) => {
                handle_stop(&engine, &status);
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        refresh_status(&engine, &status);
    }
}

fn handle_play(engine: &Option<Engine>, status: &Mutex<PlayerStatus>, req: TrackRequest) {
    let Some(engine) = engine else {
        lock_status(status).error = Some("no audio output device available".to_string());
        return;
    };

    let source = File::open(&req.path)
        .map_err(|e| e.to_string())
        .and_then(|f| Decoder::try_from(f).map_err(|e| e.to_string()));

    match source {
        Ok(source) => {
            engine.player.stop();
            engine.player.append(source);
            engine.player.play();

            let mut st = lock_status(status);
            st.current = Some(NowPlaying {
                artist: req.artist,
                title: req.title,
                duration: req.duration,
                artist_idx: req.artist_idx,
                song_idx: req.song_idx,
            });
            st.elapsed = Duration::ZERO;
            st.is_paused = false;
            st.error = None;
        }
        Err(msg) => {
            lock_status(status).error = Some(format!("{}: {}", req.path.display(), msg));
        }
    }
}

fn handle_toggle_pause(engine: &Option<Engine>, status: &Mutex<PlayerStatus>) {
    let Some(engine) = engine else {
        return;
    };
    if lock_status(status).current.is_none() {
        return;
    }
    if engine.player.is_paused() {
        engine.player.play();
    } else {
        engine.player.pause();
    }
    lock_status(status).is_paused = engine.player.is_paused();
}

fn handle_restart(engine: &Option<Engine>, status: &Mutex<PlayerStatus>) {
    let Some(engine) = engine else {
        return;
    };
    if lock_status(status).current.is_none() {
        return;
    }
    match engine.player.try_seek(Duration::ZERO) {
        Ok(()) => lock_status(status).elapsed = Duration::ZERO,
        Err(e) => lock_status(status).error = Some(e.to_string()),
    }
}

fn handle_stop(engine: &Option<Engine>, status: &Mutex<PlayerStatus>) {
    if let Some(engine) = engine {
        engine.player.stop();
    }
    let mut st = lock_status(status);
    st.current = None;
    st.elapsed = Duration::ZERO;
    st.is_paused = false;
}

fn handle_adjust_volume(engine: &Option<Engine>, delta: f32) {
    let Some(engine) = engine else {
        return;
    };
    let new_volume = (engine.player.volume() + delta).clamp(0.0, 1.0);
    engine.player.set_volume(new_volume);
}

fn refresh_status(engine: &Option<Engine>, status: &Mutex<PlayerStatus>) {
    let Some(engine) = engine else {
        return;
    };
    let mut st = lock_status(status);
    st.volume = engine.player.volume();
    if st.current.is_some() {
        st.elapsed = engine.player.get_pos();
        st.is_paused = engine.player.is_paused();
        if engine.player.empty() {
            st.current = None;
            st.elapsed = Duration::ZERO;
            st.is_paused = false;
            st.just_finished = true;
        }
    }
}
