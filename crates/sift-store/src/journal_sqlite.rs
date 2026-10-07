//! SQLite implementation of [`ScanJournal`].
//!
//! Writes never touch the scan hot path: the coordinator hands records to an
//! unbounded queue and one background writer owns the connection, so directory
//! discovery never waits on fsync. The writer drains everything queued and
//! applies it in a single transaction, which keeps commit/fsync frequency flat
//! no matter how fast the scan discovers directories. `load` is synchronous
//! through request/reply, so it observes every earlier write. WAL keeps writer
//! and reader from contending.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use sift_scan::{ClosedDir, DiscoveredDir, RestoredDir, RestoredScan, ScanJournal};

/// Maximum messages folded into one writer transaction. A cap bounds the work
/// of a single commit; further messages go into the next drain immediately.
const MAX_DRAIN: usize = 4096;

/// A durable, SQLite-backed scan journal. Cheap to clone (state is shared).
pub struct SqliteScanJournal {
    cmd: Sender<WriterMsg>,
    stats: Arc<WriterStats>,
}

/// Write-failure state shared with the background writer. Failures are never
/// silent: each one is logged, counted, and the last message is retained.
#[derive(Default)]
struct WriterStats {
    failed_writes: AtomicU64,
    last_error: Mutex<Option<String>>,
}

enum WriterMsg {
    Begin(DiscoveredDir),
    Discovered { root: String, dirs: Vec<DiscoveredDir> },
    Closed { root: String, updates: Vec<ClosedDir> },
    Load { root: String, reply: Sender<RestoredScan> },
    PruneOpen(String),
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

    fn with_conn(conn: rusqlite::Connection) -> rusqlite::Result<Self> {
        init_schema(&conn)?;
        let (cmd, rx) = channel::<WriterMsg>();
        let stats = Arc::<WriterStats>::default();
        let writer_stats = Arc::clone(&stats);
        std::thread::Builder::new()
            .name("sift-journal".into())
            .spawn(move || writer_loop(&conn, &rx, &writer_stats))
            .expect("spawn journal writer");
        Ok(Self { cmd, stats })
    }

    /// Number of background write failures since opening. A non-zero count
    /// means some resume data is missing and the disk may be failing.
    pub fn failed_write_count(&self) -> u64 {
        self.stats.failed_writes.load(Ordering::SeqCst)
    }

    /// The most recent background write error message, if any.
    pub fn last_write_error(&self) -> Option<String> {
        self.stats
            .last_error
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }
}

/// Own the connection until every sender disconnects (or a shutdown arrives).
/// Each pass applies all currently queued messages in one transaction.
fn writer_loop(conn: &rusqlite::Connection, rx: &Receiver<WriterMsg>, stats: &WriterStats) {
    while let Ok(first) = rx.recv() {
        let mut batch = Vec::with_capacity(MAX_DRAIN.min(64));
        batch.push(first);
        while batch.len() < MAX_DRAIN {
            match rx.try_recv() {
                Ok(msg) => batch.push(msg),
                Err(_) => break,
            }
        }
        if apply_batch(conn, batch, stats) {
            break;
        }
    }
}

/// Apply one drained batch. Returns true when the writer should exit.
fn apply_batch(conn: &rusqlite::Connection, msgs: Vec<WriterMsg>, stats: &WriterStats) -> bool {
    let mut tx = match conn.unchecked_transaction() {
        Ok(tx) => tx,
        // Without a transaction the records cannot be applied; count one
        // failure per batch rather than per message and let the scan continue.
        Err(err) => {
            record_write_error(stats, "begin transaction", err);
            return false;
        }
    };

    let mut shutdown = false;
    for msg in msgs {
        match msg {
            WriterMsg::Begin(root) => {
                if let Err(err) = tx.execute(
                    "INSERT INTO scans(root, running) VALUES (?1, 1)\n\
                     ON CONFLICT(root) DO UPDATE SET running=1",
                    rusqlite::params![root.path],
                ) {
                    record_write_error(stats, "begin", err);
                }
                if let Err(err) = upsert_discovered(&tx, &root.path, &root) {
                    record_write_error(stats, "begin", err);
                }
            }
            WriterMsg::Discovered { root, dirs } => {
                for d in &dirs {
                    if let Err(err) = upsert_discovered(&tx, &root, d) {
                        record_write_error(stats, "discovered", err);
                    }
                }
            }
            WriterMsg::Closed { root, updates } => {
                for c in &updates {
                    if let Err(err) = tx.execute(
                        "UPDATE dirs SET closed=1, logical=?3, physical=?4, files=?5\n\
                         WHERE root=?1 AND key=?2",
                        rusqlite::params![
                            root,
                            c.key,
                            c.logical as i64,
                            c.physical as i64,
                            c.files as i64
                        ],
                    ) {
                        record_write_error(stats, "closed", err);
                    }
                }
            }
            // A load must observe every write that preceded it: commit the
            // in-flight transaction, answer, then reopen for later messages.
            WriterMsg::Load { root, reply } => {
                if let Err(err) = tx.commit() {
                    record_write_error(stats, "commit before load", err);
                }
                let scan = load_scan(conn, &root).unwrap_or_default();
                let _ = reply.send(scan);
                tx = match conn.unchecked_transaction() {
                    Ok(tx) => tx,
                    Err(err) => {
                        record_write_error(stats, "reopen transaction", err);
                        return false;
                    }
                };
            }
            WriterMsg::PruneOpen(root) => {
                if let Err(err) = tx.execute(
                    "DELETE FROM dirs WHERE root=?1 AND closed=0",
                    rusqlite::params![root],
                ) {
                    record_write_error(stats, "prune_open", err);
                }
            }
            WriterMsg::Complete(root) => {
                if let Err(err) =
                    tx.execute("DELETE FROM dirs WHERE root=?1", rusqlite::params![root])
                {
                    record_write_error(stats, "complete", err);
                }
                if let Err(err) =
                    tx.execute("DELETE FROM scans WHERE root=?1", rusqlite::params![root])
                {
                    record_write_error(stats, "complete", err);
                }
            }
            WriterMsg::Shutdown => shutdown = true,
        }
    }

    if let Err(err) = tx.commit() {
        record_write_error(stats, "commit", err);
    }
    shutdown
}

/// Log and record one background write failure.
fn record_write_error(stats: &WriterStats, stage: &str, err: rusqlite::Error) {
    eprintln!("sift journal: write failed during {stage}: {err}");
    stats.failed_writes.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut last) = stats.last_error.lock() {
        *last = Some(err.to_string());
    }
}

impl Drop for SqliteScanJournal {
    fn drop(&mut self) {
        // The queue is unbounded, so send never blocks. The writer applies all
        // messages ahead of the shutdown and exits immediately afterwards.
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

fn upsert_discovered(
    conn: &rusqlite::Connection,
    scan_root: &str,
    d: &DiscoveredDir,
) -> rusqlite::Result<()> {
    // Cached across drain transactions: with millions of directories this
    // avoids recompiling the statement for every row.
    let mut stmt = conn.prepare_cached(
        "INSERT INTO dirs\n\
         (root, key, parent_key, name, path, depth, mtime_ms, dev, ino,\n\
          is_symlink, deletable, closed)\n\
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0)\n\
         ON CONFLICT(root, key) DO UPDATE SET\n\
          parent_key=excluded.parent_key, name=excluded.name, path=excluded.path,\n\
          depth=excluded.depth, mtime_ms=excluded.mtime_ms, dev=excluded.dev,\n\
          ino=excluded.ino, is_symlink=excluded.is_symlink,\n\
          deletable=excluded.deletable",
    )?;
    stmt.execute(rusqlite::params![
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
    ])?;
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

    fn prune_open(&self, root: &str) {
        let _ = self.cmd.send(WriterMsg::PruneOpen(root.to_string()));
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

    /// Thousands of records queued back-to-back must all survive: this is the
    /// case the old bounded channel blocked on, freezing the coordinator.
    #[test]
    fn a_burst_of_records_is_fully_persisted() {
        let journal = SqliteScanJournal::memory().unwrap();
        let scan_root = "/root";
        journal.begin(&dir("R", "", "root", scan_root, 0));

        const BURST: usize = 10_000;
        let chunk: Vec<DiscoveredDir> = (0..BURST)
            .map(|i| {
                dir(
                    &format!("D{i}"),
                    "R",
                    &format!("d{i}"),
                    &format!("/root/d{i}"),
                    1,
                )
            })
            .collect();
        journal.discovered(scan_root, chunk);

        let restored = journal.load(scan_root);
        assert_eq!(restored.dirs.len(), BURST + 1, "root + every bursted dir");
        assert!(journal.failed_write_count() == 0, "no write failures");
    }

    /// A write failure in the background writer must be observable: logged,
    /// counted exactly once, and its message retained.
    #[test]
    fn write_failures_are_counted_and_retained() {
        let scratch = std::env::temp_dir().join(format!(
            "sift-journal-err-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&scratch).unwrap();
        let db = scratch.join("journal.db");
        let journal = SqliteScanJournal::open(&db).unwrap();

        // A second connection removes the table the writer inserts into.
        let saboteur = rusqlite::Connection::open(&db).unwrap();
        saboteur.execute_batch("DROP TABLE dirs").unwrap();

        // The scans-row insert succeeds; the dirs insert fails: one error.
        journal.begin(&dir("R", "", "root", "/root", 0));

        let mut observed = false;
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if journal.failed_write_count() > 0 {
                observed = true;
                break;
            }
        }
        assert!(observed, "the failed write must be counted");
        assert_eq!(journal.failed_write_count(), 1);
        assert!(
            journal.last_write_error().is_some(),
            "the error message must be retained"
        );

        drop(saboteur);
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
