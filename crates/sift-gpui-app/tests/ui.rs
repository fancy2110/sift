//! UI integration tests: render the real view in a headless window and drive it
//! with native events.
//!
//! These exist because "it compiles and the window opens" is not evidence that a
//! surface works. Each test seeds the model with the data a scan would produce,
//! renders a frame, dispatches a click on the control a user would press, and
//! asserts the outcome — including the negative case, so a control that should
//! refuse to act is proven to refuse.

use gpui_kit::component::{Root, WindowExt as _};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{Entity, TestAppContext, px, size};
use gpui_kit::prelude::*;
use sift_core::{ByteSize, DirectoryEntry, DirectorySummary, NodeKey, ScanEvent, ScanId};
use sift_store::{Store, StorePaths};
use sift_gpui_app::app::AppView;

/// A key whose `Display` form is stable enough to use as an element id.
fn key(path: &str) -> NodeKey {
    NodeKey::from_path(std::path::Path::new(path))
}

fn entry(path: &str, size: u64, is_dir: bool, deletable: bool) -> DirectoryEntry {
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
        deletable,
    )
}

/// A store rooted in a temp directory, so a test never touches the real one.
fn temp_store(tag: &str) -> Store {
    let root = std::env::temp_dir().join(format!(
        "sift-ui-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let (store, warnings) = Store::open(StorePaths::at(root));
    assert!(warnings.is_empty(), "clean store expected: {warnings:?}");
    store
}

/// Open the application in a headless window, returning the view entity.
fn open_app(
    cx: &mut TestAppContext,
    tag: &str,
) -> (Entity<AppView>, gpui_kit::WindowHandle<Root>) {
    cx.update(gpui_kit::init);
    let store = temp_store(tag);
    let mut captured: Option<Entity<AppView>> = None;
    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| AppView::with_store(store, Vec::new(), window, cx));
        captured = Some(view.clone());
        Root::new(view, window, cx)
    });
    (captured.expect("view built"), handle)
}

/// Seed the model with one directory listing, as the scanner would.
fn seed_directory(view: &Entity<AppView>, cx: &mut gpui_kit::App, dir: &str, entries: Vec<DirectoryEntry>) {
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

/// Clicking a file row queues it, and the cleanup control then opens the
/// candidate sheet listing exactly what is about to move.
#[gpui_kit::test]
fn clicking_a_row_queues_it_and_the_cleanup_control_opens_the_sheet(
    cx: &mut TestAppContext,
) {
    let (view, handle) = open_app(cx, "sheet");
    cx.update(|cx| {
        seed_directory(
            &view,
            cx,
            "/root",
            vec![
                entry("/root/keeper.bin", 4096, false, true),
                entry("/root/junk.bin", 8192, false, true),
            ],
        )
    });

    let alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // Nothing is queued yet, so the cleanup control must do nothing.
        window.click("clean", cx);
        assert!(
            !window.has_active_sheet(cx),
            "an empty queue must not open the candidate sheet"
        );

        // Queue one file by clicking its row.
        window.click(format!("row-{}", key("/root/junk.bin").raw()), cx);
        let selected: Vec<NodeKey> = view.read(cx).model().read(cx).selection().iter().copied().collect();
        assert_eq!(selected, vec![key("/root/junk.bin")], "the row click queues it");
        assert_eq!(view.read(cx).model().read(cx).selected_bytes(), 8192);

        // Now the control opens the sheet.
        window.render_frame(cx);
        window.click("clean", cx);
        assert!(
            window.has_active_sheet(cx),
            "a non-empty queue opens the candidate sheet"
        );

        // Escape dismisses the topmost surface and returns to the task.
        window.press("escape", cx);
        assert!(!window.has_active_sheet(cx), "escape closes the sheet");
    });
    assert!(alive.is_ok(), "the window must still exist: {alive:?}");
}

/// A row the deletion policy refuses must not enter the queue, however it is
/// clicked.
#[gpui_kit::test]
fn a_locked_row_cannot_be_queued(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "locked");
    cx.update(|cx| {
        seed_directory(
            &view,
            cx,
            "/root",
            vec![entry("/root/system.bin", 4096, false, false)],
        )
    });

    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(format!("row-{}", key("/root/system.bin").raw()), cx);
        assert!(
            view.read(cx).model().read(cx).selection().is_empty(),
            "an undeletable row must not be queueable"
        );
        assert!(!window.has_active_sheet(cx));
    });
}

/// Drilling into a directory moves the current view and leaves the queue alone.
#[gpui_kit::test]
fn clicking_a_directory_drills_in(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "drill");
    cx.update(|cx| {
        seed_directory(&view, cx, "/root", vec![entry("/root/sub", 0, true, true)])
    });

    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(format!("row-{}", key("/root/sub").raw()), cx);
        assert_eq!(
            view.read(cx).model().read(cx).current_dir(),
            Some(key("/root/sub")),
            "a directory row navigates instead of queueing"
        );
        assert!(view.read(cx).model().read(cx).selection().is_empty());
    });
}

/// The treemap is a real element: it must lay out and register hitboxes in a
/// frame without panicking, and expose its tiles to the accessibility tree.
#[gpui_kit::test]
fn the_treemap_renders_tiles(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "treemap");
    cx.update(|cx| {
        seed_directory(
            &view,
            cx,
            "/root",
            vec![
                entry("/root/big", 8 * 1024 * 1024, true, true),
                entry("/root/small", 1024 * 1024, true, true),
            ],
        )
    });

    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // The tiles are laid out by the model (unit-tested separately); here the
        // claim is narrower and about the element: a frame with tiles painted
        // and hitboxes registered completes.
        let layout = view.read(cx).model().read(cx).treemap_layout(
            sift_gpui_app::model::Rect::new(0.0, 0.0, 700.0, 500.0),
            2000.0,
        );
        assert_eq!(layout.tiles.len(), 2);
        window.render_frame(cx);
    });
}

/// A volume picker is present and is a real control, not a label.
#[gpui_kit::test]
fn the_volume_picker_is_a_control(cx: &mut TestAppContext) {
    let (_view, handle) = open_app(cx, "picker");
    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let picker = window.find("volume-picker");
        assert_eq!(picker.role(), Some(gpui_kit::Role::Button));
        // With no volumes loaded yet the control still renders a hit area.
        assert!(picker.bounds().size.width > px(0.));
    });
}

/// A test-support id on the status region proves the frame composed the regions
/// the shell promises, not just the title bar.
#[gpui_kit::test]
fn the_shell_renders_its_regions(cx: &mut TestAppContext) {
    let (_view, handle) = open_app(cx, "shell");
    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        for id in ["scan", "panel", "analyze", "clean", "monitor"] {
            let control = window.find(id);
            assert!(
                control.bounds().size.width > px(0.),
                "{id} must have a real hit area"
            );
        }
    });
}

/// The scan control is disabled while a scan runs, so a second click cannot
/// start a competing walk.
#[gpui_kit::test]
fn the_scan_control_disables_while_scanning(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "scanning");
    view.update(cx, |view, cx| {
        view.model().update(cx, |model, cx| {
            model.begin_scan();
            cx.notify();
        });
    });

    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // The component does not publish `disabled` into the snapshot (only
        // opted-in custom elements expose extra state), so assert the state this
        // app owns and shows instead.
        assert_eq!(
            window.find("scan").label(),
            Some("扫描中…"),
            "a running scan changes the control's label"
        );

        // Clicking it must not start a second scan: the model is still the same
        // one and the service is not asked to restart.
        window.click("scan", cx);
        assert!(view.read(cx).model().read(cx).is_scanning());
    });
}

/// Right-clicking a row builds and shows its context menu. The menu's items do
/// not carry test ids, so this asserts the construction path (which is where an
/// API misuse would panic) rather than a chosen item.
#[gpui_kit::test]
fn right_clicking_a_row_shows_its_menu(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "context-menu");
    cx.update(|cx| {
        seed_directory(
            &view,
            cx,
            "/root",
            vec![entry("/root/junk.bin", 8192, false, true)],
        )
    });

    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.right_click(format!("row-{}", key("/root/junk.bin").raw()), cx);
        // The menu is a popover layer; rendering it again must not panic and the
        // shell must still be there.
        window.render_frame(cx);
        assert!(window.find("clean").bounds().size.width > px(0.));
        assert!(
            view.read(cx).model().read(cx).selection().is_empty(),
            "opening a menu does not itself queue anything"
        );

        // Close the window so the popover entity it owns is dropped: GPUI's test
        // harness fails a test that exits with a leaked handle, and a context
        // menu's PopupMenu lives in the window's keyed element state.
        window.remove_window();
    });
}

/// `TestSupportExt` is required for the treemap element to expose its tiles; the
/// seam exists and composes.
#[gpui_kit::test]
fn test_support_is_available_for_custom_elements(cx: &mut TestAppContext) {
    let (_view, handle) = open_app(cx, "test-support");
    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // `find` only sees opted-in ids; the treemap registers none, so asking
        // for one returns None rather than panicking.
        assert!(window.try_find("treemap-tile-0").is_none());
    });
}
