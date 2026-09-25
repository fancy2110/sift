//! The Sift binary: open one window on the GPUI Kit application.
//!
//! Everything interesting lives in the library next to this file; see
//! `sift_gpui_app::app` for the view and `sift_gpui_app::model` for the state it
//! renders.

use gpui_kit::prelude::*;
use gpui_kit::{WindowOptions, px, size};
use sift_gpui_app::app::AppView;
use sift_gpui_app::theme;

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);

        let options = WindowOptions {
            window_bounds: Some(gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds::new(
                gpui_kit::point(px(80.), px(80.)),
                size(px(1180.), px(760.)),
            ))),
            window_min_size: Some(size(px(880.), px(560.))),
            ..Default::default()
        };

        cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| AppView::new(window, cx));
            // `Root` must be the window's first view: it owns the dialog, sheet
            // and notification layers that the view renders each frame.
            cx.new(|cx| gpui_kit::component::Root::new(view, window, cx))
        })
        .expect("failed to open the Sift window");
    });
}
