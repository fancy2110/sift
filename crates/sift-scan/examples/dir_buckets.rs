// One parallel walk; bucket directory/file counts and bytes by top component.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use sift_platform::dir::DirReader;

fn top_component(path: &str, root_len: usize) -> String {
    let rest = path.get(root_len..).unwrap_or("").trim_start_matches('/');
    rest.split('/').next().filter(|s| !s.is_empty()).unwrap_or("(root)").to_string()
}

fn main() {
    let root: PathBuf = std::env::args().nth(1).map(PathBuf::from).unwrap_or("/Users/xiaocy".into());
    let workers: usize = std::env::args().nth(2).and_then(|w| w.parse().ok()).unwrap_or(14);
    let root_c = root.canonicalize().unwrap_or(root.clone());
    let root_text = root_c.to_string_lossy().to_string();
    let root_len = root_text.len();

    let root_dev = DirReader::read(&root_c, true).ok()
        .and_then(|r| r.entries().iter().find(|e| e.is_dir).map(|e| e.dev)).unwrap_or(0);

    let stack = Arc::new(Mutex::new(vec![root_c.clone()]));
    let active = Arc::new(AtomicU64::new(1));
    let buckets = Arc::new(Mutex::new(BTreeMap::<String,(u64,u64,u64)>::new()));
    let started = Instant::now();
    let mut handles = vec![];
    for _ in 0..workers {
        let stack=Arc::clone(&stack); let active=Arc::clone(&active); let buckets=Arc::clone(&buckets);
        let root_dev=root_dev;
        handles.push(std::thread::spawn(move || {
            let mut local: BTreeMap<String,(u64,u64,u64)> = BTreeMap::new();
            loop {
                let job = stack.lock().unwrap().pop();
                let Some(path) = job else {
                    if active.fetch_sub(1, std::sync::atomic::Ordering::AcqRel)==1 {}
                    std::thread::sleep(std::time::Duration::from_micros(150));
                    if active.load(std::sync::atomic::Ordering::Acquire)==0 { break; }
                    continue;
                };
                active.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                if let Ok(reader)=DirReader::read(&path,false) {
                    let path_text = path.to_string_lossy().to_string();
                    let bucket = top_component(&path_text, root_len);
                    let e = local.entry(bucket).or_insert((0,0,0));
                    e.0 += 1; // this dir
                    let mut new=vec![];
                    for entry in reader.entries() {
                        if entry.name==b"."||entry.name==b".." {continue;}
                        if entry.is_dir && !entry.is_symlink {
                            if root_dev!=0&&entry.dev!=0&&entry.dev!=root_dev {continue;}
                            let mut c=path.clone(); c.push(Path::new(&String::from_utf8_lossy(&entry.name).to_string()));
                            new.push(c);
                        } else {
                            let e=local.entry(top_component(&path_text, root_len)).or_insert((0,0,0));
                            e.1+=1; e.2+=entry.logical_size;
                        }
                    }
                    if !new.is_empty(){ stack.lock().unwrap().append(&mut new); }
                }
            }
            let mut g=buckets.lock().unwrap();
            for (k,v) in local { let e=g.entry(k).or_insert((0,0,0)); e.0+=v.0; e.1+=v.1; e.2+=v.2; }
        }));
    }
    for h in handles {h.join().ok();}
    let g=buckets.lock().unwrap();
    let mut rows: Vec<_> = g.iter().collect();
    rows.sort_by_key(|(_,v)| std::cmp::Reverse(v.0));
    println!("walk {:.1}s   top components under {} :", started.elapsed().as_secs_f64(), root_text);
    println!("{:>10} {:>10} {:>12}  component", "dirs","files","bytes");
    for (k,(d,f,b)) in rows {
        println!("{d:>10} {f:>10} {:>12}  {k}", sift_core::format_bytes(*b));
    }
}
