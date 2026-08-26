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
- Case-insensitive search within the active pane, with match cycling
- Delete an artist or song from the library (with confirmation) without
  touching the files on disk
- Tag reading via [lofty](https://github.com/Serial-ATA/lofty-rs) (artist,
  album, year, track number, title, duration, bitrate), with bitrate and
  track time shown alongside each song
- Library persisted in a local SQLite database (via `rusqlite`), so imports
  only need to happen once

## Usage

Run it from source:

```sh
cargo run --release
```

or `./target/release/rmus` after building.

On first run your library will be empty — use the `:add` command below to
import some music.

### Keybindings

| Key         | Action                                                                |
|-------------|-----------------------------------------------------------------------|
| `j` / `k`   | Move selection down / up                                              |
| `g` / `Home`| Jump to first item                                                    |
| `G` / `End` | Jump to last item                                                     |
| `h` / `←`   | Focus the artists pane                                                |
| `l` / `→`   | Focus the songs pane                                                  |
| `Tab`       | Toggle focus between the two panes                                    |
| `Enter`     | Play the selected artist/song                                         |
| `c`         | Toggle play/pause                                                     |
| `x`         | Restart the current track                                             |
| `z` / `v`   | Previous / next track (within the album)                              |
| `↑` / `↓`   | Volume up / down                                                      |
| `:`         | Enter command mode                                                    |
| `/`         | Search the active pane                                                |
| `n` / `N`   | Jump to next / previous search match                                  |
| `d`         | Delete the selected artist/song (asks first, never deletes from disk) |
| `Esc`       | Cancel input, then quit (press twice)                                 |

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
config directory.

## License

Licensed under the [GNU General Public License v3.0](LICENSE) or later.

---

*This project was created with [Claude Code](https://claude.com/claude-code), using Claude Sonnet 5.*
