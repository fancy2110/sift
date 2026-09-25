//! Analysis demo: scan a real directory and print what Sift concludes.
//!
//! ```text
//! cargo run --release -p sift-monitor --example analyze_demo -- ~/Downloads
//! cargo run --release -p sift-monitor --example analyze_demo -- /Applications
//! ```
//!
//! Shows the whole judgement path on real data: local rules nominate candidates,
//! the adjudicator assigns a safety level with a reason, the store remembers the
//! definitively cleanable ones, and habit mining reports what the user has been
//! doing by hand. Nothing is deleted and nothing leaves the machine — the remote
//! adjudicator is not attached.

use std::path::PathBuf;
use std::sync::Arc;

use sift_analyze::{
    AnalysisPolicy, Analyzer, CachedAdjudicator, Reason, RuleAdjudicator, Safety,
};
use sift_core::{format_bytes, ScanEvent, ScanId, ScanPolicy, ScanRequest};
use sift_scan::ScanEngine;
use sift_store::{MonitorSettings, Store, StorePaths};

fn main() {
    let root: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
    let root = root.canonicalize().unwrap_or(root);
    println!("分析目标 : {}", root.display());

    // ---- scan --------------------------------------------------------------
    let engine = ScanEngine::new();
    let handle = match engine.scan(
        ScanRequest::new(ScanId(1), root.clone())
            .with_focus(root.clone())
            .with_policy(ScanPolicy::thorough()),
    ) {
        Ok(handle) => handle,
        Err(err) => {
            eprintln!("扫描无法启动: {err}");
            std::process::exit(1);
        }
    };
    let mut progress = Default::default();
    while let Ok(event) = handle.events.recv() {
        match event {
            ScanEvent::Progress {
                progress: latest, ..
            } => progress = latest,
            ScanEvent::Finished { .. } => break,
            _ => {}
        }
    }
    handle.join();
    println!(
        "扫描     : {} 文件 / {} 目录，{} 逻辑字节，拒绝 {}",
        progress.files,
        progress.dirs,
        format_bytes(progress.bytes.logical),
        progress.denied
    );

    // ---- analyze -----------------------------------------------------------
    // The store is the verdict cache, so a second run reuses these conclusions.
    let store_paths = StorePaths::at(
        std::env::temp_dir().join(format!("sift-analyze-demo-{}", std::process::id())),
    );
    let (store, warnings) = Store::open(store_paths.clone());
    for warning in warnings {
        println!("提示     : {warning}");
    }
    let store = Arc::new(store);

    let analyzer = Analyzer::new(
        AnalysisPolicy::default().with_min_bytes(16 * 1024 * 1024),
        dirs::home_dir(),
    );
    // The cache is the store: a conclusion reached earlier is reused instead of
    // being re-derived (or, with a remote adjudicator attached, re-transmitted).
    let adjudicator = CachedAdjudicator::new(RuleAdjudicator::new(), Arc::clone(&store));

    let report = {
        let mut tree = handle.tree.lock().unwrap();
        analyzer.analyze(&adjudicator, &mut tree, now_ms(), |_, _, _| {})
    };

    store.record_analysis(&report, now_ms());
    let _ = store.flush_if_dirty();

    // ---- report ------------------------------------------------------------
    println!(
        "\n结论     : {} 个候选，可回收 {}（其中无需人工确认 {}）",
        report.items.len(),
        format_bytes(report.reclaimable()),
        format_bytes(report.safe_bytes())
    );
    let mut sources: Vec<(&String, &usize)> = report.source_counts.iter().collect();
    sources.sort_by(|left, right| right.1.cmp(left.1));
    println!(
        "来源     : {}",
        sources
            .iter()
            .map(|(label, count)| format!("{label}={count}"))
            .collect::<Vec<_>>()
            .join("  ")
    );

    println!("\n{:>10}  {:<6}  {:<18}  对象 / 理由", "可回收", "等级", "来源");
    println!("{}", "-".repeat(100));
    for item in report.sorted_by_size().into_iter().take(30) {
        let level = match item.verdict.safety {
            Safety::Safe => "safe",
            Safety::Review => "review",
            Safety::Keep => "keep",
        };
        println!(
            "{:>10}  {:<6}  {:<18}  {}",
            format_bytes(item.candidate.reclaimable()),
            level,
            item.verdict.source.label(),
            item.candidate.display_path,
        );
        println!("{:>10}  {:<6}  {:<18}  └ {}", "", "", "", render_reason(&item.verdict.reason));
    }

    // ---- what was remembered ----------------------------------------------
    let cleanable = store.cleanable();
    println!(
        "\n本地清单 : {} 条明确可清理，共 {}",
        cleanable.len(),
        format_bytes(cleanable.total_bytes())
    );
    let confidence = sift_analyze::ConfidencePolicy::default();
    println!(
        "自动资格 : {} 条 / {}（需逐条批准后才可无人值守清理）",
        cleanable.auto_eligible(&confidence).len(),
        format_bytes(cleanable.auto_eligible_bytes(&confidence))
    );

    // ---- habits ------------------------------------------------------------
    let suggestions = store.decisions().suggest_routines(3, 3);
    if suggestions.is_empty() {
        println!("习惯     : 暂无（需要跨 3 天以上的重复决策才会建议为例行任务）");
    } else {
        for suggestion in suggestions {
            println!(
                "习惯     : 重复{}「{}」{} 次（{} 天），均 {} / {}",
                if suggestion.is_destructive() {
                    "清理"
                } else {
                    "保留"
                },
                suggestion.name,
                suggestion.occurrences,
                suggestion.distinct_days,
                format_bytes(suggestion.average_bytes),
                suggestion.cadence.label_key()
            );
        }
    }

    // ---- monitor policy preview -------------------------------------------
    let config = sift_monitor::MonitorConfig::from_settings(&MonitorSettings::default());
    let (total, available) = sift_platform::volume::free_space(&root).unwrap_or((0, 0));
    let sample = sift_monitor::DiskSample::new(total, available, now_ms());
    let home = dirs::home_dir();
    let decision = sift_monitor::evaluate(
        &config,
        &sample,
        cleanable.entries(),
        home.as_deref(),
        None,
        None,
        now_ms(),
    );
    println!(
        "\n监控预览 : 剩余 {}（{:.1}%），级别 {:?}，动作 {}",
        format_bytes(available),
        if total > 0 {
            available as f64 / total as f64 * 100.0
        } else {
            0.0
        },
        decision.level,
        describe_action(&decision.action),
    );

    let _ = std::fs::remove_dir_all(store_paths.root());
}

fn render_reason(reason: &Reason) -> String {
    match reason {
        Reason::Key { key, params } if params.is_empty() => key.clone(),
        Reason::Key { key, params } => format!("{key}({})", params.join(", ")),
        Reason::Text(text) => text.clone(),
    }
}

fn describe_action(action: &sift_monitor::MonitorAction) -> String {
    match action {
        sift_monitor::MonitorAction::Quiet => "静默".to_string(),
        sift_monitor::MonitorAction::Notify => "仅提示".to_string(),
        sift_monitor::MonitorAction::NeedsConfirmation { reason, entries } => {
            format!("需确认（{}，{} 项）", reason.label_key(), entries.len())
        }
        sift_monitor::MonitorAction::AutoClean { entries } => {
            format!("自动清理 {} 项", entries.len())
        }
        // `MonitorAction` is non-exhaustive.
        _ => "未知动作".to_string(),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
