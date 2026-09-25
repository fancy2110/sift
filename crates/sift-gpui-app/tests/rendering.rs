//! Render the real window and write a PNG, so the design稿 alignment can be
//! *looked at* rather than only asserted about.
//!
//! `Window::render_to_image` needs GPUI's Metal renderer, so this runs as a
//! plain binary on the main thread — the same layout GPUI Kit's own rendering
//! suite uses. The harness is deliberately not a test: a pixel change is not a
//! pass/fail, it is something to inspect.
//!
//! ```sh
//! cargo test -p sift-gpui-app --test rendering          # writes target/ui-shots/
//! ```
//!
//! Scenarios: `shell` (the default screen), `popup` (the cleanup candidate
//! popup open) and `scanning` (the in-progress state).

fn main() {
    #[cfg(target_os = "macos")]
    macos::run();
    #[cfg(not(target_os = "macos"))]
    println!("rendering: skipped; GPUI has no headless renderer on this platform");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::Arc;

    use gpui_kit::component::Root;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::prelude::*;
    use gpui_kit::{AppContext as _, HeadlessAppContext, Window, px, size};
    use sift_core::{
        ByteSize, DirectoryEntry, DirectorySummary, NodeKey, ScanEvent, ScanId, Volume, VolumeId,
    };
    use sift_gpui_app::app::AppView;
    use sift_store::{Store, StorePaths};

    /// The design's window size, so the screenshot is the shape a user sees.
    const W: f32 = 1280.0;
    const H: f32 = 832.0;

    fn key(path: &str) -> NodeKey {
        NodeKey::from_path(std::path::Path::new(path))
    }

    fn entry(path: &str, size: u64, is_dir: bool) -> DirectoryEntry {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        DirectoryEntry::new(
            key(path),
            name,
            is_dir,
            ByteSize::new(size, size),
            1_700_000_000_000,
            false,
            is_dir,
        )
    }

    /// A listing shaped like a real scan of a developer's home directory: a few
    /// huge trees, a long tail, and files mixed in — which is what the treemap
    /// and the list have to lay out legibly.
    fn home_listing() -> Vec<DirectoryEntry> {
        vec![
            entry("/Users/dev/Library", 41_200_000_000, true),
            entry("/Users/dev/Docker.raw", 12_884_901_888, false),
            entry("/Users/dev/node_modules", 9_126_000_000, true),
            entry("/Users/dev/target", 2_412_000_000, true),
            entry("/Users/dev/Movies", 1_940_000_000, true),
            entry("/Users/dev/Downloads", 1_284_000_000, true),
            entry("/Users/dev/.cache", 812_000_000, true),
            entry("/Users/dev/Documents", 604_000_000, true),
            entry("/Users/dev/Photos Library.photoslibrary", 512_000_000, true),
            entry("/Users/dev/installer.dmg", 402_653_184, false),
            entry("/Users/dev/archive-2023.zip", 268_435_456, false),
            entry("/Users/dev/Desktop", 190_000_000, true),
            entry("/Users/dev/xcode-build.log", 84_000_000, false),
            entry("/Users/dev/Music", 62_000_000, true),
            entry("/Users/dev/.npm", 41_000_000, true),
            entry("/Users/dev/notes.md", 12_288, false),
        ]
    }

    fn temp_store() -> Store {
        let root = std::env::temp_dir().join(format!("sift-render-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (store, warnings) = Store::open(StorePaths::at(root));
        assert!(warnings.is_empty(), "clean store expected: {warnings:?}");
        store
    }

    fn context() -> HeadlessAppContext {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(gpui_kit::assets::Assets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(gpui_kit::init);
        cx.update(sift_gpui_app::theme::install);
        cx
    }

    fn seed(view: &gpui_kit::Entity<AppView>, cx: &mut gpui_kit::App, dir: &str, entries: Vec<DirectoryEntry>) {
        view.update(cx, |view, cx| {
            view.model().update(cx, |model, cx| {
                // The nav and the summary capsule read a volume, so a screenshot
                // without one would show a screen a user never sees.
                let id = VolumeId::from_mount_point(std::path::Path::new("/"));
                model.set_volumes(vec![Volume::new(
                    id,
                    "Macintosh HD",
                    "/",
                    1_941_354_332_160,
                    994_662_584_320,
                    false,
                    "APFS",
                )]);
                model.select_volume(id);
                model.begin_scan();
                model.apply_scan_event(ScanEvent::DirectoryListed {
                    scan: ScanId(1),
                    dir: DirectorySummary::new(
                        key(dir),
                        dir,
                        ByteSize::new(entries.iter().map(|e| e.size.logical).sum(), 0),
                        entries.len() as u32,
                        0,
                        entries,
                    ),
                });
                model.apply_scan_event(ScanEvent::Finished {
                    scan: ScanId(1),
                    outcome: sift_core::ScanOutcome::Completed,
                    progress: sift_core::Progress::default(),
                });
                cx.notify();
            });
        });
    }

    /// Render one scenario and write it to `target/ui-shots/<name>.png`.
    fn shoot(
        name: &str,
        prepare: impl FnOnce(&mut HeadlessAppContext, &gpui_kit::Entity<AppView>),
    ) {
        let mut cx = context();
        let mut captured: Option<gpui_kit::Entity<AppView>> = None;
        let handle = cx
            .open_window(size(px(W), px(H)), |window: &mut Window, cx| {
                let view = cx.new(|cx| AppView::with_store(temp_store(), Vec::new(), window, cx));
                seed(&view, cx, "/Users/dev", home_listing());
                captured = Some(view.clone());
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("a headless window");
        let view = captured.expect("the view");
        prepare(&mut cx, &view);
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .expect("a frame");
        let image = cx
            .capture_screenshot(handle.into())
            .expect("Metal rendering must be available");
        let out = std::path::Path::new("target/ui-shots");
        std::fs::create_dir_all(out).expect("output directory");
        let path = out.join(format!("{name}.png"));
        image.save(&path).expect("write the PNG");
        println!(
            "rendering: wrote {} ({}x{})",
            path.display(),
            image.width(),
            image.height()
        );
    }

    pub fn run() {
        shoot("shell", |_, _| {});
        shoot("popup", |cx, view| {
            // Queue three rows the way a user would, then open the popup through
            // the same seam the capsule calls.
            view.update(cx, |view, cx| {
                view.model().update(cx, |model, cx| {
                    for path in [
                        "/Users/dev/node_modules",
                        "/Users/dev/installer.dmg",
                        "/Users/dev/Docker.raw",
                    ] {
                        model.toggle_selected(key(path));
                    }
                    cx.notify();
                });
                view.open_candidates(cx);
            });
        });
        shoot("scanning", |cx, view| {
            view.update(cx, |view, cx| {
                view.model().update(cx, |model, cx| {
                    model.begin_scan();
                    cx.notify();
                });
            });
        });
        println!("rendering: done");
    }
}
