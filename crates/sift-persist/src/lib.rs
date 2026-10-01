//! SQLite-backed scan snapshot store.
//!
//! One database per scanned volume. Only fully-closed directories are written,
//! so the tables always describe whole, known subtrees — an interrupted scan
//! leaves no partial rows and resumes by matching them on the next walk.
//!
//! Reads and writes use different paths:
//!
//! * **Reads happen on the calling thread.** WAL mode permits any number of
//!   concurrent readers that never block the writer; each worker thread lazily
//!   opens its own read-only connection, so a lookup is one indexed SELECT
//!   instead of a round-trip through the writer thread. Records still buffered
//!   for the writer are not yet visible — that only costs a re-read, never
//!   correctness.
//! * **Writes go to one writer thread.** `record` pushes onto a bounded channel
//!   and the writer drains batches in a single transaction. This keeps the
//!   connection single-owner and the coordinator's critical path syscall-free.
//!
//! ## Schema
//!
//! ```sql
//! CREATE TABLE snapshots (
//!     path      TEXT PRIMARY KEY,
//!     mtime_ms  INTEGER NOT NULL,
//!     logical   INTEGER NOT NULL,
//!     physical  INTEGER NOT NULL,
//!     files     INTEGER NOT NULL
//! );
//! ```

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use sift_scan::{DirSnapshot, SnapshotStore};

/// Flush when either threshold is reached, whichever comes first.
const BATCH_SIZE: usize = 1_000;
const BATCH_INTERVAL: Duration = Duration::from_millis(400);
/// Bound the channel so a stalled disk cannot grow engine memory without
/// limit; records past it are dropped (a miss only costs a re-read).
const CHANNEL_CAP: usize = 1 << 18;

/// Wire message between the store and its writer thread.
enum Wire {
    Record(DirSnapshot),
    Shutdown,
}

/// A durable [`SnapshotStore`]. Dropping it signals the writer to finish.
pub struct SqliteSnapshotStore {
    tx: Option<SyncSender<Wire>>,
    /// Database location, shared with the per-thread read connections.
    db_path: Arc<PathBuf>,
}

impl SqliteSnapshotStore {
    /// Open (creating if needed) the database at `path`.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = Arc::new(path.as_ref().to_path_buf());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let (tx, rx) = mpsc::sync_channel::<Wire>(CHANNEL_CAP);
        let writer_path = Arc::clone(&path);
        std::thread::Builder::new()
            .name("sift-snapshot-writer".into())
            .spawn(move || {
                if let Err(err) = run_writer(&writer_path, rx) {
                    eprintln!("sift snapshot writer exited: {err}");
                }
            })
            .map_err(|err| std::io::Error::other(err.to_string()))?;

        Ok(Self {
            tx: Some(tx),
            db_path: path,
        })
    }

    /// Run `f` against this thread's read-only connection, opened lazily and
    /// reused for every later lookup on this thread.
    fn with_read_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> T,
    ) -> Option<T> {
        thread_local! {
            static READ_CONN: RefCell<Option<(PathBuf, Connection)>> = const { RefCell::new(None) };
        }
        READ_CONN.with(|slot| {
            let mut slot = slot.borrow_mut();
            let needs_open = match slot.as_ref() {
                Some((path, _)) => path != self.db_path.as_ref(),
                None => true,
            };
            if needs_open {
                *slot = open_read_only(&self.db_path).map(|conn| ((*self.db_path).clone(), conn));
            }
            slot.as_ref().map(|(_, conn)| f(conn))
        })
    }
}

fn open_read_only(path: &Path) -> Option<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _ = conn.busy_timeout(Duration::from_secs(2));
    let _ = conn.pragma_update(None, "query_only", true);
    let _ = conn.pragma_update(None, "temp_store", "MEMORY");
    Some(conn)
}

impl Drop for SqliteSnapshotStore {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            // Best effort: do not block shutdown for the flush.
            let _ = tx.try_send(Wire::Shutdown);
        }
    }
}

impl SnapshotStore for SqliteSnapshotStore {
    fn lookup(&self, path: &Path, mtime_ms: i64) -> Option<DirSnapshot> {
        self.with_read_connection(|conn| query(conn, path, mtime_ms))
            .flatten()
    }

    fn record(&self, snapshot: DirSnapshot) {
        if let Some(tx) = self.tx.as_ref() {
            // Non-blocking: drop under backpressure rather than stall the scan.
            let _ = tx.try_send(Wire::Record(snapshot));
        }
    }
}

// ---- writer thread ---------------------------------------------------------

fn run_writer(path: &Path, rx: Receiver<Wire>) -> rusqlite::Result<()> {
    let mut conn = Connection::open(path)?;
    configure(&conn)?;
    init_schema(&conn)?;

    let mut batch: Vec<DirSnapshot> = Vec::with_capacity(BATCH_SIZE);
    loop {
        match rx.recv_timeout(BATCH_INTERVAL) {
            Ok(Wire::Record(snapshot)) => {
                batch.push(snapshot);
                if batch.len() >= BATCH_SIZE {
                    flush(&mut conn, &mut batch);
                }
            }
            Ok(Wire::Shutdown) => {
                flush(&mut conn, &mut batch);
                // Checkpoint WAL back into the main file for tidy on-disk state.
                let _ = conn.pragma_update(None, "wal_checkpoint", "TRUNCATE");
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => flush(&mut conn, &mut batch),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                flush(&mut conn, &mut batch);
                break;
            }
        }
    }
    Ok(())
}

fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "mmap_size", 268_435_456i64)?;
    conn.pragma_update(None, "cache_size", -65_536i64)?; // 64 MiB
    Ok(())
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);\n\
         CREATE TABLE IF NOT EXISTS snapshots (\n\
             path      TEXT PRIMARY KEY,\n\
             mtime_ms  INTEGER NOT NULL,\n\
             logical   INTEGER NOT NULL,\n\
             physical  INTEGER NOT NULL,\n\
             files     INTEGER NOT NULL\n\
         );\n\
         CREATE INDEX IF NOT EXISTS idx_snapshots_mtime ON snapshots(mtime_ms);",
    )?;
    conn.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', 1)\n\
         ON CONFLICT(key) DO NOTHING",
        [],
    )?;
    Ok(())
}

/// Write one batch upsert in a single transaction.
fn flush(conn: &mut Connection, batch: &mut Vec<DirSnapshot>) {
    if batch.is_empty() {
        return;
    }
    let result = conn.transaction().and_then(|tx| {
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO snapshots(path, mtime_ms, logical, physical, files)\n\
                 VALUES (?1, ?2, ?3, ?4, ?5)\n\
                 ON CONFLICT(path) DO UPDATE SET\n\
                     mtime_ms = excluded.mtime_ms,\n\
                     logical  = excluded.logical,\n\
                     physical = excluded.physical,\n\
                     files    = excluded.files",
            )?;
            for snapshot in batch.iter() {
                stmt.execute((
                    snapshot.path.as_os_str().to_string_lossy().as_ref(),
                    snapshot.mtime_ms,
                    snapshot.logical,
                    snapshot.physical,
                    snapshot.files,
                ))?;
            }
        }
        tx.commit()
    });
    if let Err(err) = result {
        eprintln!("sift snapshot batch flush failed: {err}");
    }
    batch.clear();
}

fn query(conn: &Connection, path: &Path, mtime_ms: i64) -> Option<DirSnapshot> {
    conn.query_row(
        "SELECT path, mtime_ms, logical, physical, files\n\
         FROM snapshots WHERE path = ?1 AND mtime_ms = ?2",
        (path.as_os_str().to_string_lossy().as_ref(), mtime_ms),
        |row| {
            let stored_path: String = row.get(0)?;
            Ok(DirSnapshot::new(
                stored_path,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sift-persist-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn opens_and_round_trips_a_snapshot() {
        let dir = unique_dir("roundtrip");
        let store = SqliteSnapshotStore::open(dir.join("scan.db")).unwrap();
        let snap = DirSnapshot::new("/some/path", 123_456, 4096, 8192, 7);
        store.record(snap.clone());
        // The writer flushes within BATCH_INTERVAL.
        std::thread::sleep(BATCH_INTERVAL + Duration::from_millis(200));
        assert_eq!(store.lookup(Path::new("/some/path"), 123_456), Some(snap));
        assert_eq!(store.lookup(Path::new("/some/path"), 999), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn lookup_can_miss_records_still_buffered() {
        // Direct read connections do not see unflushed writer batches: this is
        // the documented tradeoff (a miss only costs a re-read).
        let dir = unique_dir("buffered");
        let store = SqliteSnapshotStore::open(dir.join("scan.db")).unwrap();
        let snap = DirSnapshot::new("/buffered/path", 555, 1, 2, 3);
        store.record(snap);
        // Immediately: still buffered, miss. After the interval: visible.
        std::thread::sleep(BATCH_INTERVAL + Duration::from_millis(200));
        assert_eq!(
            store.lookup(Path::new("/buffered/path"), 555).map(|s| s.files),
            Some(3)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_survives_reopen() {
        let dir = unique_dir("reopen");
        let db = dir.join("scan.db");
        {
            let store = SqliteSnapshotStore::open(&db).unwrap();
            store.record(DirSnapshot::new("/persisted", 42, 99, 88, 1));
        }
        // Drop triggers shutdown + flush + checkpoint.
        std::thread::sleep(Duration::from_millis(300));
        let store = SqliteSnapshotStore::open(&db).unwrap();
        assert!(store.lookup(Path::new("/persisted"), 42).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
