//! End-to-end resume test for snapshot-backed scanning.
//!
//! A deep tree is built, the first scan is cancelled partway (simulating a
//! crash), and a second scan must still arrive at the exact full byte/file
//! totals — reusing the fully-closed subtrees that landed in SQLite and
//! re-reading whatever never closed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sift_core::scan::{ScanOutcome, ScanPolicy, ScanRequest};
use sift_persist::SqliteSnapshotStore;
use sift_scan::ScanEngine;

fn unique_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "sift-resume-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// Build a bushy tree of small files: `branches` top-level dirs, each with
/// `depth` levels of nesting, `files_per_dir` files at every level.
fn build_tree(root: &Path, branches: u32, depth: u32, files_per_dir: u32) -> u64 {
    let mut written = 0u64;
    for branch in 0..branches {
        let mut dir = root.to_path_buf();
        for level in 0..depth {
            dir = dir.join(format!("b{branch}")).join(format!("l{level}"));
            std::fs::create_dir_all(&dir).unwrap();
            for file in 0..files_per_dir {
                let path = dir.join(format!("f{file}.bin"));
                std::fs::write(&path, vec![7u8; 4096]).unwrap();
                written += 4096;
            }
        }
    }
    written
}

/// Run a scan to completion and return the root node's totals.
fn run_scan(
    root: &Path,
    store: Arc<SqliteSnapshotStore>,
    workers: usize,
) -> (ScanOutcome, u64, u32) {
    let engine = ScanEngine::with_workers(workers).with_snapshot_store(store);
    let policy = ScanPolicy::interactive();
    let request = ScanRequest::new(sift_core::ScanId(1), root).with_policy(policy);
    let handle = engine.scan(request).unwrap();
    let outcome = handle.join();
    let tree = handle.tree.lock().unwrap();
    let node = tree.node(tree.root()).unwrap();
    (outcome, node.size.logical, node.file_count)
}

#[test]
fn cancelled_scan_resumes_to_exact_totals() {
    let root = unique_root("cancel");
    let total_bytes = build_tree(&root, 6, 5, 4);
    let db = root.parent().unwrap().join("resume-scan.db");

    // First scan, aborted almost immediately.
    let store1 = Arc::new(SqliteSnapshotStore::open(&db).unwrap());
    let engine1 = ScanEngine::with_workers(1).with_snapshot_store(store1.clone());
    let request = ScanRequest::new(sift_core::ScanId(1), &root)
        .with_policy(ScanPolicy::interactive());
    let handle1 = engine1.scan(request).unwrap();
    handle1.cancel();
    assert_eq!(handle1.join(), ScanOutcome::Cancelled);
    // Let the writer settle the in-flight batch.
    drop(store1);
    std::thread::sleep(Duration::from_millis(500));

    // Second scan completes, reusing whatever fully closed and re-reading
    // the rest; totals must match exactly.
    let store2 = Arc::new(SqliteSnapshotStore::open(&db).unwrap());
    let (outcome, logical, files) = run_scan(&root, store2, 2);
    assert_eq!(outcome, ScanOutcome::Completed);
    assert_eq!(logical, total_bytes, "resumed scan must report exact bytes");
    assert_eq!(files, 6 * 5 * 4, "resumed scan must report every file");

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_file(&db);
}
