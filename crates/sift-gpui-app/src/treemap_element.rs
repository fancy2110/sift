//! The treemap, painted as the design稿 specifies.
//!
//! A custom [`Element`] because the tiles are resolved geometry: GPUI Kit has no
//! treemap, and a tree of `div`s would give up the single measured area the
//! squarified layout needs.
//!
//! What is matched from the design (`src/lib/components/Treemap.svelte`):
//!
//! * **Flat rectangles**, `rx = 0` — no corner radius on a tile.
//! * **A diagonal gradient fill per tile**, from the categorical colour at 92%
//!   to the same colour at 62% (`x1,y1 → x2,y2`, i.e. 135°).
//! * **Categorical colours by index** (`CATS[i % 8]`), not a rank tint; the
//!   long-tail aggregate tile uses `--color-cat-other`.
//! * **A 1.5 px border**, `oklch(0.13 0.01 255 / 0.55)` at rest and
//!   `oklch(0.985 0.005 250)` when focused.
//! * **Non-focused tiles dim to 30%** once anything is focused.
//! * **One label row per tile**, centred vertically: the name at `x + 10` and
//!   the size flush right at `x + w - 10`, drawn only when the tile is at least
//!   70 × 26, with the name truncated so it can never collide with the size.
//!
//! One deliberate difference: the design strokes its labels with a 2.5 px dark
//! outline for legibility over a light tile. GPUI's text pipeline has no glyph
//! outline, so the label is painted once in the design's own fill colour
//! (`oklch(0.97 0.004 255 / 0.92)`). Everything else is as drawn.

use std::panic::Location;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::{
    App, Bounds, Corners, CursorStyle, Edges, Element, ElementId, FontWeight,
    GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, LayoutId, MouseDownEvent,
    MouseMoveEvent, Pixels, Point, ShapedLine, Style, TextAlign, TextStyleRefinement, TruncateFrom,
    Window, fill, linear_color_stop, linear_gradient, point, px, relative, size,
};

use sift_core::{format_bytes, NodeKey};

use crate::model::{Tile, WorkspaceModel};
use crate::theme::{self, metrics, text};

/// The design's gradient: the same hue, brighter at the top-left.
const GRADIENT_FROM_ALPHA: f32 = 0.92;
const GRADIENT_TO_ALPHA: f32 = 0.62;
/// `filter: brightness(1.12)` on hover, approximated by lifting the stops.
const HOVER_LIFT: f32 = 0.12;
/// `opacity: 0.3` for tiles that are not the focused one.
const DIM_ALPHA: f32 = 0.30;
/// The gap the design leaves between a tile's name and its size.
const LABEL_SIZE_GAP: f32 = 8.0;
const LABEL_LINE_HEIGHT: f32 = 1.35;

/// One painted tile: everything `paint` needs, resolved during `prepaint`.
pub struct PaintedTile {
    key: Option<NodeKey>,
    is_dir: bool,
    is_aggregate: bool,
    is_focused: bool,
    /// The tile's own colour; the gradient and any dimming are derived from it at
    /// paint time so hover and focus cannot disagree with the fill.
    color: Hsla,
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    label: Option<ShapedLine>,
    label_origin: Point<Pixels>,
    size_text: Option<ShapedLine>,
    size_origin: Point<Pixels>,
    line_height: Pixels,
}

/// A callback the element invokes with the tile the user acted on.
pub type TileCallback = Rc<dyn Fn(NodeKey, &mut Window, &mut App)>;

/// The treemap element.
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

    /// The design's `labelSize`: the taller the tile, the larger the type.
    fn label_size(tile: &Tile) -> f32 {
        if tile.rect.h >= 64.0 {
            text::TILE_MAX
        } else if tile.rect.h >= 52.0 {
            text::TILE_MID
        } else {
            text::TILE_MIN
        }
    }

    /// The design's `rowLabel.raw`: the aggregate tile names its count.
    fn label_text(tile: &Tile) -> String {
        match (tile.is_aggregate(), tile.folded_count) {
            (true, Some(count)) => format!("其他 · {count} 项"),
            _ => tile.label.clone(),
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
        // The stage fills whatever the canvas gives it.
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
        let palette = theme::palette(cx);
        let unit: f32 = window.rem_size().into();
        let _ = unit;

        // The design pads the stage by 14 px and lays the tiles out inside it.
        let pad = metrics::TREEMAP_PAD;
        let area = crate::model::Rect::new(
            bounds.origin.x.as_f32() + pad,
            bounds.origin.y.as_f32() + pad,
            (bounds.size.width.as_f32() - pad * 2.0).max(0.0),
            (bounds.size.height.as_f32() - pad * 2.0).max(0.0),
        );
        // A tile below the design's minimum for a label is still drawn; the
        // layout folds the long tail before it gets that small.
        let min_tile_area = metrics::TILE_MIN_W * metrics::TILE_MIN_H;

        let (layout, focused) = {
            let model = self.model.read(cx);
            (model.treemap_layout(area, min_tile_area), model.focus_key())
        };

        let mut painted: Vec<PaintedTile> = Vec::with_capacity(layout.tiles.len());
        let any_focused = focused.is_some() && layout.tiles.iter().any(|tile| tile.key == focused);

        for (index, tile) in layout.tiles.iter().enumerate() {
            if tile.rect.w <= 0.0 || tile.rect.h <= 0.0 {
                continue;
            }
            // `squarify` writes rects in the coordinate space of the area it was
            // given, and that area is already absolute, so a tile rect needs no
            // further origin added.
            let tile_bounds = Bounds {
                origin: point(px(tile.rect.x), px(tile.rect.y)),
                size: size(px(tile.rect.w), px(tile.rect.h)),
            };

            let is_focused = tile.key.is_some() && tile.key == focused;
            let color = if tile.is_aggregate() {
                palette.category_other
            } else {
                palette.category(index)
            };

            let mut label = None;
            let mut label_origin = point(tile_bounds.origin.x, tile_bounds.origin.y);
            let mut size_text = None;
            let mut size_origin = point(tile_bounds.origin.x, tile_bounds.origin.y);
            let mut line_height = px(0.);

            // The design's threshold: `t.w >= 70 && t.h >= 26`.
            if tile.rect.w >= metrics::TILE_LABEL_MIN_W
                && tile.rect.h >= metrics::TILE_LABEL_MIN_H
            {
                let font_size = px(Self::label_size(tile));
                line_height = font_size * LABEL_LINE_HEIGHT;
                // Vertically centred, as the design's baseline formula does.
                let top = (tile_bounds.origin.y
                    + (tile_bounds.size.height - line_height) / 2.0)
                    .max(tile_bounds.origin.y);
                let inset = px(metrics::TILE_LABEL_INSET);

                let name_run = |color: Hsla, len: usize, window: &mut Window| -> Vec<gpui_kit::TextRun> {
                    window.with_text_style(
                        Some(TextStyleRefinement {
                            color: Some(color),
                            font_weight: Some(FontWeight::SEMIBOLD),
                            ..Default::default()
                        }),
                        |window| vec![window.text_style().to_run(len)],
                    )
                };

                // The size first: it is never truncated, so it defines the room
                // the name may use.
                let size_width = if tile.is_aggregate() {
                    px(0.)
                } else {
                    let raw = format_bytes(tile.size);
                    let runs = {
                        let style_color = palette.label_fg.opacity(0.75);
                        window.with_text_style(
                            Some(TextStyleRefinement {
                                color: Some(style_color),
                                font_weight: Some(FontWeight::NORMAL),
                                ..Default::default()
                            }),
                            |window| vec![window.text_style().to_run(raw.len())],
                        )
                    };
                    let shaped =
                        window
                            .text_system()
                            .shape_line(raw.into(), font_size * 0.92, &runs, None);
                    let width = shaped.width;
                    size_origin = point(
                        tile_bounds.origin.x + tile_bounds.size.width - inset - width,
                        top,
                    );
                    size_text = Some(shaped);
                    width
                };

                let budget = tile_bounds.size.width - inset * 2.0 - size_width - px(LABEL_SIZE_GAP);
                if budget > font_size * 2.0 {
                    let raw = Self::label_text(tile);
                    let runs = name_run(palette.label_fg.opacity(0.92), raw.len(), window);
                    let (text, runs) = {
                        let mut wrapper = cx
                            .text_system()
                            .line_wrapper(window.text_style().font(), font_size);
                        wrapper.truncate_line(
                            raw.into(),
                            budget,
                            "…",
                            &runs,
                            TruncateFrom::End,
                        )
                    };
                    label = Some(window.text_system().shape_line(text, font_size, &runs, None));
                    label_origin = point(tile_bounds.origin.x + inset, top);
                }
            }

            painted.push(PaintedTile {
                key: tile.key,
                is_dir: tile.is_dir,
                is_aggregate: tile.is_aggregate(),
                is_focused,
                color,
                bounds: tile_bounds,
                hitbox: window.insert_hitbox(tile_bounds, HitboxBehavior::Normal),
                label,
                label_origin,
                size_text,
                size_origin,
                line_height,
            });
        }

        let _ = any_focused;
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
        let palette = theme::palette(cx);
        let any_focused = tiles.iter().any(|tile| tile.is_focused);

        for tile in tiles.iter() {
            // The design dims every tile except the focused one, and lifts the
            // focused tile's fill by `brightness(1.12)`.
            let (from_alpha, to_alpha, lift) = if tile.is_focused {
                (GRADIENT_FROM_ALPHA, GRADIENT_TO_ALPHA, HOVER_LIFT)
            } else if any_focused {
                (
                    GRADIENT_FROM_ALPHA * DIM_ALPHA,
                    GRADIENT_TO_ALPHA * DIM_ALPHA,
                    0.0,
                )
            } else {
                (GRADIENT_FROM_ALPHA, GRADIENT_TO_ALPHA, 0.0)
            };

            let mut base = tile.color;
            if lift > 0.0 {
                base.l = (base.l + lift).min(1.0);
            }
            let stroke = if tile.is_focused {
                palette.tile_stroke_hover
            } else if any_focused {
                palette.tile_stroke.opacity(0.55 * DIM_ALPHA)
            } else {
                palette.tile_stroke.opacity(0.55)
            };

            // `linear-gradient(x1 0, y1 0 → x2 1, y2 1)` is a 135° gradient.
            let background = linear_gradient(
                135.,
                linear_color_stop(base.opacity(from_alpha), 0.),
                linear_color_stop(base.opacity(to_alpha), 1.),
            );

            window.paint_quad(
                fill(tile.bounds, background)
                    // The design sets no radius on a tile (`rx="0"`).
                    .corner_radii(Corners::all(px(0.)))
                    .border_widths(Edges::all(px(metrics::TILE_STROKE)))
                    .border_color(stroke),
            );

            // Interaction: the pointer owns hover, the keyboard owns activation.
            let hitbox = tile.hitbox.clone();
            if let Some(key) = tile.key {
                if hitbox.is_hovered(window) {
                    window.set_cursor_style(
                        if tile.is_dir {
                            CursorStyle::PointingHand
                        } else {
                            CursorStyle::Arrow
                        },
                        &hitbox,
                    );
                }
                let on_focus = Rc::clone(&self.on_focus);
                let on_activate = Rc::clone(&self.on_activate);
                let hover_hitbox = hitbox.clone();
                let click_hitbox = hitbox.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if !phase.bubble() || !hover_hitbox.is_hovered(window) {
                        return;
                    }
                    on_focus(key, window, cx);
                    let _ = event;
                });
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if !phase.bubble() || !click_hitbox.is_hovered(window) {
                        return;
                    }
                    if event.button == gpui_kit::MouseButton::Left {
                        on_activate(key, window, cx);
                    }
                });
            }

            if let Some(label) = tile.label.as_ref() {
                let _ = label.paint(
                    tile.label_origin,
                    tile.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            if let Some(size_text) = tile.size_text.as_ref() {
                let _ = size_text.paint(
                    tile.size_origin,
                    tile.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rect;

    fn tile(label: &str, w: f32, h: f32, aggregate: Option<usize>) -> Tile {
        Tile {
            key: if aggregate.is_some() {
                None
            } else {
                Some(NodeKey::from_bytes(label.as_bytes()))
            },
            label: label.to_string(),
            rect: Rect::new(0.0, 0.0, w, h),
            size: 1024,
            is_dir: true,
            folded_count: aggregate,
        }
    }

    #[test]
    fn label_size_follows_the_design_s_height_tiers() {
        assert_eq!(TreemapElement::label_size(&tile("a", 200.0, 40.0, None)), 12.5);
        assert_eq!(TreemapElement::label_size(&tile("a", 200.0, 52.0, None)), 13.0);
        assert_eq!(TreemapElement::label_size(&tile("a", 200.0, 64.0, None)), 14.0);
        assert_eq!(TreemapElement::label_size(&tile("a", 200.0, 120.0, None)), 14.0);
    }

    #[test]
    fn the_aggregate_tile_names_its_count() {
        assert_eq!(
            TreemapElement::label_text(&tile("", 100.0, 30.0, Some(42))),
            "其他 · 42 项"
        );
        assert_eq!(
            TreemapElement::label_text(&tile("node_modules", 100.0, 30.0, None)),
            "node_modules"
        );
    }

    #[test]
    fn the_design_s_gradient_and_dim_factors_are_intact() {
        // These are the numbers a reviewer compares against the design稿.
        assert!((GRADIENT_FROM_ALPHA - 0.92).abs() < 1e-6);
        assert!((GRADIENT_TO_ALPHA - 0.62).abs() < 1e-6);
        assert!((DIM_ALPHA - 0.30).abs() < 1e-6);
        assert!((HOVER_LIFT - 0.12).abs() < 1e-6);
    }
}
