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
            // The design's own icons, so a screenshot shows the real glyphs.
            Arc::new(sift_gpui_app::assets::SiftAssets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(gpui_kit::init);
        cx.update(sift_gpui_app::theme::install);
        cx
    }

    /// Put a disk in the window, as the platform enumeration does at launch.
    fn seed_volume(view: &gpui_kit::Entity<AppView>, cx: &mut gpui_kit::App) {
        view.update(cx, |view, cx| {
            view.model().update(cx, |model, cx| {
                model.adopt_volumes(vec![Volume::new(
                    VolumeId::from_mount_point(std::path::Path::new("/")),
                    "Macintosh HD",
                    "/",
                    1_941_354_332_160,
                    994_662_584_320,
                    false,
                    "APFS",
                )]);
                cx.notify();
            });
        });
    }

    fn seed(view: &gpui_kit::Entity<AppView>, cx: &mut gpui_kit::App, dir: &str, entries: Vec<DirectoryEntry>) {
        view.update(cx, |view, cx| {
            view.model().update(cx, |model, cx| {
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
    /// Render one scenario, let any entrance animation settle, and write it.
    ///
    /// `settle_ms` matters: the popup's entrance is 320 ms of time-based motion,
    /// so a capture taken immediately would show the frame the panel is still
    /// growing out of. Zero keeps the mid-flight frame on purpose, which is how
    /// the motion itself gets checked.
    fn shoot(
        name: &str,
        seed_scan: bool,
        settle_ms: u64,
        prepare: impl FnOnce(&mut HeadlessAppContext, &gpui_kit::Entity<AppView>),
    ) {
        let mut cx = context();
        let mut captured: Option<gpui_kit::Entity<AppView>> = None;
        let handle = cx
            .open_window(size(px(W), px(H)), |window: &mut Window, cx| {
                let view = cx.new(|cx| AppView::with_store(temp_store(), Vec::new(), window, cx));
                // Every scenario has a disk: the window shows which one it is on
                // from the moment it opens, and only the walk waits for a click.
                seed_volume(&view, cx);
                if seed_scan {
                    seed(&view, cx, "/Users/dev", home_listing());
                }
                captured = Some(view.clone());
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("a headless window");
        let view = captured.expect("the view");
        prepare(&mut cx, &view);
        if settle_ms > 0 {
            let _ = cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx));
            std::thread::sleep(std::time::Duration::from_millis(settle_ms));
        }
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
        shoot("shell", true, 0, |_, _| {});
        // Before the user asks: no walk, and the window has to say so.
        shoot("idle", false, 0, |_, _| {});
        // Mid-flight: at 45 ms of a 320 ms entrance the panel is still ~96% of
        // its size and ~54% opaque (backOut covers most of its distance early),
        // and the rows have not started their staggered arrival yet.
        shoot("popup-enter", true, 45, |cx, view| {
            view.update(cx, |view, cx| {
                view.model().update(cx, |model, cx| {
                    model.toggle_selected(key("/Users/dev/node_modules"));
                    cx.notify();
                });
                view.open_candidates(cx);
            });
        });
        shoot("popup", true, 420, |cx, view| {
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
        // The AI configuration surface, opened from the title bar or ⌘,.
        shoot("settings", true, 420, |cx, view| {
            view.update(cx, |view, cx| {
                view.model().update(cx, |model, cx| {
                    model.set_settings_open(true);
                    cx.notify();
                });
            });
        });
        shoot("scanning", true, 0, |cx, view| {
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
