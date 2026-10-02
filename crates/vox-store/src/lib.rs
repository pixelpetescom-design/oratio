//! SQLite adapter for the `History` port. WAL mode, so a crash mid-recording
//! leaves every already-written segment intact.
#![cfg_attr(test, allow(clippy::unwrap_used))]

use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use vox_core::history::{Entry, History, RecordingId, Status};
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
"];

pub struct SqliteHistory {
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

impl SqliteHistory {
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

impl History for SqliteHistory {
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

    fn list(&self, limit: u32) -> Result<Vec<Entry>, CoreError> {
        let c = self.conn()?;
        let mut stmt = c
            .prepare("SELECT id, started_at, status, final_text, error FROM recordings ORDER BY id DESC LIMIT ?1")
            .map_err(err)?;
        let rows = stmt
            .query_map(params![limit], |r| {
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

    fn recover_interrupted(&self) -> Result<usize, CoreError> {
        self.conn()?
            .execute("UPDATE recordings SET status = 'failed', error = 'interrupted' WHERE status = 'recording'", [])
            .map_err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_orders_segments_and_lists_newest_first() {
        let h = SqliteHistory::open_in_memory().unwrap();
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
        let h = SqliteHistory::open_in_memory().unwrap();
        let id = h.begin(1).unwrap();
        h.append_segment(id, "saved before the crash").unwrap();
        assert_eq!(h.recover_interrupted().unwrap(), 1);
        let e = &h.list(10).unwrap()[0];
        assert_eq!((e.status, e.error.as_deref()), (Status::Failed, Some("interrupted")));
        assert_eq!(e.text(), "Saved before the crash.");
    }

    #[test]
    fn delete_removes_segments_too() {
        let h = SqliteHistory::open_in_memory().unwrap();
        let id = h.begin(1).unwrap();
        h.append_segment(id, "x").unwrap();
        h.delete(id).unwrap();
        assert!(h.list(10).unwrap().is_empty());
        let n: i64 = h.conn().unwrap().query_row("SELECT COUNT(*) FROM segments", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn data_survives_reopen_and_migrations_are_idempotent() {
        let dir = std::env::temp_dir().join(format!("vox-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("h.db");
        {
            let h = SqliteHistory::open(&path).unwrap();
            let id = h.begin(1).unwrap();
            h.append_segment(id, "persisted").unwrap();
        }
        let h = SqliteHistory::open(&path).unwrap();
        assert_eq!(h.list(10).unwrap()[0].segments, vec!["persisted"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
