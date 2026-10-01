//! SQLite implementation of [`ScanJournal`].
//!
//! Writes leave the scan hot path immediately: the coordinator hands records to
//! one background writer that owns the connection, so directory discovery never
//! waits on fsync. `load` is synchronous through request/reply so it observes
//! committed data. WAL keeps writer and reader from contending.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};

use sift_scan::{ClosedDir, DiscoveredDir, RestoredDir, RestoredScan, ScanJournal};

/// A durable, SQLite-backed scan journal. Cheap to clone (state is shared).
pub struct SqliteScanJournal {
    cmd: Sender<WriterMsg>,
}

enum WriterMsg {
    Begin(DiscoveredDir),
    Discovered { root: String, dirs: Vec<DiscoveredDir> },
    Closed { root: String, updates: Vec<ClosedDir> },
    Load { root: String, reply: Sender<RestoredScan> },
    Complete(String),
    Shutdown,
}

impl SqliteScanJournal {
    /// Open (or create) the journal database at `path`.
    pub fn open(path: impl Into<PathBuf>) -> rusqlite::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = rusqlite::Connection::open(path)?;
        Self::with_conn(conn)
    }

    /// Create an in-memory journal for tests.
    pub fn memory() -> rusqlite::Result<Self> {
        Self::with_conn(rusqlite::Connection::open_in_memory()?)
    }

    fn with_conn(mut conn: rusqlite::Connection) -> rusqlite::Result<Self> {
        init_schema(&mut conn)?;
        let (cmd, rx) = channel::<WriterMsg>();
        std::thread::Builder::new()
            .name("sift-journal".into())
            .spawn(move || {
                while let Ok(msg) = rx.recv() {
                    match msg {
                        WriterMsg::Begin(root) => {
                            let _ = apply_begin(&conn, &root);
                        }
                        WriterMsg::Discovered { root, dirs } => {
                            let _ = apply_discovered(&conn, &root, dirs);
                        }
                        WriterMsg::Closed { root, updates } => {
                            let _ = apply_closed(&conn, &root, updates);
                        }
                        WriterMsg::Load { root, reply } => {
                            let scan = load_scan(&conn, &root).unwrap_or_default();
                            let _ = reply.send(scan);
                        }
                        WriterMsg::Complete(root) => {
                            let _ = apply_complete(&conn, &root);
                        }
                        WriterMsg::Shutdown => break,
                    }
                }
            })
            .expect("spawn journal writer");
        Ok(Self { cmd })
    }
}

impl Drop for SqliteScanJournal {
    fn drop(&mut self) {
        let _ = self.cmd.send(WriterMsg::Shutdown);
    }
}

fn init_schema(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;\n\
         PRAGMA synchronous=NORMAL;\n\
         CREATE TABLE IF NOT EXISTS scans (\n\
             root TEXT PRIMARY KEY,\n\
             running INTEGER NOT NULL DEFAULT 1\n\
         );\n\
         CREATE TABLE IF NOT EXISTS dirs (\n\
             root TEXT NOT NULL,\n\
             key TEXT NOT NULL,\n\
             parent_key TEXT NOT NULL,\n\
             name BLOB NOT NULL,\n\
             path TEXT NOT NULL,\n\
             depth INTEGER NOT NULL,\n\
             mtime_ms INTEGER NOT NULL,\n\
             dev INTEGER NOT NULL,\n\
             ino INTEGER NOT NULL,\n\
             is_symlink INTEGER NOT NULL,\n\
             deletable INTEGER NOT NULL,\n\
             closed INTEGER NOT NULL DEFAULT 0,\n\
             logical INTEGER NOT NULL DEFAULT 0,\n\
             physical INTEGER NOT NULL DEFAULT 0,\n\
             files INTEGER NOT NULL DEFAULT 0,\n\
             PRIMARY KEY (root, key)\n\
         );\n\
         CREATE INDEX IF NOT EXISTS dirs_root_depth ON dirs(root, depth);",
    )
}

fn apply_begin(conn: &rusqlite::Connection, root: &DiscoveredDir) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO scans(root, running) VALUES (?1, 1)\n\
         ON CONFLICT(root) DO UPDATE SET running=1",
        rusqlite::params![root.path],
    )?;
    upsert_discovered(conn, &root.path, root)?;
    Ok(())
}

fn upsert_discovered(
    conn: &rusqlite::Connection,
    scan_root: &str,
    d: &DiscoveredDir,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO dirs\n\
         (root, key, parent_key, name, path, depth, mtime_ms, dev, ino,\n\
          is_symlink, deletable, closed)\n\
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0)\n\
         ON CONFLICT(root, key) DO UPDATE SET\n\
          parent_key=excluded.parent_key, name=excluded.name, path=excluded.path,\n\
          depth=excluded.depth, mtime_ms=excluded.mtime_ms, dev=excluded.dev,\n\
          ino=excluded.ino, is_symlink=excluded.is_symlink,\n\
          deletable=excluded.deletable",
        rusqlite::params![
            scan_root,
            d.key,
            d.parent_key,
            d.name,
            d.path,
            d.depth,
            d.mtime_ms,
            d.dev as i64,
            d.ino as i64,
            d.is_symlink as i64,
            d.deletable as i64,
        ],
    )?;
    Ok(())
}

fn apply_discovered(
    conn: &rusqlite::Connection,
    root: &str,
    dirs: Vec<DiscoveredDir>,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for d in &dirs {
        upsert_discovered(&tx, root, d)?;
    }
    tx.commit()
}

fn apply_closed(
    conn: &rusqlite::Connection,
    root: &str,
    updates: Vec<ClosedDir>,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for c in &updates {
        tx.execute(
            "UPDATE dirs SET closed=1, logical=?3, physical=?4, files=?5\n\
             WHERE root=?1 AND key=?2",
            rusqlite::params![root, c.key, c.logical as i64, c.physical as i64, c.files as i64],
        )?;
    }
    tx.commit()
}

fn apply_complete(conn: &rusqlite::Connection, root: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM dirs WHERE root=?1", rusqlite::params![root])?;
    conn.execute("DELETE FROM scans WHERE root=?1", rusqlite::params![root])?;
    Ok(())
}

fn load_scan(conn: &rusqlite::Connection, root: &str) -> rusqlite::Result<RestoredScan> {
    let running: Option<i64> = conn
        .query_row(
            "SELECT running FROM scans WHERE root=?1",
            rusqlite::params![root],
            |row| row.get(0),
        )
        .ok();
    if running == Some(0) {
        return Ok(RestoredScan::default());
    }

    let mut stmt = conn.prepare(
        "SELECT key, parent_key, name, path, depth, mtime_ms, dev, ino,\n\
         is_symlink, deletable, closed, logical, physical, files\n\
         FROM dirs WHERE root=?1 ORDER BY depth ASC, rowid ASC",
    )?;
    let rows = stmt.query_map(rusqlite::params![root], |row| {
        let name: Vec<u8> = row.get(2)?;
        let discovered = DiscoveredDir::new(
            row.get(0)?,
            row.get(1)?,
            name,
            row.get(3)?,
            row.get::<_, i64>(4)? as u16,
            row.get(5)?,
            row.get::<_, i64>(6)? as u64,
            row.get::<_, i64>(7)? as u64,
            row.get::<_, i64>(8)? != 0,
            row.get::<_, i64>(9)? != 0,
        );
        Ok(RestoredDir::new(
            discovered,
            row.get::<_, i64>(10)? != 0,
            row.get::<_, i64>(11)? as u64,
            row.get::<_, i64>(12)? as u64,
            row.get::<_, i64>(13)? as u32,
        ))
    })?;

    let mut dirs = Vec::new();
    for row in rows {
        dirs.push(row?);
    }
    Ok(RestoredScan::new(dirs))
}

impl ScanJournal for SqliteScanJournal {
    fn begin(&self, root: &DiscoveredDir) {
        let _ = self.cmd.send(WriterMsg::Begin(root.clone()));
    }

    fn discovered(&self, scan_root: &str, dirs: Vec<DiscoveredDir>) {
        if dirs.is_empty() {
            return;
        }
        let _ = self.cmd.send(WriterMsg::Discovered {
            root: scan_root.to_string(),
            dirs,
        });
    }

    fn closed(&self, scan_root: &str, updates: Vec<ClosedDir>) {
        if updates.is_empty() {
            return;
        }
        let _ = self.cmd.send(WriterMsg::Closed {
            root: scan_root.to_string(),
            updates,
        });
    }

    fn load(&self, root: &str) -> RestoredScan {
        let (reply, rx) = channel();
        if self
            .cmd
            .send(WriterMsg::Load { root: root.to_string(), reply })
            .is_err()
        {
            return RestoredScan::default();
        }
        rx.recv().unwrap_or_default()
    }

    fn complete(&self, root: &str) {
        let _ = self.cmd.send(WriterMsg::Complete(root.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(key: &str, parent: &str, name: &str, path: &str, depth: u16) -> DiscoveredDir {
        DiscoveredDir::new(
            key.into(),
            parent.into(),
            name.as_bytes().to_vec(),
            path.into(),
            depth,
            0,
            1,
            depth as u64 + 1,
            false,
            false,
        )
    }

    #[test]
    fn nothing_restoreable_initially() {
        let journal = SqliteScanJournal::memory().unwrap();
        assert!(journal.load("/root").is_empty());
    }

    #[test]
    fn interrupted_frontier_is_restored() {
        let journal = SqliteScanJournal::memory().unwrap();
        let scan_root = "/root";
        let root = dir("R", "", "root", scan_root, 0);
        journal.begin(&root);
        let a = dir("A", "R", "a", "/root/a", 1);
        let b = dir("B", "R", "b", "/root/b", 1);
        journal.discovered(scan_root, vec![a.clone(), b.clone()]);
        // `a` fully closed; `b` discovered but not closed.
        journal.closed(
            scan_root,
            vec![ClosedDir::new("A".into(), 100, 200, 4)],
        );

        let restored = journal.load(scan_root);
        assert_eq!(restored.dirs.len(), 3, "root, a, b");
        let by = |k: &str| restored.dirs.iter().find(|d| d.discovered.key == k).unwrap();
        assert!(by("A").closed);
        assert_eq!(by("A").logical, 100);
        assert!(!by("B").closed, "b stays in the unfinished frontier");
    }

    #[test]
    fn completion_clears_resume_state() {
        let journal = SqliteScanJournal::memory().unwrap();
        let root = dir("R", "", "root", "/root", 0);
        journal.begin(&root);
        journal.discovered("/root", vec![dir("A", "R", "a", "/root/a", 1)]);
        journal.complete("/root");
        assert!(journal.load("/root").is_empty());
    }
}
