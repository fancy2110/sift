// POC: parallel openat walk with correct termination and adaptive fan-out.
//
// Termination: every open directory fd carries one token (`inflight`).
// Children kept on a worker's local DFS stack stay inside that token;
// children handed to peers increment inflight and go through the channel.
// When a worker has no local work it closes its token; inflight==0 means the
// whole scan finished. Every wait polls `done`, so blocked workers are never
// left hanging.
//
// Fan-out: workers that are about to block register in `idle`. While opening
// a directory's children, up to `idle` of them are handed off so parallelism
// spreads fast near the root; the rest stay local for DFS locality once
// everyone is busy.
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender};
use sift_platform::dir::DirReader;

fn cstr(s: &[u8]) -> CString {
    CString::new(s).unwrap()
}

fn update_max(counter: &AtomicU64, value: u64) {
    let mut current = counter.load(Ordering::Relaxed);
    while value > current {
        match counter.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(existing) => current = existing,
        }
    }
}

struct Jobs {
    tx: Sender<i32>,
    rx: Receiver<i32>,
    inflight: AtomicU64,
    idle: AtomicU64,
    done: AtomicBool,
    /// High-water mark of simultaneously open directory fds.
    open_dirs: AtomicU64,
    max_open_dirs: AtomicU64,
    /// Most direct children seen in a single directory.
    max_fanout: AtomicU64,
}

impl Jobs {
    /// Block until a handed-off fd arrives or the scan finishes.
    fn wait_fd(&self) -> Option<i32> {
        self.idle.fetch_add(1, Ordering::SeqCst);
        loop {
            match self.rx.recv_timeout(Duration::from_millis(1)) {
                Ok(fd) => {
                    self.idle.fetch_sub(1, Ordering::SeqCst);
                    return Some(fd);
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return None,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if self.done.load(Ordering::SeqCst) {
                        self.idle.fetch_sub(1, Ordering::SeqCst);
                        return None;
                    }
                }
            }
        }
    }

    /// Close the current token; returns true if this worker should exit
    /// outright (it was the last token).
    fn close_token(&self) -> bool {
        let prev = self.inflight.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            self.done.store(true, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
}

fn worker_loop(
    first_fd: Option<i32>,
    jobs: Arc<Jobs>,
    files: Arc<AtomicU64>,
    dirs: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
) {
    let mut stack: Vec<i32> = Vec::new();
    let mut current = match first_fd {
        Some(fd) => fd,
        None => match jobs.wait_fd() {
            Some(fd) => fd,
            None => return,
        },
    };

    loop {
        let mut child_fds: Vec<i32> = Vec::new();
        let mut children_opened = 0u64;
        match DirReader::read_fd(current, true) {
            Ok(reader) => {
                dirs.fetch_add(1, Ordering::Relaxed);
                for entry in reader.entries() {
                    if entry.name == b"." || entry.name == b".." {
                        continue;
                    }
                    if entry.is_dir && !entry.is_symlink {
                        let name = cstr(&entry.name);
                        let child = unsafe {
                            libc::openat(
                                current,
                                name.as_ptr(),
                                libc::O_RDONLY | libc::O_DIRECTORY,
                            )
                        };
                        if child >= 0 {
                            let now = jobs.open_dirs.fetch_add(1, Ordering::SeqCst) + 1;
                            update_max(&jobs.max_open_dirs, now);
                            child_fds.push(child);
                            children_opened += 1;
                        }
                    } else if entry.is_file {
                        files.fetch_add(1, Ordering::Relaxed);
                        bytes.fetch_add(entry.logical_size, Ordering::Relaxed);
                    }
                }
            }
            Err(_) => {}
        }
        unsafe { libc::close(current) };
        jobs.open_dirs.fetch_sub(1, Ordering::SeqCst);
        update_max(&jobs.max_fanout, children_opened);

        // Hand off as many children as there appear to be idle peers; keep the
        // rest locally for DFS locality.
        let mut handoff: Vec<i32> = Vec::new();
        while !child_fds.is_empty() && jobs.idle.load(Ordering::SeqCst) > 0 {
            handoff.push(child_fds.pop().unwrap());
        }
        if !handoff.is_empty() {
            jobs.inflight.fetch_add(handoff.len() as u64, Ordering::SeqCst);
            for fd in handoff {
                let _ = jobs.tx.send(fd);
            }
        }
        stack.extend(child_fds);

        if let Some(fd) = stack.pop() {
            current = fd;
            continue;
        }
        if jobs.close_token() {
            return;
        }
        match jobs.wait_fd() {
            Some(fd) => current = fd,
            None => return,
        }
    }
}

fn main() {
    let root: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or("/Users".into());
    let workers: usize = std::env::args()
        .nth(2)
        .and_then(|w| w.parse().ok())
        .unwrap_or(10);

    let root_c = cstr(root.as_os_str().as_bytes());
    let root_fd =
        unsafe { libc::open(root_c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY) };
    assert!(root_fd >= 0, "cannot open root {}", root.display());

    let (tx, rx) = unbounded::<i32>();
    let jobs = Arc::new(Jobs {
        tx,
        rx,
        inflight: AtomicU64::new(1),
        idle: AtomicU64::new(0),
        done: AtomicBool::new(false),
        open_dirs: AtomicU64::new(1),
        max_open_dirs: AtomicU64::new(1),
        max_fanout: AtomicU64::new(0),
    });
    let files = Arc::new(AtomicU64::new(0));
    let dirs = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));

    let started = Instant::now();
    let mut handles = Vec::new();
    for id in 0..workers {
        let jobs = Arc::clone(&jobs);
        let files = Arc::clone(&files);
        let dirs = Arc::clone(&dirs);
        let bytes = Arc::clone(&bytes);
        let first_fd = if id == 0 { Some(root_fd) } else { None };
        handles.push(std::thread::spawn(move || {
            worker_loop(first_fd, jobs, files, dirs, bytes)
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    let elapsed = started.elapsed();
    println!(
        "openat-parallel workers={workers} files={} dirs={} bytes={} time={:.3}s",
        files.load(Ordering::Relaxed),
        dirs.load(Ordering::Relaxed),
        bytes.load(Ordering::Relaxed),
        elapsed.as_secs_f64()
    );
    println!(
        "resources : peak open dir fds={}, max directory fanout={}",
        jobs.max_open_dirs.load(Ordering::Relaxed),
        jobs.max_fanout.load(Ordering::Relaxed)
    );
}
