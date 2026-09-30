// Single-thread A/B: open(full path) walk vs openat(parent_fd) walk.
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use sift_platform::dir::DirReader;

fn cstr(s: &[u8]) -> CString {
    CString::new(s).unwrap()
}

fn by_path(root: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut dirs = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(reader) = DirReader::read(&path, true) else { continue };
        dirs += 1;
        for entry in reader.entries() {
            if entry.name == b"." || entry.name == b".." {
                continue;
            }
            if entry.is_dir && !entry.is_symlink {
                let mut child = path.clone();
                child.push(Path::new(&String::from_utf8_lossy(&entry.name).to_string()));
                stack.push(child);
            } else {
                files += 1;
            }
        }
    }
    (files, dirs)
}

fn by_fd(fd: i32) -> (u64, u64) {
    use std::os::raw::c_int;
    let mut files = 0u64;
    let mut dirs = 0u64;
    let reader = match DirReader::read_fd(fd, true) {
        Ok(reader) => reader,
        Err(_) => return (0, 0),
    };
    dirs += 1;
    for entry in reader.entries() {
        if entry.name == b"." || entry.name == b".." {
            continue;
        }
        if entry.is_dir && !entry.is_symlink {
            let name = cstr(&entry.name);
            let child: c_int = unsafe {
                libc::openat(fd, name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY)
            };
            if child >= 0 {
                let (f, d) = by_fd(child);
                files += f;
                dirs += d;
                unsafe { libc::close(child) };
            }
        } else {
            files += 1;
        }
    }
    (files, dirs)
}

fn main() {
    let root: PathBuf = std::env::args().nth(1).map(PathBuf::from).unwrap_or("/Users".into());

    let root_c = cstr(root.as_os_str().as_bytes());

    // Warmup both paths once so steady-state numbers do not favour whichever
    // ran first (a path walk populates the dentry cache for the other).
    let _ = by_path(&root);
    let wfd = unsafe { libc::open(root_c.as_ptr(), libc::O_RDONLY) };
    if wfd >= 0 { let _ = by_fd(wfd); unsafe { libc::close(wfd) }; }

    // Alternate three rounds; print each, so ordering bias averages out.
    for round in 1..=3 {
        let started = Instant::now();
        let (f1, _d1) = by_path(&root);
        let e1 = started.elapsed();
        let fd = unsafe { libc::open(root_c.as_ptr(), libc::O_RDONLY) };
        let started = Instant::now();
        let (f2, _d2) = if fd >= 0 { by_fd(fd) } else { (0, 0) };
        let e2 = started.elapsed();
        if fd >= 0 { unsafe { libc::close(fd) }; }
        println!("round{round} by_path={:.2}s({f1})  openat={:.2}s({f2})",
            e1.as_secs_f64(), e2.as_secs_f64());
    }
}
