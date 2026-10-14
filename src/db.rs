use crate::audible::library::LibraryItem;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

/// SQLite behind a Mutex, so `&Db` can cross `await`s in tokio tasks.
pub struct Db(Mutex<Connection>);

#[derive(Debug, Clone)]
pub struct Book {
    pub asin: String,
    pub title: String,
    pub authors: String,
    pub series: String,
    pub runtime_ms: i64,
    pub percent_complete: f64,
    pub cover_url: Option<String>,
    /// Folder with the downloaded chapters.
    pub path: Option<PathBuf>,
    pub acr: Option<String>,
    pub content_format: Option<String>,
    /// Local position, if listened to here.
    pub position_ms: Option<i64>,
}

impl Book {
    pub fn finished(&self) -> bool {
        self.percent_complete >= 100.0
    }

    pub fn in_progress(&self) -> bool {
        !self.finished() && (self.percent_complete > 0.0 || self.position_ms.is_some_and(|p| p > 0))
    }
}

/// Speed and volume remembered per book.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub speed: f64,
    pub volume: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { speed: 1.0, volume: 100 }
    }
}

/// Position saved locally.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalPos {
    pub position_ms: i64,
    pub updated_ms: i64,
    /// Last time it was sent to or received from Audible.
    pub synced_ms: Option<i64>,
}

pub struct Pending {
    pub asin: String,
    pub acr: String,
    pub position_ms: i64,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        crate::config::secure_file(path)?;
        Self::init(Connection::open(path)?)
    }

    #[cfg(test)]
    pub fn memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(c: Connection) -> Result<Self> {
        c.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS books (
                 asin TEXT PRIMARY KEY, title TEXT NOT NULL, authors TEXT NOT NULL,
                 narrators TEXT NOT NULL, series TEXT NOT NULL, runtime_ms INTEGER NOT NULL,
                 percent_complete REAL NOT NULL, cover_url TEXT,
                 path TEXT, acr TEXT, content_format TEXT);
             CREATE TABLE IF NOT EXISTS positions (
                 asin TEXT PRIMARY KEY, position_ms INTEGER NOT NULL,
                 updated_ms INTEGER NOT NULL, synced_ms INTEGER);
             CREATE TABLE IF NOT EXISTS pending_positions (
                 asin TEXT PRIMARY KEY, acr TEXT NOT NULL, position_ms INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS book_settings (
                 asin TEXT PRIMARY KEY, speed REAL NOT NULL, volume INTEGER NOT NULL);",
        )?;
        Ok(Self(Mutex::new(c)))
    }

    fn c(&self) -> MutexGuard<'_, Connection> {
        // A query that panicked does not leave the connection inconsistent.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Updates the catalog without touching download info (path, acr).
    pub fn upsert_books(&self, items: &[LibraryItem]) -> Result<()> {
        let mut c = self.c();
        let tx = c.transaction()?;
        {
            let mut st = tx.prepare(
                "INSERT INTO books (asin, title, authors, narrators, series, runtime_ms, percent_complete, cover_url)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(asin) DO UPDATE SET title = ?2, authors = ?3, narrators = ?4, series = ?5,
                     runtime_ms = ?6, percent_complete = ?7, cover_url = ?8",
            )?;
            for i in items {
                st.execute(params![
                    i.asin,
                    i.title,
                    i.authors,
                    i.narrators,
                    i.series,
                    i.runtime_ms,
                    i.percent_complete,
                    i.cover_url
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn is_empty(&self) -> Result<bool> {
        Ok(self.c().query_row("SELECT COUNT(*) FROM books", [], |r| r.get::<_, i64>(0))? == 0)
    }

    /// Books whose title, author or series contain `query` (case-insensitive),
    /// most recently listened first.
    pub fn search(&self, query: &str) -> Result<Vec<Book>> {
        let c = self.c();
        let mut st = c.prepare(
            "SELECT b.asin, b.title, b.authors, b.series, b.runtime_ms, b.percent_complete, b.cover_url,
                    b.path, b.acr, b.content_format, p.position_ms
             FROM books b LEFT JOIN positions p ON p.asin = b.asin
             WHERE b.title LIKE ?1 OR b.authors LIKE ?1 OR b.series LIKE ?1
             ORDER BY COALESCE(p.updated_ms, 0) DESC, b.title",
        )?;
        let rows = st.query_map([format!("%{query}%")], |r| {
            Ok(Book {
                asin: r.get(0)?,
                title: r.get(1)?,
                authors: r.get(2)?,
                series: r.get(3)?,
                runtime_ms: r.get(4)?,
                percent_complete: r.get(5)?,
                cover_url: r.get(6)?,
                path: r.get::<_, Option<String>>(7)?.map(PathBuf::from),
                acr: r.get(8)?,
                content_format: r.get(9)?,
                position_ms: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn settings(&self, asin: &str) -> Result<Settings> {
        Ok(self
            .c()
            .query_row("SELECT speed, volume FROM book_settings WHERE asin = ?1", [asin], |r| {
                Ok(Settings { speed: r.get(0)?, volume: r.get(1)? })
            })
            .optional()?
            .unwrap_or_default())
    }

    pub fn save_settings(&self, asin: &str, s: Settings) -> Result<()> {
        self.c().execute(
            "INSERT INTO book_settings (asin, speed, volume) VALUES (?1, ?2, ?3)
             ON CONFLICT(asin) DO UPDATE SET speed = ?2, volume = ?3",
            params![asin, s.speed, s.volume],
        )?;
        Ok(())
    }

    pub fn set_downloaded(&self, asin: &str, path: &Path, acr: &str, content_format: &str) -> Result<()> {
        self.c().execute(
            "UPDATE books SET path = ?2, acr = ?3, content_format = ?4 WHERE asin = ?1",
            params![asin, path.to_string_lossy(), acr, content_format],
        )?;
        Ok(())
    }

    pub fn position(&self, asin: &str) -> Result<Option<LocalPos>> {
        Ok(self
            .c()
            .query_row("SELECT position_ms, updated_ms, synced_ms FROM positions WHERE asin = ?1", [asin], |r| {
                Ok(LocalPos { position_ms: r.get(0)?, updated_ms: r.get(1)?, synced_ms: r.get(2)? })
            })
            .optional()?)
    }

    pub fn save_position(&self, asin: &str, position_ms: i64, now_ms: i64) -> Result<()> {
        self.c().execute(
            "INSERT INTO positions (asin, position_ms, updated_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(asin) DO UPDATE SET position_ms = ?2, updated_ms = ?3",
            params![asin, position_ms, now_ms],
        )?;
        Ok(())
    }

    /// Marks the position as equal to Audible's at `now_ms`.
    pub fn mark_synced(&self, asin: &str, position_ms: i64, now_ms: i64) -> Result<()> {
        self.c().execute(
            "INSERT INTO positions (asin, position_ms, updated_ms, synced_ms) VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT(asin) DO UPDATE SET position_ms = ?2, updated_ms = ?3, synced_ms = ?3",
            params![asin, position_ms, now_ms],
        )?;
        self.c().execute("DELETE FROM pending_positions WHERE asin = ?1", [asin])?;
        Ok(())
    }

    /// Queues a failed push; only the latest per book matters.
    pub fn enqueue(&self, asin: &str, acr: &str, position_ms: i64) -> Result<()> {
        self.c().execute(
            "INSERT INTO pending_positions (asin, acr, position_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(asin) DO UPDATE SET acr = ?2, position_ms = ?3",
            params![asin, acr, position_ms],
        )?;
        Ok(())
    }

    pub fn pending(&self) -> Result<Vec<Pending>> {
        let c = self.c();
        let mut st = c.prepare("SELECT asin, acr, position_ms FROM pending_positions")?;
        let rows = st.query_map([], |r| Ok(Pending { asin: r.get(0)?, acr: r.get(1)?, position_ms: r.get(2)? }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_queue_keeps_last_and_clears_on_sync() {
        let db = Db::memory().unwrap();
        db.enqueue("A", "acr", 100).unwrap();
        db.enqueue("A", "acr", 200).unwrap();
        db.enqueue("B", "acr", 50).unwrap();
        let mut p: Vec<_> = db.pending().unwrap().into_iter().map(|p| (p.asin, p.position_ms)).collect();
        p.sort();
        assert_eq!(p, [("A".into(), 200), ("B".into(), 50)]);

        db.mark_synced("A", 200, 1_000).unwrap();
        assert_eq!(db.pending().unwrap().len(), 1);
        assert_eq!(
            db.position("A").unwrap(),
            Some(LocalPos { position_ms: 200, updated_ms: 1_000, synced_ms: Some(1_000) })
        );
    }

    #[test]
    fn save_position_keeps_synced_mark() {
        let db = Db::memory().unwrap();
        db.mark_synced("A", 10, 1).unwrap();
        db.save_position("A", 20, 2).unwrap();
        assert_eq!(db.position("A").unwrap(), Some(LocalPos { position_ms: 20, updated_ms: 2, synced_ms: Some(1) }));
    }
}
