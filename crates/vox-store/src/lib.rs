//! SQLite adapter for the `History` and `Lexicon` ports. WAL mode, so a crash mid-recording
//! leaves every already-written segment intact.
#![cfg_attr(test, allow(clippy::unwrap_used))]

use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use vox_core::history::{Entry, History, RecordingId, Status};
use vox_core::lexicon::{Fix, Lexicon};
use vox_core::CoreError;

/// Append-only list of schema migrations; index + 1 is the `user_version`.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE recordings (
        id INTEGER PRIMARY KEY,
        started_at INTEGER NOT NULL,
        status TEXT NOT NULL,
        final_text TEXT,
        error TEXT
    );
    CREATE TABLE segments (
        recording_id INTEGER NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
        idx INTEGER NOT NULL,
        text TEXT NOT NULL,
        PRIMARY KEY (recording_id, idx)
    );
",
// Personal vocabulary and learned corrections.
"
    CREATE TABLE words (
        word TEXT PRIMARY KEY COLLATE NOCASE,
        added_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
    );
    CREATE TABLE fixes (
        from_text TEXT PRIMARY KEY COLLATE NOCASE,
        to_text TEXT NOT NULL,
        added_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
    );
"];

pub struct SqliteStore {
    conn: Mutex<Connection>,
}

fn err(e: impl std::fmt::Display) -> CoreError {
    CoreError::History(e.to_string())
}

fn parse_status(s: &str) -> Status {
    match s {
        "completed" => Status::Completed,
        "failed" => Status::Failed,
        _ => Status::Recording,
    }
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self, CoreError> {
        Self::init(Connection::open(path).map_err(err)?)
    }

    pub fn open_in_memory() -> Result<Self, CoreError> {
        Self::init(Connection::open_in_memory().map_err(err)?)
    }

    fn init(conn: Connection) -> Result<Self, CoreError> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;").map_err(err)?;
        let version: usize = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(err)?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            conn.execute_batch(&format!("BEGIN; {sql} PRAGMA user_version = {}; COMMIT;", i + 1)).map_err(err)?;
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, CoreError> {
        self.conn.lock().map_err(|_| err("database lock poisoned"))
    }
}

impl History for SqliteStore {
    fn begin(&self, started_at_ms: i64) -> Result<RecordingId, CoreError> {
        let c = self.conn()?;
        c.execute("INSERT INTO recordings (started_at, status) VALUES (?1, 'recording')", params![started_at_ms]).map_err(err)?;
        Ok(c.last_insert_rowid())
    }

    fn append_segment(&self, id: RecordingId, text: &str) -> Result<(), CoreError> {
        self.conn()?
            .execute(
                "INSERT INTO segments (recording_id, idx, text)
                 SELECT ?1, COALESCE(MAX(idx) + 1, 0), ?2 FROM segments WHERE recording_id = ?1",
                params![id, text],
            )
            .map_err(err)?;
        Ok(())
    }

    fn complete(&self, id: RecordingId, final_text: &str) -> Result<(), CoreError> {
        self.conn()?
            .execute("UPDATE recordings SET status = 'completed', final_text = ?2 WHERE id = ?1", params![id, final_text])
            .map_err(err)?;
        Ok(())
    }

    fn fail(&self, id: RecordingId, reason: &str) -> Result<(), CoreError> {
        self.conn()?
            .execute("UPDATE recordings SET status = 'failed', error = ?2 WHERE id = ?1", params![id, reason])
            .map_err(err)?;
        Ok(())
    }

    fn delete(&self, id: RecordingId) -> Result<(), CoreError> {
        self.conn()?.execute("DELETE FROM recordings WHERE id = ?1", params![id]).map_err(err)?;
        Ok(())
    }

    fn clear(&self) -> Result<usize, CoreError> {
        self.conn()?.execute("DELETE FROM recordings WHERE status != 'recording'", []).map_err(err)
    }

    fn get(&self, id: RecordingId) -> Result<Option<Entry>, CoreError> {
        let c = self.conn()?;
        Ok(read_entries(&c, "WHERE id = ?1", params![id])?.into_iter().next())
    }

    fn update_text(&self, id: RecordingId, text: &str) -> Result<(), CoreError> {
        self.conn()?.execute("UPDATE recordings SET final_text = ?2 WHERE id = ?1", params![id, text]).map_err(err)?;
        Ok(())
    }

    fn list(&self, limit: u32) -> Result<Vec<Entry>, CoreError> {
        let c = self.conn()?;
        read_entries(&c, "ORDER BY id DESC LIMIT ?1", params![limit])
    }

    fn recover_interrupted(&self) -> Result<usize, CoreError> {
        self.conn()?
            .execute("UPDATE recordings SET status = 'failed', error = 'interrupted' WHERE status = 'recording'", [])
            .map_err(err)
    }
}

fn read_entries(c: &Connection, tail: &str, args: impl rusqlite::Params) -> Result<Vec<Entry>, CoreError> {
    let mut stmt = c
        .prepare(&format!("SELECT id, started_at, status, final_text, error FROM recordings {tail}"))
        .map_err(err)?;
    let rows = stmt
        .query_map(args, |r| {
            Ok(Entry {
                id: r.get(0)?,
                started_at_ms: r.get(1)?,
                status: parse_status(&r.get::<_, String>(2)?),
                segments: vec![],
                final_text: r.get(3)?,
                error: r.get(4)?,
            })
        })
        .map_err(err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)?;

    let mut seg = c.prepare("SELECT text FROM segments WHERE recording_id = ?1 ORDER BY idx").map_err(err)?;
    rows.into_iter()
        .map(|mut e| {
            e.segments = seg.query_map(params![e.id], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
            Ok(e)
        })
        .collect()
}

impl Lexicon for SqliteStore {
    fn words(&self) -> Result<Vec<String>, CoreError> {
        let c = self.conn()?;
        let mut stmt = c.prepare("SELECT word FROM words ORDER BY added_at DESC, rowid DESC").map_err(err)?;
        let rows = stmt.query_map([], |r| r.get(0)).map_err(err)?.collect::<Result<_, _>>().map_err(err)?;
        Ok(rows)
    }

    fn add_word(&self, word: &str) -> Result<(), CoreError> {
        let word = word.trim();
        if word.is_empty() {
            return Ok(());
        }
        // Re-adding bumps it to the front (and takes the new spelling).
        let c = self.conn()?;
        c.execute("DELETE FROM words WHERE word = ?1", params![word]).map_err(err)?;
        c.execute("INSERT INTO words (word) VALUES (?1)", params![word]).map_err(err)?;
        Ok(())
    }

    fn remove_word(&self, word: &str) -> Result<(), CoreError> {
        self.conn()?.execute("DELETE FROM words WHERE word = ?1", params![word]).map_err(err)?;
        Ok(())
    }

    fn fixes(&self) -> Result<Vec<Fix>, CoreError> {
        let c = self.conn()?;
        let mut stmt = c.prepare("SELECT from_text, to_text FROM fixes ORDER BY added_at DESC, rowid DESC").map_err(err)?;
        let rows = stmt
            .query_map([], |r| Ok(Fix { from: r.get(0)?, to: r.get(1)? }))
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)?;
        Ok(rows)
    }

    fn add_fix(&self, fix: &Fix) -> Result<(), CoreError> {
        let c = self.conn()?;
        c.execute("DELETE FROM fixes WHERE from_text = ?1", params![fix.from]).map_err(err)?;
        c.execute("INSERT INTO fixes (from_text, to_text) VALUES (?1, ?2)", params![fix.from, fix.to]).map_err(err)?;
        Ok(())
    }

    fn remove_fix(&self, from: &str) -> Result<(), CoreError> {
        self.conn()?.execute("DELETE FROM fixes WHERE from_text = ?1", params![from]).map_err(err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_orders_segments_and_lists_newest_first() {
        let h = SqliteStore::open_in_memory().unwrap();
        let a = h.begin(1).unwrap();
        h.append_segment(a, "one").unwrap();
        h.append_segment(a, "two").unwrap();
        h.complete(a, "One two.").unwrap();
        let b = h.begin(2).unwrap();
        let rows = h.list(10).unwrap();
        assert_eq!(rows.iter().map(|e| e.id).collect::<Vec<_>>(), vec![b, a]);
        assert_eq!(rows[1].segments, vec!["one", "two"]);
        assert_eq!(rows[1].text(), "One two.");
        assert_eq!(rows[0].status, Status::Recording);
    }

    #[test]
    fn unfinished_recording_still_exposes_its_text() {
        let h = SqliteStore::open_in_memory().unwrap();
        let id = h.begin(1).unwrap();
        h.append_segment(id, "saved before the crash").unwrap();
        assert_eq!(h.recover_interrupted().unwrap(), 1);
        let e = &h.list(10).unwrap()[0];
        assert_eq!((e.status, e.error.as_deref()), (Status::Failed, Some("interrupted")));
        assert_eq!(e.text(), "Saved before the crash.");
    }

    #[test]
    fn delete_removes_segments_too() {
        let h = SqliteStore::open_in_memory().unwrap();
        let id = h.begin(1).unwrap();
        h.append_segment(id, "x").unwrap();
        h.delete(id).unwrap();
        assert!(h.list(10).unwrap().is_empty());
        let n: i64 = h.conn().unwrap().query_row("SELECT COUNT(*) FROM segments", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn clear_removes_finished_entries_but_not_the_one_being_recorded() {
        let h = SqliteStore::open_in_memory().unwrap();
        let done = h.begin(1).unwrap();
        h.append_segment(done, "old").unwrap();
        h.complete(done, "Old.").unwrap();
        let failed = h.begin(2).unwrap();
        h.fail(failed, "x").unwrap();
        let live = h.begin(3).unwrap();
        h.append_segment(live, "still talking").unwrap();
        assert_eq!(h.clear().unwrap(), 2);
        let rows = h.list(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, live);
        h.append_segment(live, "and more").unwrap(); // the live recording keeps working
        assert_eq!(h.list(10).unwrap()[0].segments.len(), 2);
        let n: i64 = h.conn().unwrap().query_row("SELECT COUNT(*) FROM segments WHERE recording_id != ?1", [live], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "segments of cleared entries are gone");
    }

    #[test]
    fn get_and_update_text() {
        let h = SqliteStore::open_in_memory().unwrap();
        let id = h.begin(1).unwrap();
        h.complete(id, "Old.").unwrap();
        h.update_text(id, "New.").unwrap();
        assert_eq!(h.get(id).unwrap().unwrap().text(), "New.");
        assert!(h.get(999).unwrap().is_none());
    }

    #[test]
    fn vocabulary_is_case_insensitive_newest_first_and_removable() {
        let l = SqliteStore::open_in_memory().unwrap();
        l.add_word("Postiz").unwrap();
        l.add_word("Tauri").unwrap();
        l.add_word("postiz").unwrap(); // same word, new spelling, bumped to front
        assert_eq!(l.words().unwrap(), vec!["postiz", "Tauri"]);
        l.remove_word("TAURI").unwrap();
        assert_eq!(l.words().unwrap(), vec!["postiz"]);
    }

    #[test]
    fn fixes_replace_by_source_and_can_be_removed() {
        let l = SqliteStore::open_in_memory().unwrap();
        l.add_fix(&Fix { from: "post is".into(), to: "Postiz".into() }).unwrap();
        l.add_fix(&Fix { from: "post is".into(), to: "PostIz".into() }).unwrap();
        assert_eq!(l.fixes().unwrap(), vec![Fix { from: "post is".into(), to: "PostIz".into() }]);
        l.remove_fix("post is").unwrap();
        assert!(l.fixes().unwrap().is_empty());
    }

    #[test]
    fn existing_v1_database_upgrades_without_losing_history() {
        let dir = std::env::temp_dir().join(format!("vox-store-mig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("h.db");
        {
            // A database as shipped before vocabulary existed: only migration 1 applied.
            let c = Connection::open(&path).unwrap();
            c.execute_batch(&format!("BEGIN; {} PRAGMA user_version = 1; COMMIT;", MIGRATIONS[0])).unwrap();
            c.execute("INSERT INTO recordings (started_at, status, final_text) VALUES (1, 'completed', 'kept.')", []).unwrap();
        }
        let s = SqliteStore::open(&path).unwrap();
        assert_eq!(s.list(10).unwrap()[0].text(), "kept.");
        s.add_word("works").unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn data_survives_reopen_and_migrations_are_idempotent() {
        let dir = std::env::temp_dir().join(format!("vox-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("h.db");
        {
            let h = SqliteStore::open(&path).unwrap();
            let id = h.begin(1).unwrap();
            h.append_segment(id, "persisted").unwrap();
        }
        let h = SqliteStore::open(&path).unwrap();
        assert_eq!(h.list(10).unwrap()[0].segments, vec!["persisted"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
