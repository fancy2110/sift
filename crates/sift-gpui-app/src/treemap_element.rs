//! The treemap as a custom GPUI element.
//!
//! The geometry is not here: `WorkspaceModel::treemap_layout` computes it (and
//! is unit-tested in `model/treemap.rs`). This element only measures the space
//! it was given, asks the model for the layout of that rectangle, and paints the
//! tiles, their labels, and the interaction affordances.
//!
//! Exact `gpui` signatures implemented (verified against
//! `gpui-pre-0.3.6/src/element.rs:53`):
//!
//! ```text
//! fn id(&self) -> Option<ElementId>;
//! fn source_location(&self) -> Option<&'static panic::Location<'static>>;
//! fn request_layout(&mut self, id: Option<&GlobalElementId>,
//!                   inspector_id: Option<&InspectorElementId>,
//!                   window: &mut Window, cx: &mut App)
//!     -> (LayoutId, Self::RequestLayoutState);
//! fn prepaint(&mut self, id: Option<&GlobalElementId>,
//!             inspector_id: Option<&InspectorElementId>, bounds: Bounds<Pixels>,
//!             request_layout: &mut Self::RequestLayoutState,
//!             window: &mut Window, cx: &mut App) -> Self::PrepaintState;
//! fn paint(&mut self, id: Option<&GlobalElementId>,
//!          inspector_id: Option<&InspectorElementId>, bounds: Bounds<Pixels>,
//!          request_layout: &mut Self::RequestLayoutState,
//!          prepaint: &mut Self::PrepaintState,
//!          window: &mut Window, cx: &mut App);
//! ```
//!
//! This is the only place in the treemap that names a raw pixel value, and only
//! for physical boundaries: a one-device-pixel separator and the two-pixel focus
//! ring that must stay visible over it.

use std::panic::Location;
use std::rc::Rc;

use sift_core::{format_bytes, NodeKey};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Bounds, Corners, CursorStyle, Edges, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior,
    Hsla, InspectorElementId, LayoutId, MouseDownEvent, MouseMoveEvent, Pixels, Point, ShapedLine,
    Style, TextAlign, TextStyleRefinement, TruncateFrom, Window, fill, point, px, relative, size,
};

use crate::model::{Rect, Tile, WorkspaceModel};
use crate::theme;

/// A one-device-pixel separator. This is the documented physical-boundary
/// exception to the rem scale: a hairline is a raster fact, not product spacing.
const HAIRLINE: Pixels = px(1.);
/// The focus ring is two hairlines wide so it stays visible over the tile tint.
const FOCUS_RING: Pixels = px(2.);
/// Tile labels sit one step below body type, expressed in rem.
const LABEL_REM: f32 = 0.8125;
/// Tile label line height as a multiple of the label's font size.
const LABEL_LINE_HEIGHT: f32 = 1.35;
/// The minimum tile area, in rem², before the tail is folded into "其他".
const MIN_TILE_REMS: f32 = 11.0;
/// A label needs at least this many font-heights of width to be worth painting.
const LABEL_MIN_FONT_WIDTHS: f32 = 3.0;

/// One painted tile: everything `paint` needs, resolved during `prepaint`.
pub struct PaintedTile {
    key: Option<NodeKey>,
    is_dir: bool,
    is_aggregate: bool,
    is_focused: bool,
    bounds: Bounds<Pixels>,
    fill: Hsla,
    /// Inserted during prepaint; GPUI only allows `insert_hitbox` there, and
    /// paint uses the stored handle for the cursor and the click/hover checks.
    hitbox: Hitbox,
    label: Option<ShapedLine>,
    label_origin: Point<Pixels>,
    line_height: Pixels,
}

/// The treemap element.
/// A callback the element invokes with the tile the user acted on.
pub type TileCallback = Rc<dyn Fn(NodeKey, &mut Window, &mut App)>;

pub struct TreemapElement {
    id: ElementId,
    model: gpui_kit::Entity<WorkspaceModel>,
    on_activate: TileCallback,
    on_focus: TileCallback,
}

impl TreemapElement {
    /// Build the element for one frame.
    ///
    /// `on_activate` receives the tile the user confirmed (drill in, or queue a
    /// file); `on_focus` receives the tile the pointer moved onto, so the
    /// treemap and the file list share one highlight.
    pub fn new(
        model: gpui_kit::Entity<WorkspaceModel>,
        on_activate: impl Fn(NodeKey, &mut Window, &mut App) + 'static,
        on_focus: impl Fn(NodeKey, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: ElementId::Name("treemap".into()),
            model,
            on_activate: Rc::new(on_activate),
            on_focus: Rc::new(on_focus),
        }
    }

    /// The label for a tile, already carrying its size.
    fn tile_label(tile: &Tile) -> String {
        match (tile.is_aggregate(), tile.folded_count) {
            (true, Some(count)) => format!("其他 · {count} 项"),
            _ => format!("{}  {}", tile.label, format_bytes(tile.size)),
        }
    }
}

impl IntoElement for TreemapElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TreemapElement {
    type RequestLayoutState = ();
    type PrepaintState = Vec<PaintedTile>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        // The treemap fills whatever its container gives it; the container
        // decides how much that is.
        let style = Style {
            size: size(relative(1.).into(), relative(1.).into()),
            flex_grow: 1.,
            flex_shrink: 1.,
            ..Style::default()
        };
        (window.request_layout(style, vec![], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let area = Rect::new(
            bounds.origin.x.into(),
            bounds.origin.y.into(),
            bounds.size.width.into(),
            bounds.size.height.into(),
        );

        let unit: f32 = window.rem_size().into();
        let min_tile_area = unit * unit * MIN_TILE_REMS;

        // The element is measured once per frame; the model hands back the
        // already-tested geometry for exactly this rectangle.
        let (layout, focused) = {
            let model = self.model.read(cx);
            (model.treemap_layout(area, min_tile_area), model.focus_key())
        };

        let font_size = window.rem_size() * LABEL_REM;
        let line_height = font_size * LABEL_LINE_HEIGHT;
        // Labels are inset from the tile edge by half a line's leading.
        let inset = font_size * 0.5;
        let label_color = cx.theme().foreground;
        let dark = cx.theme().is_dark();

        let mut painted: Vec<PaintedTile> = Vec::with_capacity(layout.tiles.len());

        window.with_text_style(
            Some(TextStyleRefinement {
                color: Some(label_color),
                ..Default::default()
            }),
            |window| {
                for (rank, tile) in layout.tiles.iter().enumerate() {
                    let tile_bounds = Bounds {
                        origin: point(
                            bounds.origin.x + tile.rect.x.into(),
                            bounds.origin.y + tile.rect.y.into(),
                        ),
                        size: size(tile.rect.w.into(), tile.rect.h.into()),
                    };

                    let rank_ratio = if layout.tiles.len() <= 1 {
                        0.0
                    } else {
                        rank as f32 / (layout.tiles.len() - 1) as f32
                    };
                    let is_focused = tile.key.is_some() && tile.key == focused;
                    let base = if tile.is_aggregate() {
                        let muted = cx.theme().muted;
                        if dark {
                            muted.blend(cx.theme().chart_3.opacity(0.45))
                        } else {
                            muted
                        }
                    } else {
                        theme::tile_tint(cx, rank_ratio)
                    };
                    // Hover and keyboard focus are one state, so the highlight
                    // has to be distinctly brighter than rest.
                    let fill = if is_focused {
                        let mut lifted = base;
                        lifted.l = (lifted.l + 0.10).min(1.0);
                        lifted.a = (lifted.a + 0.18).min(1.0);
                        lifted
                    } else {
                        base
                    };

                    let available: Pixels = tile_bounds.size.width - inset * 2.;
                    let label = if available > font_size * LABEL_MIN_FONT_WIDTHS {
                        let text = Self::tile_label(tile);
                        let runs = vec![window.text_style().to_run(text.len())];
                        let (text, runs) = {
                            let mut wrapper = cx
                                .text_system()
                                .line_wrapper(window.text_style().font(), font_size);
                            wrapper.truncate_line(text.into(), available, "…", &runs, TruncateFrom::End)
                        };
                        Some(
                            window
                                .text_system()
                                .shape_line(text, font_size, &runs, None),
                        )
                    } else {
                        None
                    };

                    painted.push(PaintedTile {
                        key: tile.key,
                        is_dir: tile.is_dir,
                        is_aggregate: tile.is_aggregate(),
                        is_focused,
                        bounds: tile_bounds,
                        fill,
                        hitbox: window.insert_hitbox(tile_bounds, HitboxBehavior::Normal),
                        label,
                        label_origin: point(tile_bounds.origin.x + inset, tile_bounds.origin.y + inset),
                        line_height,
                    });
                }
            },
        );

        painted
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        tiles: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let radius = cx.theme().radius;
        let separator = cx.theme().background;
        let ring = cx.theme().primary;
        let transparent = cx.theme().transparent;

        for tile in tiles.iter() {
            let hitbox = tile.hitbox.clone();

            window.paint_quad(
                fill(tile.bounds, tile.fill)
                    .corner_radii(Corners::all(radius))
                    .border_widths(Edges::all(HAIRLINE))
                    .border_color(separator),
            );
            if tile.is_focused {
                window.paint_quad(
                    fill(tile.bounds, transparent)
                        .corner_radii(Corners::all(radius))
                        .border_widths(Edges::all(FOCUS_RING))
                        .border_color(ring),
                );
            }
            if let Some(label) = tile.label.as_ref() {
                let _ = label.paint(
                    tile.label_origin,
                    tile.line_height,
                    TextAlign::Left,
                    Some(tile.bounds.size.width),
                    window,
                    cx,
                );
            }

            // A directory reads as a destination: the pointer becomes a hand
            // only over content that navigates.
            if tile.is_dir && !tile.is_aggregate {
                window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
            }

            let Some(key) = tile.key else {
                continue;
            };
            let on_activate = Rc::clone(&self.on_activate);
            let on_focus = Rc::clone(&self.on_focus);
            let hover_hitbox = hitbox.clone();
            let click_hitbox = hitbox;

            window.on_mouse_event(move |_event: &MouseMoveEvent, phase, window, cx| {
                if !phase.bubble() || !hover_hitbox.is_hovered(window) {
                    return;
                }
                on_focus(key, window, cx);
            });

            window.on_mouse_event(move |_event: &MouseDownEvent, phase, window, cx| {
                if !phase.bubble() || !click_hitbox.is_hovered(window) {
                    return;
                }
                on_activate(key, window, cx);
            });
        }
    }
}
