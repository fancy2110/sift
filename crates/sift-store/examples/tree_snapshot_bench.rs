//! Synthetic benchmark for the Store tree/listings snapshot.
//!
//! Run with: cargo run -p sift-store --release --example tree_snapshot_bench
//! It builds a 20k-directory / 200k-entry index, measures cold snapshot,
//! restore, list and size queries, and prints the compressed snapshot size.

use std::time::Instant;

use sift_store::{FolderEntry, FolderIndex};

fn main() {
    const DIRS: usize = 20_000;
    const ENTRIES: usize = 10;

    let index = FolderIndex::default();
    let start = Instant::now();
    for d in 0..DIRS {
        let parent = format!("/bench/d{d:06}");
        let entries = (0..ENTRIES)
            .map(|e| FolderEntry {
                name: format!("f{e:02}.log"),
                path: format!("{parent}/f{e:02}.log"),
                is_dir: false,
                size: (e as u64 + 1) * 4096,
                modified_ms: Some(1_700_000_000_000),
            })
            .collect();
        index.apply(&parent, &parent, entries);
        index.apply_size(&parent, (ENTRIES as u64) * 4096, true);
    }
    println!(
        "build: {:?} for {} dirs / {} entries",
        start.elapsed(),
        DIRS,
        DIRS * ENTRIES
    );

    let start = Instant::now();
    let bytes = index.snapshot_bytes().unwrap();
    println!(
        "snapshot: {:?}, {} bytes ({:.1} MiB)",
        start.elapsed(),
        bytes.len(),
        bytes.len() as f64 / (1024.0 * 1024.0)
    );

    let restored = FolderIndex::default();
    let start = Instant::now();
    restored.restore_bytes(&bytes).unwrap();
    println!("restore: {:?}", start.elapsed());

    let start = Instant::now();
    for d in 0..100 {
        let path = format!("/bench/d{d:06}");
        let _ = restored.list(&path);
        let _ = restored.size(&path);
    }
    println!("200 warm list+size queries: {:?}", start.elapsed());
}
