//! Scan benchmark: measures the engine against a real directory tree.
//!
//! ```text
//! cargo run --release -p sift-scan --example scan_bench -- /Applications
//! cargo run --release -p sift-scan --example scan_bench -- / --workers 8
//! ```
//!
//! Reports wall time, throughput, the tracked-node cost and the tree's own
//! resident bytes, so the "2 TB in under 60 s" target can be checked against a
//! real machine instead of asserted. It also prints which directory reader the
//! platform selected, because a silent fall back to the portable path would
//! otherwise look like a mysterious slowdown.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use sift_core::{format_bytes, ScanEvent, ScanId, ScanOutcome, ScanPolicy, ScanRequest};
use sift_platform::dir::DirReader;
use sift_scan::ScanEngine;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root: Option<PathBuf> = None;
    let mut workers: usize = 0;
    let mut policy = ScanPolicy::thorough();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workers" => {
                if let Some(value) = args.next() {
                    workers = value.parse().unwrap_or(0);
                }
            }
            "--lean" => {
                // Directories only, no hardlink table: the cheapest mode.
                policy = ScanPolicy::thorough().with_file_detail(u64::MAX);
                policy.dedupe_hardlinks = false;
            }
            other => root = Some(PathBuf::from(other)),
        }
    }

    let root = root.unwrap_or_else(|| PathBuf::from("/"));
    let root = root.canonicalize().unwrap_or_else(|_| root.clone());

    println!("root      : {}", root.display());
    println!("reader    : {}", describe_reader(&root));

    let engine = if workers > 0 {
        ScanEngine::with_workers(workers)
    } else {
        ScanEngine::new()
    };
    println!(
        "policy    : threads={} physical={} hardlinks={} file_detail={}",
        if workers > 0 {
            workers.to_string()
        } else {
            format!("auto({})", policy.resolved_threads())
        },
        policy.want_physical_size,
        policy.dedupe_hardlinks,
        if policy.file_detail_min_bytes == u64::MAX {
            "none".to_string()
        } else {
            format_bytes(policy.file_detail_min_bytes)
        }
    );

    let started = Instant::now();
    let handle = match engine.scan(
        ScanRequest::new(ScanId(1), root.clone())
            .with_focus(root.clone())
            .with_policy(policy),
    ) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("scan failed to start: {err}");
            std::process::exit(1);
        }
    };

    // Track the peak tracked-node count while draining events.
    let peak_nodes = Arc::new(AtomicU64::new(0));
    let peak_for_thread = Arc::clone(&peak_nodes);
    let tree_for_thread = Arc::clone(&handle.tree);
    let sampler = std::thread::spawn(move || {
        let mut peak = 0u64;
        for _ in 0..2000 {
            if let Ok(guard) = tree_for_thread.lock() {
                peak = peak.max(guard.node_count() as u64);
            }
            peak_for_thread.store(peak, Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });

    let mut outcome = ScanOutcome::Failed;
    let mut last_progress = None;
    while let Ok(event) = handle.events.recv() {
        match event {
            ScanEvent::Finished {
                outcome: finished,
                progress,
                ..
            } => {
                outcome = finished;
                last_progress = Some(progress);
                break;
            }
            ScanEvent::Progress { progress, .. } => last_progress = Some(progress),
            _ => {}
        }
    }
    handle.join();
    let elapsed = started.elapsed();
    sampler.join().ok();

    let progress = last_progress.unwrap_or_default();
    let (
        nodes,
        unrecorded,
        resident,
        node_bytes,
        interner_bytes,
        hardlink_bytes,
        hardlinks,
        table_full,
    ) = {
        let guard = handle.tree.lock().unwrap();
        let stats = guard.stats();
        (
            stats.nodes,
            stats.unrecorded_dirs,
            stats.resident_bytes,
            stats.node_bytes,
            stats.interner_bytes,
            stats.hardlink_bytes,
            stats.hardlink_entries,
            stats.hardlink_table_full,
        )
    };

    println!("outcome   : {outcome:?}");
    println!(
        "entries   : {} files, {} dirs, {} denied",
        progress.files, progress.dirs, progress.denied
    );
    println!(
        "bytes     : {} logical",
        format_bytes(progress.bytes.logical)
    );
    println!("time      : {:.3} s", elapsed.as_secs_f64());
    println!(
        "throughput: {:.0} entries/s, {:.0} files/s, {:.1} MB/s (by files)",
        progress.entries_per_sec(),
        progress.files as f64 / elapsed.as_secs_f64().max(0.001),
        progress.bytes.logical as f64 / 1_048_576.0 / elapsed.as_secs_f64().max(0.001)
    );
    println!(
        "memory    : tree {} tracked nodes, peak {} during scan, {} resident, {:.1} B/node",
        nodes,
        peak_nodes.load(Ordering::Relaxed),
        format_bytes(resident as u64),
        resident as f64 / nodes.max(1) as f64
    );
    println!(
        "  nodes   : {} ({:.0} B/node record), names {}, hardlinks {}",
        format_bytes(node_bytes as u64),
        node_bytes as f64 / nodes.max(1) as f64,
        format_bytes(interner_bytes as u64),
        format_bytes(hardlink_bytes as u64)
    );
    if unrecorded > 0 {
        println!("budget    : {unrecorded} directories counted but not recorded");
    }
    if hardlinks > 0 || table_full {
        println!("hardlinks : {hardlinks} considered, table full = {table_full}");
    }

    // Extrapolate honestly to the 2 TB target using the measured file rate.
    let files = progress.files.max(1);
    let secs_for_2m_files = 2_000_000.0 / (files as f64 / elapsed.as_secs_f64().max(0.001));
    println!(
        "budget    : at this rate, 2 M files would take {:.1} s (target: < 60 s)",
        secs_for_2m_files
    );
    if progress.coverage > 0.0 {
        println!(
            "coverage  : {:.1}% of the volume's used bytes",
            progress.coverage * 100.0
        );
    }
}

/// Which reader the platform picks for a directory, and whether it is the fast
/// path. Reporting this is what turns "the scan was slow" into a diagnosis.
fn describe_reader(dir: &std::path::Path) -> String {
    match DirReader::read(dir, true) {
        Ok(reader) => {
            let kind = reader.kind();
            format!(
                "{:?}{}",
                kind,
                if kind.is_fast() {
                    " (platform bulk enumeration)"
                } else {
                    " (portable read_dir + stat)"
                }
            )
        }
        Err(err) => format!("unavailable: {err}"),
    }
}
