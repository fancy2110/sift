//! Sift's native GPUI application.
//!
//! The model, the treemap layout and the service wiring deliberately expose a
//! little more than the current view uses: the drawing surface is still growing
//! into them (see the module docs), and every one of those items is covered by
//! unit tests. A binary crate's unused-API lint cannot see "tested and
//! exercised, just not wired to a button yet", so the allow is scoped here, with
//! this reason, rather than sprinkled over the items.
#![allow(dead_code)]
//!
//! The window is a thin renderer over [`model::WorkspaceModel`], which holds
//! every display decision and consumes the shared core's events. Nothing in the
//! view layer scans, analyzes, or decides what is deletable.
//!
//! Layout of the crate:
//!
//! * [`app`] — the root view and the window's regions;
//! * [`services`] — background wiring to `sift-platform`, `sift-scan`,
//!   `sift-analyze`, `sift-store` and `sift-monitor`;
//! * [`treemap_element`] — the custom `Element` that paints the treemap;
//! * [`overlays`] — sheets, dialogs and notifications;
//! * [`theme`] — the one place raw colours live;
//! * [`model`] — the GPUI-free workspace model (unchanged).

mod app;
mod model;
mod overlays;
mod services;
mod theme;
mod treemap_element;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::prelude::*;
use gpui_kit::{App, KeyBinding, WindowBounds, WindowOptions, bounds, point, px, size};

/// Window geometry is a physical, platform-owned boundary: a window is opened
/// against real screen coordinates, not against the interface's rem scale.
const WINDOW_SIZE: (f32, f32) = (1200., 800.);
const WINDOW_MIN_SIZE: (f32, f32) = (880., 560.);

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            theme::install(cx);
            bind_keys(cx);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds(
                    point(px(0.), px(0.)),
                    size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
                ))),
                window_min_size: Some(size(px(WINDOW_MIN_SIZE.0), px(WINDOW_MIN_SIZE.1))),
                ..TitleBar::window_options()
            };

            cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| app::AppView::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open the Sift window");
        });
}

/// Register the window's commands before anything can read the keymap.
///
/// The coding guide requires bindings to exist before a menu bar is built;
/// Sift ships no native menu bar, and binding here keeps that ordering true
/// even if one is added later.
fn bind_keys(cx: &mut App) {
    use app::{Activate, Dismiss, GoUp, Rescan, SelectNext, SelectPrev};

    cx.bind_keys([
        KeyBinding::new("cmd-r", Rescan, None),
        KeyBinding::new("ctrl-r", Rescan, None),
        KeyBinding::new("escape", Dismiss, None),
        KeyBinding::new("enter", Activate, None),
        KeyBinding::new("space", Activate, None),
        KeyBinding::new("up", SelectPrev, None),
        KeyBinding::new("down", SelectNext, None),
        KeyBinding::new("left", GoUp, None),
        KeyBinding::new("right", Activate, None),
    ]);
}
