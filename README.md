# rmus

A terminal music player for MP3 libraries, written in Rust with [ratatui](https://ratatui.rs/).

Point it at a directory of MP3s, browse by artist and album in a two-pane
TUI, and play. Your library is indexed into a local SQLite database, so
subsequent launches start up instantly without rescanning.

## Features

- Two-pane browser: artists on the left, songs (grouped by album, newest
  first) on the right
- Playback via [rodio](https://github.com/RustAudio/rodio): play/pause,
  restart, skip to next/previous track, volume control
- Auto-advance through the rest of an album when a track finishes
- Fuzzy, case-insensitive search within the active pane, with match
  cycling
- Tag reading via [lofty](https://github.com/Serial-ATA/lofty-rs) (artist,
  album, year, track number, title, duration)
- Library persisted in a local SQLite database (via `rusqlite`), so imports
  only need to happen once

## Installation

Requires a Rust toolchain (edition 2024).

```sh
cargo build --release
```

The binary will be at `target/release/rmus`.

## Usage

Run it from anywhere:

```sh
cargo run --release
```

or `./target/release/rmus` after building.

On first run your library will be empty — use the `:add` command below to
import some music.

### Keybindings

| Key         | Action                                    |
|-------------|--------------------------------------------|
| `j` / `k`   | Move selection down / up                   |
| `g` / `Home`| Jump to first item                         |
| `G` / `End` | Jump to last item                          |
| `h`         | Focus the artists pane                     |
| `l`         | Focus the songs pane                       |
| `Enter`     | Play the selected artist/song              |
| `c`         | Toggle play/pause                          |
| `x`         | Restart the current track                  |
| `z` / `v`   | Previous / next track (within the album)   |
| `↑` / `↓`   | Volume up / down                           |
| `:`         | Enter command mode                         |
| `/`         | Search the active pane                     |
| `n` / `N`   | Jump to next / previous search match       |
| `Esc`       | Cancel input, then quit (press twice)      |

### Search

Press `/` to search whichever pane (artists or songs) currently has focus.
Matching is fuzzy and case-insensitive, and the selection jumps to the best
match as you type — an item starting with your query ranks above one that
just contains it elsewhere, which in turn ranks above a looser, non-
contiguous match. Press `Enter` to stop typing and keep the selection, `Esc`
to cancel and restore your previous selection, or `n` / `N` to cycle forward
/ backward through the remaining matches (wrapping around).

### Commands

Enter command mode with `:`, then:

| Command            | Effect                                            |
|--------------------|----------------------------------------------------|
| `:add <path>`      | Recursively scan `<path>` for MP3s and import/update them in the library. Alias: `:a <path>` |
| `:clear`           | Stop playback and wipe the entire library          |
| `:quit`            | Quit rmus                                          |

Example:

```
:add ~/Music
```

### Data storage

The song library is kept in a SQLite database in your platform's standard
config directory (via the [`directories`](https://docs.rs/directories)
crate) — no configuration needed.

## License

Licensed under the [GNU General Public License v3.0](LICENSE) or later.

---

*This project was created with [Claude Code](https://claude.com/claude-code), using Claude Sonnet 5.*
