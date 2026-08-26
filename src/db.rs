use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use directories::ProjectDirs;
use rusqlite::Connection;

use crate::library::{Library, LibraryEntry};

const CREATE_TABLE_SQL: &str = "
    CREATE TABLE IF NOT EXISTS songs (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        path         TEXT    NOT NULL UNIQUE,
        artist       TEXT    NOT NULL,
        album        TEXT    NOT NULL,
        year         INTEGER,
        track_number INTEGER,
        title        TEXT    NOT NULL,
        duration_ms  INTEGER NOT NULL
    )";

const UPSERT_SQL: &str = "
    INSERT INTO songs (path, artist, album, year, track_number, title, duration_ms)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
    ON CONFLICT(path) DO UPDATE SET
        artist = excluded.artist,
        album = excluded.album,
        year = excluded.year,
        track_number = excluded.track_number,
        title = excluded.title,
        duration_ms = excluded.duration_ms";

const SELECT_ALL_SQL: &str =
    "SELECT path, artist, album, year, track_number, title, duration_ms FROM songs";

pub struct Db {
    conn: Connection,
}

#[derive(Debug)]
pub struct DbError {
    message: String,
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(err: rusqlite::Error) -> Self {
        DbError {
            message: err.to_string(),
        }
    }
}

impl From<std::io::Error> for DbError {
    fn from(err: std::io::Error) -> Self {
        DbError {
            message: err.to_string(),
        }
    }
}

impl Db {
    /// Opens (creating if needed) the library database in the user's XDG data directory.
    pub fn open() -> Result<Self, DbError> {
        let dirs = ProjectDirs::from("", "", "rmus").ok_or_else(|| DbError {
            message: "could not determine a data directory for this platform".to_string(),
        })?;
        fs::create_dir_all(dirs.data_dir())?;

        let conn = Connection::open(dirs.data_dir().join("library.db"))?;
        conn.execute(CREATE_TABLE_SQL, [])?;

        Ok(Db { conn })
    }

    /// Inserts or, for a path already present, updates the row with freshly-scanned tag data.
    pub fn upsert_entries(&mut self, entries: &[LibraryEntry]) -> Result<(), DbError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(UPSERT_SQL)?;
            for entry in entries {
                stmt.execute(rusqlite::params![
                    entry.path.to_string_lossy(),
                    entry.artist,
                    entry.album,
                    entry.year.map(i64::from),
                    entry.track_number.map(i64::from),
                    entry.title,
                    entry.duration.as_millis() as i64,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes every song from the database.
    pub fn clear(&self) -> Result<(), DbError> {
        self.conn.execute("DELETE FROM songs", [])?;
        Ok(())
    }

    /// Removes the given paths from the database. Never touches the files on disk.
    pub fn remove_paths(&mut self, paths: &[PathBuf]) -> Result<(), DbError> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare("DELETE FROM songs WHERE path = ?1")?;
            for path in paths {
                stmt.execute(rusqlite::params![path.to_string_lossy()])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Loads every known song and groups it into a `Library`. This is the single function used
    /// to build the in-memory library, whether at startup or after any command that mutates the
    /// database, so the database is always the one source of truth for library contents.
    pub fn load_library(&self) -> Result<Library, DbError> {
        let mut stmt = self.conn.prepare(SELECT_ALL_SQL)?;
        let entries = stmt
            .query_map([], |row| {
                let path: String = row.get(0)?;
                let year: Option<i64> = row.get(3)?;
                let track_number: Option<i64> = row.get(4)?;
                let duration_ms: i64 = row.get(6)?;
                Ok(LibraryEntry {
                    path: PathBuf::from(path),
                    artist: row.get(1)?,
                    album: row.get(2)?,
                    year: year.map(|y| y as u16),
                    track_number: track_number.map(|t| t as u32),
                    title: row.get(5)?,
                    duration: Duration::from_millis(duration_ms as u64),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(Library::from_entries(entries))
    }
}
