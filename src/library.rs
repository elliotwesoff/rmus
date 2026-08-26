use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lofty::prelude::{Accessor, AudioFile, TaggedFileExt};

#[derive(Debug, Clone)]
pub struct Song {
    pub title: String,
    pub album: String,
    pub year: Option<u16>,
    pub track_number: Option<u32>,
    pub path: PathBuf,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct Artist {
    pub name: String,
    pub songs: Vec<Song>,
}

#[derive(Debug, Default)]
pub struct Library {
    pub artists: Vec<Artist>,
}

#[derive(Debug)]
pub struct LibraryError {
    message: String,
}

impl fmt::Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LibraryError {}

impl From<std::io::Error> for LibraryError {
    fn from(err: std::io::Error) -> Self {
        LibraryError {
            message: err.to_string(),
        }
    }
}

/// One song's full tag data in flat form, independent of how it's grouped for display. This is
/// the shape shared by both scanning (`scan`) and the database (see `db.rs`), so a `Library` is
/// always built the same way regardless of where the entries came from.
#[derive(Debug, Clone)]
pub struct LibraryEntry {
    pub path: PathBuf,
    pub artist: String,
    pub album: String,
    pub year: Option<u16>,
    pub track_number: Option<u32>,
    pub title: String,
    pub duration: Duration,
}

impl Library {
    /// Groups flat entries by artist and sorts artists/albums/songs into display order.
    pub fn from_entries(entries: Vec<LibraryEntry>) -> Self {
        let mut library = Library::default();

        for entry in entries {
            let artist_idx = match library.artists.iter().position(|a| a.name == entry.artist) {
                Some(idx) => idx,
                None => {
                    library.artists.push(Artist {
                        name: entry.artist,
                        songs: Vec::new(),
                    });
                    library.artists.len() - 1
                }
            };
            library.artists[artist_idx].songs.push(Song {
                title: entry.title,
                album: entry.album,
                year: entry.year,
                track_number: entry.track_number,
                path: entry.path,
                duration: entry.duration,
            });
        }

        library.artists.sort_by(|a, b| a.name.cmp(&b.name));
        for artist in &mut library.artists {
            artist.songs.sort_by(|a, b| {
                album_order(a, b)
                    .then_with(|| {
                        a.track_number
                            .unwrap_or(u32::MAX)
                            .cmp(&b.track_number.unwrap_or(u32::MAX))
                    })
                    .then_with(|| a.title.cmp(&b.title))
            });
        }

        library
    }
}

/// Recursively scans `root` for mp3 files and reads their tags, skipping any file whose tags
/// can't be read. Does not touch the database or any in-memory `Library`.
pub fn scan(root: &Path) -> Result<Vec<LibraryEntry>, LibraryError> {
    if !root.exists() {
        return Err(LibraryError {
            message: format!("path does not exist: {}", root.display()),
        });
    }

    let mut mp3_paths = Vec::new();
    collect_mp3_files(root, &mut mp3_paths)?;

    let mut entries = Vec::new();
    for path in mp3_paths {
        let Ok(tags) = read_tags(&path) else {
            continue;
        };
        entries.push(LibraryEntry {
            path,
            artist: tags.artist,
            album: tags.album,
            year: tags.year,
            track_number: tags.track_number,
            title: tags.title,
            duration: tags.duration,
        });
    }

    Ok(entries)
}

/// Orders songs by album: albums with a year sort newest-first, and any album missing a year
/// sorts after all dated albums, alphabetically among themselves.
fn album_order(a: &Song, b: &Song) -> std::cmp::Ordering {
    if a.album == b.album && a.year == b.year {
        return std::cmp::Ordering::Equal;
    }
    match (a.year, b.year) {
        (Some(ya), Some(yb)) if ya != yb => yb.cmp(&ya),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        _ => a.album.cmp(&b.album),
    }
}

fn collect_mp3_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), LibraryError> {
    if dir.is_file() {
        if is_mp3(dir) {
            out.push(dir.to_path_buf());
        }
        return Ok(());
    }

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_mp3_files(&path, out)?;
        } else if is_mp3(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_mp3(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("mp3"))
        .unwrap_or(false)
}

struct SongTags {
    artist: String,
    title: String,
    album: String,
    year: Option<u16>,
    track_number: Option<u32>,
    duration: Duration,
}

/// Reads artist/title/album/year/track-number/duration from an mp3's ID3 tags, falling back to
/// "Unknown Artist" / "Unknown Album" / the file stem when tags are missing.
fn read_tags(path: &Path) -> Result<SongTags, LibraryError> {
    let tagged_file = lofty::read_from_path(path).map_err(|e| LibraryError {
        message: format!("{}: {}", path.display(), e),
    })?;

    let duration = tagged_file.properties().duration();
    let tag = tagged_file.primary_tag().or_else(|| tagged_file.first_tag());

    let artist = tag
        .and_then(|t| t.artist())
        .map(|c| c.into_owned())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Unknown Artist".to_string());

    let title = tag
        .and_then(|t| t.title())
        .map(|c| c.into_owned())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown Title")
                .to_string()
        });

    let album = tag
        .and_then(|t| t.album())
        .map(|c| c.into_owned())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Unknown Album".to_string());

    let year = tag.and_then(|t| t.date()).map(|d| d.year);

    let track_number = tag.and_then(|t| t.track());

    Ok(SongTags {
        artist,
        title,
        album,
        year,
        track_number,
        duration,
    })
}
