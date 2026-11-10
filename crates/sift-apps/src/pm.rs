//! Package-manager capability catalog and safe execution.
//!
//! Commands are static templates selected by a closed action token. The caller
//! only supplies ids/actions; it never supplies a program, argument list or
//! shell string. Execution resolves the program through PATH, runs it without a
//! shell and enforces a timeout.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Destructiveness {
    Safe,
    Review,
    Danger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PmActionPlan {
    pub pm_id: String,
    pub action: String,
    pub program: String,
    pub args: Vec<String>,
    pub scope: String,
    pub requires_admin: bool,
    pub destructiveness: Destructiveness,
    pub reason_key: String,
    pub impact_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PmRunReport {
    pub pm_id: String,
    pub action: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
}

/// Look up a runnable plan for `(pm_id, action)`. Unknown combinations return
/// `None`; the UI must not invent commands.
pub fn plan_action(pm_id: &str, action: &str) -> Option<PmActionPlan> {
    let (program, args, scope, destructiveness, requires_admin) = match (pm_id, action) {
        ("homebrew", "cleanCache") => (
            "brew",
            vec!["cleanup", "--prune=all"],
            "global",
            Destructiveness::Safe,
            false,
        ),
        ("homebrew", "removeOrphan") => (
            "brew",
            vec!["autoremove"],
            "global",
            Destructiveness::Review,
            false,
        ),
        ("homebrew", "verifyIntegrity") => (
            "brew",
            vec!["doctor"],
            "global",
            Destructiveness::Safe,
            false,
        ),
        ("npm", "cleanCache") | ("npm", "verifyIntegrity") => (
            "npm",
            vec!["cache", "verify"],
            "global",
            Destructiveness::Safe,
            false,
        ),
        ("pnpm", "pruneStore") | ("pnpm", "cleanCache") => (
            "pnpm",
            vec!["store", "prune"],
            "global",
            Destructiveness::Safe,
            false,
        ),
        ("yarn", "cleanCache") => (
            "yarn",
            vec!["cache", "clean"],
            "global",
            Destructiveness::Safe,
            false,
        ),
        ("pip", "cleanCache") => (
            "pip",
            vec!["cache", "purge"],
            "global",
            Destructiveness::Safe,
            false,
        ),
        ("conda", "cleanCache") => (
            "conda",
            vec!["clean", "--all", "--yes"],
            "global",
            Destructiveness::Review,
            false,
        ),
        ("go", "cleanCache") => (
            "go",
            vec!["clean", "-cache"],
            "global",
            Destructiveness::Safe,
            false,
        ),
        ("gem", "cleanCache") => (
            "gem",
            vec!["cleanup"],
            "global",
            Destructiveness::Review,
            false,
        ),
        ("composer", "cleanCache") => (
            "composer",
            vec!["clear-cache"],
            "global",
            Destructiveness::Safe,
            false,
        ),
        ("scoop", "cleanCache") => (
            "scoop",
            vec!["cache", "rm", "*"],
            "global",
            Destructiveness::Review,
            false,
        ),
        ("flatpak", "removeOrphan") => (
            "flatpak",
            vec!["uninstall", "--unused", "--assume-yes"],
            "user",
            Destructiveness::Review,
            false,
        ),
        _ => return None,
    };
    Some(PmActionPlan {
        pm_id: pm_id.to_string(),
        action: action.to_string(),
        program: program.to_string(),
        args: args.into_iter().map(str::to_string).collect(),
        scope: scope.to_string(),
        requires_admin,
        destructiveness,
        reason_key: format!("maintenance.pm.{action}.reason"),
        impact_key: format!("maintenance.pm.{action}.impact"),
    })
}

/// Execute a validated plan. `Danger` actions are refused here even if a plan
/// somehow exists; the caller must not be able to bypass the catalog.
pub fn run_action(plan: &PmActionPlan) -> Result<PmRunReport, String> {
    if plan.destructiveness == Destructiveness::Danger {
        return Err("err.dangerousAction".to_string());
    }
    let program = find_program(&plan.program)
        .ok_or_else(|| format!("err.programNotFound|{}", plan.program))?;
    let started = Instant::now();
    let child = Command::new(program)
        .args(&plan.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("err.spawn|{err}"))?;
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(Duration::from_secs(300)) {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return Err(format!("err.wait|{err}")),
        Err(_) => {
            kill_process(pid);
            return Err("err.timeout".to_string());
        }
    };
    Ok(PmRunReport {
        pm_id: plan.pm_id.clone(),
        action: plan.action.clone(),
        success: output.status.success(),
        exit_code: output.status.code(),
        duration_ms: started.elapsed().as_millis() as u64,
        stdout: bounded(&output.stdout),
        stderr: bounded(&output.stderr),
    })
}

fn bounded(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.chars().take(8_000).collect()
}

fn find_program(name: &str) -> Option<PathBuf> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pnpm_store_prune_is_a_static_safe_command() {
        let plan = plan_action("pnpm", "pruneStore").expect("plan");
        assert_eq!(plan.program, "pnpm");
        assert_eq!(plan.args, vec!["store", "prune"]);
        assert_eq!(plan.destructiveness, Destructiveness::Safe);
        assert!(!plan.requires_admin);
    }

    #[test]
    fn unknown_pairs_are_rejected() {
        assert!(plan_action("pnpm", "uninstallPackage").is_none());
        assert!(plan_action("unknown", "cleanCache").is_none());
    }

    #[test]
    fn danger_is_never_executed() {
        let plan = PmActionPlan {
            pm_id: "test".into(),
            action: "cleanCache".into(),
            program: "true".into(),
            args: vec![],
            scope: "global".into(),
            requires_admin: false,
            destructiveness: Destructiveness::Danger,
            reason_key: String::new(),
            impact_key: String::new(),
        };
        assert!(run_action(&plan).is_err());
    }
}
