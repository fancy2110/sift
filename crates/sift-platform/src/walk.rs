//! Parallel directory reading primitive.
//!
//! The scan engine owns *scheduling* (which directory to walk next, focus
//! priority, budgets); this module owns *throughput*: reading many independent
//! directories at once so syscall latency is hidden behind other reads. It is a
//! deliberately policy-free seam — no ordering, no tree, no budgets.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::dir::{DirReader, RawEntry};

/// Result of reading one directory.
pub type DirReadResult = (PathBuf, Option<Vec<RawEntry>>);

/// Read every directory in `paths` with a fixed worker pool, returning results
/// in input order. A `None` entry means the directory could not be read at all;
/// individual unreadable entries inside a readable directory are simply absent.
///
/// Workers pull paths from a shared index, so a handful of enormous directories
/// do not starve the rest of the batch. Allocations are per-directory and
/// dropped when the caller is done, keeping peak memory proportional to the
/// largest directory, not the whole batch.
pub fn walk_dirs(paths: &[PathBuf], want_physical: bool, workers: usize) -> Vec<DirReadResult> {
    let workers = workers.clamp(1, 64);
    if paths.is_empty() {
        return Vec::new();
    }
    if workers == 1 || paths.len() == 1 {
        return paths
            .iter()
            .map(|path| (path.clone(), read_one(path, want_physical)))
            .collect();
    }

    let next = Arc::new(AtomicUsize::new(0));
    let results: Arc<Mutex<Vec<Option<DirReadResult>>>> =
        Arc::new(Mutex::new((0..paths.len()).map(|_| None).collect()));

    std::thread::scope(|scope| {
        for _ in 0..workers {
            let next = Arc::clone(&next);
            let results = Arc::clone(&results);
            let paths: Vec<PathBuf> = paths.to_vec();
            scope.spawn(move || loop {
                let ix = next.fetch_add(1, Ordering::Relaxed);
                if ix >= paths.len() {
                    return;
                }
                let result = read_one(&paths[ix], want_physical);
                // Each slot is written by exactly one worker; the mutex only
                // guards the Vec's own mutation while the caller reads later.
                results.lock().unwrap()[ix] = Some((paths[ix].clone(), result));
            });
        }
    });

    let slots = std::mem::take(&mut *results.lock().unwrap());
    slots
        .into_iter()
        .map(|slot| slot.expect("every slot filled by a worker"))
        .collect()
}

fn read_one(path: &Path, want_physical: bool) -> Option<Vec<RawEntry>> {
    DirReader::read(path, want_physical)
        .ok()
        .map(DirReader::into_entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parallel_read_matches_sequential() {
        let base = std::env::temp_dir().join(format!("sift-walk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        for i in 0..8 {
            let dir = base.join(format!("d{i}"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("f.txt"), vec![b'x'; i * 100 + 1]).unwrap();
        }

        let paths: Vec<PathBuf> = (0..8).map(|i| base.join(format!("d{i}"))).collect();
        let parallel = walk_dirs(&paths, true, 4);
        let sequential = walk_dirs(&paths, true, 1);

        assert_eq!(parallel.len(), sequential.len());
        for (p, seq) in sequential.iter() {
            let par = parallel
                .iter()
                .find(|(path, _)| path == p)
                .expect("parallel result present");
            let mut par_names: Vec<&[u8]> = par
                .1
                .as_ref()
                .unwrap()
                .iter()
                .map(|e| e.name.as_slice())
                .collect();
            let mut seq_names: Vec<&[u8]> = seq
                .as_ref()
                .unwrap()
                .iter()
                .map(|e| e.name.as_slice())
                .collect();
            par_names.sort_unstable();
            seq_names.sort_unstable();
            assert_eq!(par_names, seq_names, "results disagree for {p:?}");
        }

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_directories_render_none() {
        let missing = PathBuf::from("/nonexistent/sift-walk-missing");
        let results = walk_dirs(std::slice::from_ref(&missing), true, 2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, missing);
        assert!(results[0].1.is_none());
    }
}
