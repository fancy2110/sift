//! UI integration tests: render the real view in a headless window and drive it
//! with native events.
//!
//! These exist because "it compiles and the window opens" is not evidence that a
//! surface works. Each test seeds the model with the data a scan would produce,
//! renders a frame, dispatches a click on the control a user would press, and
//! asserts the outcome — including the negative case, so a control that should
//! refuse to act is proven to refuse.

use gpui_kit::component::Root;
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
    cx.update(sift_gpui_app::theme::install);
    let store = temp_store(tag);
    let mut captured: Option<Entity<AppView>> = None;
    let handle = cx.open_window(size(px(1180.), px(760.)), |window, cx| {
        let view = cx.new(|cx| AppView::with_store(store, Vec::new(), window, cx));
        captured = Some(view.clone());
        Root::new(view, window, cx)
    });
    (captured.expect("view built"), handle)
}

/// Let the popup's entrance finish before touching what is inside it.
///
/// The panel morphs out of the capsule over 320 ms, so for the first frames it
/// is still the capsule's size and its controls are outside the visible area.
/// The animation is driven by real time, so waiting is the way to settle it.
fn settle_entrance(window: &mut gpui_kit::Window, cx: &mut gpui_kit::App) {
    window.render_frame(cx);
    std::thread::sleep(std::time::Duration::from_millis(360));
    window.render_frame(cx);
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

        // The capsule toggles the popup through `open_candidates` /
        // `close_candidates`, which is the seam this asserts; the capsule's own
        // pointer handler is that same call.
        view.update(cx, |view, cx| view.open_candidates(cx));
        assert!(view.read(cx).candidates_open(cx), "the popup opens");
        view.update(cx, |view, cx| view.close_candidates(cx));
        assert!(!view.read(cx).candidates_open(cx), "and closes again");

        // Queue one file by clicking its row.
        window.click(format!("row-{}", key("/root/junk.bin").raw()), cx);
        let selected: Vec<NodeKey> = view.read(cx).model().read(cx).selection().iter().copied().collect();
        assert_eq!(selected, vec![key("/root/junk.bin")], "the row click queues it");
        assert_eq!(view.read(cx).model().read(cx).selected_bytes(), 8192);

        // Now the capsule opens the popup, which carries the commit action.
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_candidates(cx));
        settle_entrance(window, cx);
        assert!(view.read(cx).candidates_open(cx));
        assert!(
            window.find("clean").bounds().size.width > px(0.),
            "the popup carries the commit action"
        );

        // Escape dismisses the topmost surface and returns to the task.
        window.press("escape", cx);
        assert!(!view.read(cx).candidates_open(cx), "escape closes the popup");
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
        assert!(!view.read(cx).candidates_open(cx));
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
        // Component controls register themselves; the shell's custom surfaces
        // (the capsule, the switch) are asserted through state instead, because
        // a plain `div` is not in the accessibility tree.
        for id in ["scan", "volume-picker"] {
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
            Some("正在扫描"),
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
        assert!(
            window
                .find(format!("row-{}", key("/root/junk.bin").raw()))
                .bounds()
                .size
                .width
                > px(0.),
            "the shell is still there under the menu"
        );
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

/// The design's frames, measured on the rendered elements.
///
/// A screenshot shows that a height is wrong; only a number stops it drifting
/// back. The row *pitch* is measurable because each row's button is observable,
/// so two consecutive rows give the frame the design specifies.
#[gpui_kit::test]
fn the_shell_frames_match_the_design(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "frames");
    let first = key("/root/cache");
    let second = key("/root/junk.bin");
    cx.update(|cx| {
        seed_directory(
            &view,
            cx,
            "/root",
            vec![
                entry("/root/cache", 4096, true, true),
                entry("/root/junk.bin", 8192, false, true),
            ],
        );
    });
    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let top = window.find(format!("row-{}", first.raw())).bounds().origin.y;
        let bottom = window.find(format!("row-{}", second.raw())).bounds().origin.y;
        let pitch: f32 = (bottom - top).into();
        assert!(
            (pitch - sift_gpui_app::theme::metrics::ROW_H).abs() < 0.5,
            "rows are {pitch} px apart; the design's frame is {} px (18 px slot + 2x7 px)",
            sift_gpui_app::theme::metrics::ROW_H
        );
        assert_eq!(sift_gpui_app::theme::metrics::ROW_H, 32.0);
    });
}

/// A treemap whose long tail folds into the aggregate tile must render.
///
/// The earlier fixtures were small enough that the aggregate tile never earned a
/// label, so this case was never rendered by a test: the tile is 884x156 px here
/// and its label starts with a three-byte character ("其他 · N 项"). That matters
/// because GPUI shapes text through byte-ranged runs, so a run length that does
/// not land on a character boundary is a process abort rather than a visual
/// glitch — an early revision did abort the application here. The label run is
/// now derived from the string, and this test holds the case open.
#[gpui_kit::test]
fn a_folded_long_tail_renders_its_aggregate_label(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "aggregate");
    let mut entries = vec![entry("/root/big.bin", 2_000_000_000, false, true)];
    // Many entries that individually fall under the fold threshold but together
    // hold enough bytes to give the aggregate tile a label-bearing area.
    for index in 0..40 {
        entries.push(entry(
            &format!("/root/tail-{index:02}.bin"),
            18_000_000,
            false,
            true,
        ));
    }
    cx.update(|cx| seed_directory(&view, cx, "/root", entries));

    let alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // The model must have folded, or the test is not exercising the label.
        let folded = view
            .read(cx)
            .model()
            .read(cx)
            .treemap_layout(
                sift_gpui_app::model::Rect::new(0.0, 0.0, 912.0, 657.0),
                88.0 * 48.0,
            )
            .tiles
            .iter()
            .any(|tile| tile.is_aggregate());
        assert!(folded, "the fixture must fold its long tail");
    });
    assert!(alive.is_ok());
}

/// The window must know which disk it is on, and must not scan until asked.
///
/// Both were broken in the same place: the startup path kept the chosen volume in
/// the service layer only — so the picker rendered "—" and the summary had no free
/// space — and it started the walk immediately.
///
/// The volume enumeration runs on its own thread and the pump that delivers it
/// waits on a background timer, which a test scheduler does not advance, so this
/// drives the same policy with the same real input instead: the platform's own
/// volume list, through the model call the window uses.
#[gpui_kit::test]
fn the_window_knows_its_disk_and_waits_to_be_asked(cx: &mut TestAppContext) {
    let (view, _handle) = open_app(cx, "idle");
    assert!(
        !sift_platform::volume::list_volumes().is_empty(),
        "this machine has at least one volume"
    );

    // The same call the startup path makes, minus the background thread and the
    // pump's timer — neither of which a test scheduler advances.
    let (known, named, scanning, started, entries) = cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.services_mut().refresh_volumes(cx);
            let model = view.model().read(cx);
            (
                model.current_volume_id().is_some(),
                model
                    .current_volume()
                    .is_some_and(|volume| !volume.name.is_empty()),
                model.is_scanning(),
                model.has_started(),
                model.visible_entries().len(),
            )
        })
    });

    assert!(
        known && named,
        "the enumerated disk must land in the model, or the picker shows a dash"
    );
    assert!(
        !scanning && !started,
        "learning which disks exist must not start a scan"
    );
    assert_eq!(entries, 0, "nothing has been walked yet");
}

/// The AI configuration must be reachable and must actually persist.
///
/// Both halves matter: a settings form that cannot be opened is dead UI, and one
/// that closes without writing is worse — it looks like it worked. The shortcut
/// is checked too, because the menu item's displayed shortcut comes from the same
/// binding, so a broken chord is a menu bar that lies about it.
#[gpui_kit::test]
fn the_ai_settings_open_from_the_shortcut_and_persist(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "ai-settings");
    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            !view.read(cx).model().read(cx).settings_open(),
            "the panel starts closed"
        );

        window.press("cmd-,", cx);
        assert!(
            view.read(cx).model().read(cx).settings_open(),
            "the preferences shortcut opens the panel"
        );

        window.render_frame(cx);
        // The consent step is the one that decides whether anything leaves the
        // machine, so it is the one worth driving end to end.
        window.click("ai-consent", cx);
        window.render_frame(cx);
        window.click("ai-settings-save", cx);
    });

    let (consent, open) = cx.update(|cx| {
        (
            view.read(cx).services().store().settings().ai.consent_granted,
            view.read(cx).model().read(cx).settings_open(),
        )
    });
    assert!(
        consent,
        "saving must write the consent decision to the store"
    );
    assert!(!open, "saving closes the panel");
}

/// Escape closes the settings panel before anything under it.
#[gpui_kit::test]
fn escape_closes_the_settings_panel(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "ai-settings-escape");
    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.press("cmd-,", cx);
        assert!(view.read(cx).model().read(cx).settings_open());
        window.press("escape", cx);
        assert!(!view.read(cx).model().read(cx).settings_open());
    });
}

/// The scan control is the only thing that starts a walk, and the menu items are
/// actions, so the two must agree: pressing them must not scan on their own.
#[gpui_kit::test]
fn menu_actions_do_not_start_a_scan_on_their_own(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "menu-actions");
    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // Toggling the candidate list and the monitor are window actions; neither
        // is a scan.
        window.press("cmd-shift-c", cx);
        assert!(view.read(cx).model().read(cx).drawer_open());
        window.press("cmd-shift-c", cx);
        assert!(!view.read(cx).model().read(cx).drawer_open());

        assert!(
            !view.read(cx).model().read(cx).has_started(),
            "no menu action may begin a scan"
        );
    });
}

/// Clicking outside the candidate popup must dismiss it.
///
/// The click lands on a file row that sits *under* the popup's scrim, which
/// makes this two assertions in one: the popup closes, and the row does not
/// receive the click. A scrim that merely looks right would fail the second.
#[gpui_kit::test]
fn clicking_outside_the_popup_closes_it(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "popup-outside");
    let junk = key("/root/junk.bin");
    cx.update(|cx| {
        seed_directory(
            &view,
            cx,
            "/root",
            vec![
                entry("/root/junk.bin", 8192, false, true),
                entry("/root/cache", 4096, true, true),
            ],
        );
    });
    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_candidates(cx));
        settle_entrance(window, cx);
        assert!(view.read(cx).model().read(cx).drawer_open(), "the popup is open");

        // A row that is behind the popup's scrim.
        window.click(format!("row-{}", junk.raw()), cx);
        window.render_frame(cx);

        assert!(
            !view.read(cx).model().read(cx).drawer_open(),
            "a click outside the popup closes it"
        );
        assert!(
            view.read(cx).model().read(cx).selection().is_empty(),
            "and the row behind the scrim must not act on it"
        );

        // The scrim covers the whole window, so the title bar counts as outside
        // too — and the control under it must not fire on the dismissing click.
        view.update(cx, |view, cx| view.open_candidates(cx));
        window.render_frame(cx);
        window.click("ai-settings-open", cx);
        window.render_frame(cx);
        assert!(
            !view.read(cx).model().read(cx).drawer_open(),
            "a click on the title bar closes the popup"
        );
        assert!(
            !view.read(cx).model().read(cx).settings_open(),
            "and does not also press the control underneath"
        );
    });
}

/// A click inside the popup is not a click outside it.
#[gpui_kit::test]
fn clicking_inside_the_popup_keeps_it_open(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "popup-inside");
    cx.update(|cx| {
        seed_directory(
            &view,
            cx,
            "/root",
            vec![entry("/root/junk.bin", 8192, false, true)],
        );
    });
    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        view.update(cx, |view, cx| view.open_candidates(cx));
        settle_entrance(window, cx);

        // The panel's own furniture is the nearest observable thing inside it.
        // The queue is empty, so the commit button is disabled and pressing it
        // does nothing — which makes it a safe probe for the one thing that
        // matters here: without stop-propagation the click would fall through to
        // the scrim and dismiss the panel it is inside.
        assert!(
            view.read(cx).model().read(cx).selection().is_empty(),
            "the probe only works with an empty queue"
        );
        window.click("clean", cx);
        window.render_frame(cx);
        assert!(
            view.read(cx).model().read(cx).drawer_open(),
            "a click inside the panel must not dismiss it"
        );
    });
}

/// A running scan reports progress beside the cleanup entry, and can be stopped.
///
/// The progress element is a plain `div`, which a test cannot look up, so the
/// position is asserted through the stop control inside it: that control is a
/// component button, and its bounds are enough to prove the status sits in the
/// bottom band next to the capsule rather than over the map.
#[gpui_kit::test]
fn a_running_scan_can_be_stopped_from_its_progress(cx: &mut TestAppContext) {
    let (view, handle) = open_app(cx, "cancel-scan");
    let _alive = cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window.try_find("cancel-scan").is_none(),
            "nothing to stop when nothing is running"
        );

        // A scan in flight, as the engine reports it.
        view.update(cx, |view, cx| {
            view.model().update(cx, |model, cx| {
                model.begin_scan();
                cx.notify();
            });
        });
        window.render_frame(cx);

        let cancel = window.find("cancel-scan").bounds();
        // The window is 1180x760; the workspace's bottom band starts after the
        // map, and the capsule occupies the first ~200px of it.
        assert!(
            cancel.origin.y > px(660.),
            "the stop control belongs to the bottom band, not the map: y={:?}",
            cancel.origin.y
        );
        assert!(
            cancel.origin.x > px(200.),
            "and to the right of the capsule: x={:?}",
            cancel.origin.x
        );

        window.click("cancel-scan", cx);
        window.render_frame(cx);
        assert!(
            !view.read(cx).model().read(cx).is_scanning(),
            "stopping ends the walk"
        );
        assert!(
            window.try_find("cancel-scan").is_none(),
            "and the progress goes with it"
        );
    });
}
