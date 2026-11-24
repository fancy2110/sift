//! Audit a saved Store tree snapshot.
//!
//! cargo run -p sift-store --release --example tree_validate -- \
//!   "$HOME/Library/Application Support/Sift/tree.snapshot.gz"
//!
//! Exits non-zero when a parent total differs from its direct-child sum or a
//! listing disagrees with the recorded tree edges.

use std::path::PathBuf;

use sift_store::FolderIndex;

fn main() {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_default()
                .join("Sift")
                .join("tree.snapshot.gz")
        });
    let index = FolderIndex::default();
    match index.restore_from(&path) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("no snapshot at {}", path.display());
            std::process::exit(2);
        }
        Err(error) => {
            eprintln!("restore failed: {error}");
            std::process::exit(2);
        }
    }
    let report = index.validate();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if !report.is_consistent() {
        std::process::exit(1);
    }
}
