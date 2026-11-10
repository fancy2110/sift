//! Bounded environment inventory: apps, package managers and projects.
//!
//! Everything here is intentionally incremental-friendly: the caller passes the
//! scan roots and a home directory; the functions cap their work and never read
//! file contents beyond small manifests (a package.json for `packageManager`,
//! an app's Info.plist for identity).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::context::{redact_path, AppFact, PmFact, ProjectFact, ScanContext, SCHEMA_VERSION};

const MAX_APPS: usize = 300;
const MAX_PROJECTS: usize = 100;
const MAX_WALK_DIRS: usize = 3_000;
const PROJECT_DEPTH: usize = 3;

/// A package manager worth detecting in the first version.
struct PmSpec {
    id: &'static str,
    bins: &'static [&'static str],
    version_args: &'static [&'static str],
    /// Absolute roots, or roots relative to `$HOME` when prefixed with `~`.
    roots: &'static [&'static str],
    capabilities: &'static [&'static str],
}

const PM_SPECS: &[PmSpec] = &[
    PmSpec {
        id: "homebrew",
        bins: &["brew"],
        version_args: &["--version"],
        roots: &["/opt/homebrew", "/usr/local"],
        capabilities: &["cleanCache", "removeOrphan", "verifyIntegrity"],
    },
    PmSpec {
        id: "npm",
        bins: &["npm"],
        version_args: &["--version"],
        roots: &["~/.npm", "~/Library/Caches/npm"],
        capabilities: &["cleanCache", "verifyIntegrity", "dedupe"],
    },
    PmSpec {
        id: "pnpm",
        bins: &["pnpm"],
        version_args: &["--version"],
        roots: &[
            "~/.local/share/pnpm/store",
            "~/Library/pnpm/store",
            "~/.pnpm-store",
        ],
        capabilities: &["cleanCache", "pruneStore", "dedupe"],
    },
    PmSpec {
        id: "yarn",
        bins: &["yarn"],
        version_args: &["--version"],
        roots: &["~/.cache/yarn", "~/Library/Caches/Yarn"],
        capabilities: &["cleanCache", "dedupe"],
    },
    PmSpec {
        id: "cargo",
        bins: &["cargo"],
        version_args: &["--version"],
        roots: &["~/.cargo"],
        capabilities: &["cleanCache", "removeOldVersion"],
    },
    PmSpec {
        id: "pip",
        bins: &["pip3", "pip"],
        version_args: &["--version"],
        roots: &["~/.cache/pip", "~/Library/Caches/pip"],
        capabilities: &["cleanCache", "verifyIntegrity"],
    },
    PmSpec {
        id: "conda",
        bins: &["conda"],
        version_args: &["--version"],
        roots: &["~/miniconda3", "~/anaconda3", "~/.conda"],
        capabilities: &["cleanCache", "removeOldVersion"],
    },
    PmSpec {
        id: "go",
        bins: &["go"],
        version_args: &["version"],
        roots: &["~/go", "~/.cache/go-build"],
        capabilities: &["cleanCache", "verifyIntegrity"],
    },
    PmSpec {
        id: "gem",
        bins: &["gem"],
        version_args: &["--version"],
        roots: &["~/.gem"],
        capabilities: &["cleanCache", "removeOldVersion"],
    },
    PmSpec {
        id: "composer",
        bins: &["composer"],
        version_args: &["--version"],
        roots: &["~/.composer", "~/.cache/composer"],
        capabilities: &["cleanCache", "verifyIntegrity"],
    },
    PmSpec {
        id: "rustup",
        bins: &["rustup"],
        version_args: &["--version"],
        roots: &["~/.rustup"],
        capabilities: &["removeOldVersion"],
    },
    PmSpec {
        id: "flatpak",
        bins: &["flatpak"],
        version_args: &["--version"],
        roots: &["~/.var/app"],
        capabilities: &["removeOrphan", "cleanCache"],
    },
    PmSpec {
        id: "snap",
        bins: &["snap"],
        version_args: &["version"],
        roots: &["~/snap"],
        capabilities: &["cleanCache", "removeOldVersion"],
    },
    PmSpec {
        id: "scoop",
        bins: &["scoop"],
        version_args: &["--version"],
        roots: &["~/scoop"],
        capabilities: &["cleanCache", "removeOldVersion"],
    },
];

/// Build a scan context. Never fails: missing tools/projects simply produce an
/// empty section, and the caller can mark it stale if it times out.
pub fn detect(roots: &[PathBuf], home: Option<&Path>) -> ScanContext {
    let home_owned = home.map(Path::to_path_buf).or_else(dirs::home_dir);
    let home = home_owned.as_deref();
    let now = now_ms();

    let apps = detect_apps(home);
    let projects = detect_projects(roots, home);
    let package_managers = detect_package_managers(home, &projects);

    ScanContext {
        schema_version: SCHEMA_VERSION,
        scanned_at_ms: now,
        roots: roots.iter().map(|root| redact_path(root, home)).collect(),
        apps,
        package_managers,
        projects,
        stale: false,
        truncated: false,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

// ---- applications ----------------------------------------------------------

#[cfg(target_os = "macos")]
fn detect_apps(home: Option<&Path>) -> Vec<AppFact> {
    let mut dirs: Vec<PathBuf> = vec![PathBuf::from("/Applications")];
    if let Some(home) = home {
        dirs.push(home.join("Applications"));
    }
    // Apple's utility apps live one level down; system apps are not user
    // uninstallable, so /System/Applications is deliberately excluded.
    dirs.push(PathBuf::from("/Applications/Utilities"));

    let mut apps = Vec::new();
    let mut seen = BTreeSet::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|ext| ext == "app") {
                continue;
            }
            if !path.is_dir() {
                continue;
            }
            let Some(app) = read_macos_app(&path, home) else {
                continue;
            };
            if seen.insert(app.key.clone()) {
                apps.push(app);
                if apps.len() >= MAX_APPS {
                    return apps;
                }
            }
        }
    }
    apps
}

#[cfg(target_os = "linux")]
fn detect_apps(home: Option<&Path>) -> Vec<AppFact> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/var/lib/flatpak/exports/share/applications"),
    ];
    if let Some(home) = home {
        dirs.push(home.join(".local/share/applications"));
        dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
    }
    let mut apps = Vec::new();
    let mut seen = BTreeSet::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut name = None;
            let mut flatpak = None;
            let mut no_display = false;
            for line in text.lines() {
                let line = line.trim();
                if let Some(value) = line.strip_prefix("Name=") {
                    name.get_or_insert_with(|| value.trim().to_string());
                } else if let Some(value) = line.strip_prefix("X-Flatpak=") {
                    flatpak = Some(value.trim().to_string());
                } else if line.eq_ignore_ascii_case("NoDisplay=true") {
                    no_display = true;
                }
            }
            if no_display {
                continue;
            }
            let Some(name) = name else {
                continue;
            };
            let key = flatpak.unwrap_or_else(|| {
                path.file_stem()
                    .map(|stem| format!("desktop:{}", stem.to_string_lossy()))
                    .unwrap_or_else(|| format!("desktop:{}", path.display()))
            });
            if !seen.insert(key.clone()) {
                continue;
            }
            apps.push(AppFact {
                key,
                name,
                version: None,
                bundle_id: None,
                install_path: redact_path(&path, home),
                last_used_days: None,
                artifact_count: 0,
            });
            if apps.len() >= MAX_APPS {
                return apps;
            }
        }
    }
    apps
}

#[cfg(windows)]
fn detect_apps(home: Option<&Path>) -> Vec<AppFact> {
    // Registry is the canonical uninstall list on Windows. `reg query /s` is
    // used instead of a new dependency; output is treated as best-effort text.
    let mut apps = Vec::new();
    let mut seen = BTreeSet::new();
    for root in [
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\Software\Microsoft\Windows\CurrentVersion\Uninstall",
        r"HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
    ] {
        let Ok(output) = Command::new("reg")
            .args(["query", root, "/s"])
            .stdin(Stdio::null())
            .output()
        else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut current_key = String::new();
        let mut name = None;
        let mut version = None;
        let mut location = None;
        let mut flush = |key: &str,
                         name: &Option<String>,
                         version: &Option<String>,
                         location: &Option<String>| {
            let Some(name) = name else {
                return None;
            };
            if !seen.insert(key.to_string()) {
                return None;
            }
            Some(AppFact {
                key: format!("registry:{key}"),
                name: name.clone(),
                version: version.clone(),
                bundle_id: None,
                install_path: location
                    .as_deref()
                    .map(|path| redact_path(Path::new(path), home))
                    .unwrap_or_default(),
                last_used_days: None,
                artifact_count: 0,
            })
        };
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("HKEY_") {
                if let Some(app) = flush(&current_key, &name, &version, &location) {
                    apps.push(app);
                    if apps.len() >= MAX_APPS {
                        return apps;
                    }
                }
                current_key = trimmed.to_string();
                name = None;
                version = None;
                location = None;
            } else if let Some(value) = trimmed.strip_prefix("DisplayName") {
                name = registry_value(value);
            } else if let Some(value) = trimmed.strip_prefix("DisplayVersion") {
                version = registry_value(value);
            } else if let Some(value) = trimmed.strip_prefix("InstallLocation") {
                location = registry_value(value);
            }
        }
        if let Some(app) = flush(&current_key, &name, &version, &location) {
            apps.push(app);
        }
    }
    apps
}

#[cfg(windows)]
fn registry_value(line: &str) -> Option<String> {
    let value = line.split_once("REG_SZ").map(|(_, value)| value.trim())?;
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn detect_apps(_home: Option<&Path>) -> Vec<AppFact> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn read_macos_app(path: &Path, home: Option<&Path>) -> Option<AppFact> {
    let info = path.join("Contents/Info.plist");
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(&info)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

    let string = |key: &str| {
        value
            .get(key)
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let bundle_id = string("CFBundleIdentifier");
    let name = string("CFBundleDisplayName")
        .or_else(|| string("CFBundleName"))
        .or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })?;
    let version = string("CFBundleShortVersionString");

    let key = bundle_id
        .clone()
        .unwrap_or_else(|| sift_core::id::NodeKey::from_path(path).to_string());
    Some(AppFact {
        key,
        name,
        version,
        bundle_id,
        install_path: redact_path(path, home),
        last_used_days: None,
        artifact_count: 0,
    })
}

// ---- package managers ------------------------------------------------------

fn detect_package_managers(home: Option<&Path>, projects: &[ProjectFact]) -> Vec<PmFact> {
    let mut out = Vec::new();
    for spec in PM_SPECS {
        let Some((bin, version)) = spec.bins.iter().find_map(|bin| {
            let path = find_on_path(bin)?;
            let version = probe_version(&path, spec.version_args);
            Some((path, version))
        }) else {
            continue;
        };
        let roots: Vec<String> = spec
            .roots
            .iter()
            .filter_map(|root| expand_root(root, home))
            .filter(|path| path.exists())
            .map(|path| redact_path(&path, home))
            .collect();
        let projects: Vec<String> = projects
            .iter()
            .filter(|project| project.pm == spec.id)
            .map(|project| project.root.clone())
            .collect();
        out.push(PmFact {
            id: spec.id.to_string(),
            version,
            detected: true,
            roots,
            projects,
            capabilities: spec.capabilities.iter().map(|c| c.to_string()).collect(),
        });
        let _ = bin;
    }
    out
}

fn expand_root(root: &str, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(rest) = root.strip_prefix("~/") {
        return home.map(|home| home.join(rest));
    }
    Some(PathBuf::from(root))
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

fn probe_version(bin: &Path, args: &[&str]) -> Option<String> {
    // A version probe must never hang a scan. Spawn it, wait at most a couple
    // of seconds, and kill a stuck child.
    let child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(Ok(output)) => output,
        _ => {
            kill_process(pid);
            return None;
        }
    };
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|line| !line.trim().is_empty())?;
    Some(line.trim().chars().take(80).collect())
}

fn kill_process(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
    }
}

// ---- projects --------------------------------------------------------------

fn detect_projects(roots: &[PathBuf], home: Option<&Path>) -> Vec<ProjectFact> {
    let mut search_roots: Vec<PathBuf> = roots
        .iter()
        .filter(|root| root.parent().is_some())
        // `/Applications` is full of bundles, not projects; walking it is both
        // useless and expensive.
        .filter(|root| {
            !matches!(
                root.to_string_lossy().as_ref(),
                "/Applications" | "/System" | "/Library" | "/usr" | "/opt" | "/var"
            )
        })
        .cloned()
        .collect();
    if search_roots.is_empty() {
        if let Some(home) = home {
            search_roots.push(home.to_path_buf());
        }
    }

    let mut projects = Vec::new();
    let mut visited = 0usize;
    let mut stack: Vec<(PathBuf, usize)> = search_roots.into_iter().map(|root| (root, 0)).collect();

    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_WALK_DIRS || projects.len() >= MAX_PROJECTS {
            break;
        }
        if let Some(project) = project_at(&dir, home) {
            projects.push(project);
            // A project root is a leaf for inventory purposes: nested packages
            // are usually part of the same workspace.
            continue;
        }
        if depth >= PROJECT_DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name();
            if is_skipped_dir(&name.to_string_lossy()) {
                continue;
            }
            stack.push((path, depth + 1));
        }
    }

    projects.sort_by(|a, b| a.root.cmp(&b.root));
    projects
}

fn is_skipped_dir(name: &str) -> bool {
    if [
        ".app",
        ".framework",
        ".bundle",
        ".xcodeproj",
        ".xcworkspace",
        ".photoslibrary",
        ".fcpbundle",
        ".appex",
        ".plugin",
        ".kext",
        ".dSYM",
    ]
    .iter()
    .any(|extension| name.ends_with(extension))
    {
        return true;
    }
    matches!(
        name,
        "node_modules"
            | "target"
            | ".git"
            | ".svn"
            | ".hg"
            | "Library"
            | "AppData"
            | ".cache"
            | ".Trash"
            | "vendor"
            | "Pods"
            | "DerivedData"
            | "dist"
            | "build"
            | "out"
            | ".next"
            | ".venv"
            | "venv"
            | "__pycache__"
            | "snap"
            | ".rustup"
            | ".cargo"
            | ".npm"
            | ".pnpm-store"
    )
}

fn project_at(dir: &Path, home: Option<&Path>) -> Option<ProjectFact> {
    let root = redact_path(dir, home);
    let has = |name: &str| dir.join(name).is_file();

    if has("package.json") {
        let lockfile = [
            ("pnpm-lock.yaml", "pnpm"),
            ("yarn.lock", "yarn"),
            ("bun.lockb", "bun"),
            ("package-lock.json", "npm"),
            ("npm-shrinkwrap.json", "npm"),
        ]
        .iter()
        .find(|(name, _)| has(name));
        let (pm, lock) = match lockfile {
            Some((name, pm)) => (*pm, Some(*name)),
            None => ("npm", None),
        };
        return Some(ProjectFact {
            root,
            pm: pm.to_string(),
            manifest: "package.json".to_string(),
            lockfile: lock.map(str::to_string),
            pm_version: package_manager_field(dir),
            dirty: false,
        });
    }

    if has("Cargo.toml") {
        return Some(ProjectFact {
            root,
            pm: "cargo".to_string(),
            manifest: "Cargo.toml".to_string(),
            lockfile: has("Cargo.lock").then(|| "Cargo.lock".to_string()),
            pm_version: None,
            dirty: false,
        });
    }

    if has("pyproject.toml") {
        let (pm, lock) = if has("poetry.lock") {
            ("poetry", Some("poetry.lock"))
        } else if has("uv.lock") {
            ("uv", Some("uv.lock"))
        } else {
            ("pip", None)
        };
        return Some(ProjectFact {
            root,
            pm: pm.to_string(),
            manifest: "pyproject.toml".to_string(),
            lockfile: lock.map(str::to_string),
            pm_version: None,
            dirty: false,
        });
    }

    if has("requirements.txt") {
        return Some(ProjectFact {
            root,
            pm: "pip".to_string(),
            manifest: "requirements.txt".to_string(),
            lockfile: None,
            pm_version: None,
            dirty: false,
        });
    }

    if has("go.mod") {
        return Some(ProjectFact {
            root,
            pm: "go".to_string(),
            manifest: "go.mod".to_string(),
            lockfile: has("go.sum").then(|| "go.sum".to_string()),
            pm_version: None,
            dirty: false,
        });
    }

    if has("Gemfile") {
        return Some(ProjectFact {
            root,
            pm: "bundler".to_string(),
            manifest: "Gemfile".to_string(),
            lockfile: has("Gemfile.lock").then(|| "Gemfile.lock".to_string()),
            pm_version: None,
            dirty: false,
        });
    }

    if has("composer.json") {
        return Some(ProjectFact {
            root,
            pm: "composer".to_string(),
            manifest: "composer.json".to_string(),
            lockfile: has("composer.lock").then(|| "composer.lock".to_string()),
            pm_version: None,
            dirty: false,
        });
    }

    if has("pom.xml") {
        return Some(ProjectFact {
            root,
            pm: "maven".to_string(),
            manifest: "pom.xml".to_string(),
            lockfile: None,
            pm_version: None,
            dirty: false,
        });
    }

    if has("build.gradle") || has("build.gradle.kts") {
        return Some(ProjectFact {
            root,
            pm: "gradle".to_string(),
            manifest: if has("build.gradle.kts") {
                "build.gradle.kts"
            } else {
                "build.gradle"
            }
            .to_string(),
            lockfile: None,
            pm_version: None,
            dirty: false,
        });
    }

    if has("mix.exs") {
        return Some(ProjectFact {
            root,
            pm: "mix".to_string(),
            manifest: "mix.exs".to_string(),
            lockfile: has("mix.lock").then(|| "mix.lock".to_string()),
            pm_version: None,
            dirty: false,
        });
    }

    None
}

fn package_manager_field(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("package.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let field = value.get("packageManager")?.as_str()?;
    Some(
        field
            .split_once('@')
            .map(|(_, version)| version.to_string())
            .unwrap_or_else(|| field.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sift-apps-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pnpm_lockfile_beats_npm_default() {
        let dir = temp_dir("pnpm");
        std::fs::write(
            dir.join("package.json"),
            r#"{"packageManager":"pnpm@9.12.0"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        let project = project_at(&dir, None).expect("project");
        assert_eq!(project.pm, "pnpm");
        assert_eq!(project.lockfile.as_deref(), Some("pnpm-lock.yaml"));
        assert_eq!(project.pm_version.as_deref(), Some("9.12.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cargo_project_is_detected_with_lockfile() {
        let dir = temp_dir("cargo");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        std::fs::write(dir.join("Cargo.lock"), "").unwrap();
        let project = project_at(&dir, None).expect("project");
        assert_eq!(project.pm, "cargo");
        assert_eq!(project.manifest, "Cargo.toml");
        assert_eq!(project.lockfile.as_deref(), Some("Cargo.lock"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn project_walk_skips_dependency_directories() {
        let dir = temp_dir("walk");
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::write(dir.join("node_modules/pkg/package.json"), "{}").unwrap();
        let project = project_at(&dir, None);
        assert!(project.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
