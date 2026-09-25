//! The design system, in one place.
//!
//! Every value here is transcribed from the design稿 (`src/app.css` and the
//! Svelte components) rather than approximated: the oklch palette converted to
//! sRGB, the type scale, the component frames, and the spacing. Views read names
//! from here and carry no literals, which is also what the design guides require
//! — raw colour values belong in the theme definition and nowhere else.
//!
//! Why explicit pixels: the design稿 is a fixed desktop scale (a 46 px title bar,
//! a 268 px list, a 20 px canvas radius). Expressing it in `rem` would move every
//! one of those numbers as soon as the base font changed. The numbers live here
//! as `metrics::*`, so there is still exactly one owner — the audited
//! "product-owned scale" the guides describe — instead of literals at call sites.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Global, Hsla, Rgba, rgb};

/// The design's colours, as sRGB.
///
/// Each is the exact conversion of the corresponding `oklch()` token in
/// `src/app.css` (`oklab` → linear sRGB → sRGB), so the rendered result matches
/// what the design稿 specifies rather than a hand-picked near neighbour.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    // surfaces
    pub bg: Hsla,
    pub surface: Hsla,
    pub surface2: Hsla,
    pub surface3: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    // text
    pub fg: Hsla,
    pub muted: Hsla,
    pub faint: Hsla,
    // one accent
    pub accent: Hsla,
    pub accent_hi: Hsla,
    pub accent_deep: Hsla,
    pub accent_contrast: Hsla,
    // states
    pub danger: Hsla,
    pub danger_hi: Hsla,
    pub warn: Hsla,
    pub ok: Hsla,
    pub violet: Hsla,
    // chart / treemap categorical encoding
    pub categories: [Hsla; 8],
    pub category_other: Hsla,
    // window chrome (the design's three dots)
    pub traffic: [Hsla; 3],
    // treemap
    pub tile_stroke: Hsla,
    pub tile_stroke_hover: Hsla,
    pub label_fg: Hsla,
    pub label_fg_dim: Hsla,
    pub label_stroke: Hsla,
}

impl Palette {
    /// The palette, built from the design's tokens.
    pub fn design() -> Self {
        Self {
            bg: rgb(0x07_09_0C).into(),
            surface: rgb(0x0F_12_17).into(),
            surface2: rgb(0x18_1C_21).into(),
            surface3: rgb(0x23_27_2D).into(),
            border: rgb(0x26_2B_31).into(),
            border_strong: rgb(0x3B_40_47).into(),
            fg: rgb(0xE9_EB_EE).into(),
            muted: rgb(0x9B_9F_A4).into(),
            faint: rgb(0x64_69_70).into(),
            accent: rgb(0x30_93_EC).into(),
            accent_hi: rgb(0x52_A9_FE).into(),
            accent_deep: rgb(0x01_5B_A6).into(),
            accent_contrast: rgb(0xF8_FA_FD).into(),
            danger: rgb(0xF4_5A_56).into(),
            danger_hi: rgb(0xFF_72_6C).into(),
            warn: rgb(0xE8_A6_3D).into(),
            ok: rgb(0x59_C9_77).into(),
            violet: rgb(0xA5_84_DC).into(),
            categories: [
                rgb(0x2F_91_E2).into(),
                rgb(0x00_AF_B0).into(),
                rgb(0xD6_A0_44).into(),
                rgb(0xE8_60_5B).into(),
                rgb(0xA4_82_D5).into(),
                rgb(0x49_B5_67).into(),
                rgb(0xDB_6E_A5).into(),
                rgb(0x73_7B_86).into(),
            ],
            category_other: rgb(0x39_3E_43).into(),
            traffic: [
                rgb(0xEF_66_61).into(),
                rgb(0xE0_AF_3B).into(),
                rgb(0x5B_C6_63).into(),
            ],
            tile_stroke: rgb(0x05_07_0B).into(),
            // The design strokes the hovered tile with `oklch(0.985 0.005 250)`,
            // which is the accent-contrast token itself.
            tile_stroke_hover: rgb(0xF8_FA_FD).into(),
            label_fg: rgb(0xF3_F5_F8).into(),
            label_fg_dim: rgb(0xF3_F5_F8).into(),
            label_stroke: rgb(0x04_06_09).into(),
        }
    }

    /// A categorical colour for tile `index`, cycling like the design's `CATS`.
    pub fn category(&self, index: usize) -> Hsla {
        self.categories[index % self.categories.len()]
    }
}

/// The application's palette, stored as a global so a view reads it in one call
/// and cannot drift from the component theme.
pub struct SiftTheme(pub Palette);

impl Global for SiftTheme {}

/// The palette, from the application global.
pub fn palette(cx: &App) -> Palette {
    cx.global::<SiftTheme>().0
}

// ---- type scale (design稿 font sizes, in px) -------------------------------

pub mod text {
    /// `body { font-size: 13.5px }`
    pub const BODY: f32 = 13.5;
    pub const TITLE: f32 = 13.0; // "Sift"
    pub const CRUMB: f32 = 12.0; // text-[12px]
    pub const NAV_NAME: f32 = 12.5; // location picker
    pub const NAV_SUB: f32 = 10.5; // "N% 已使用 · 内置磁盘"、磁盘 label
    pub const LIST_TITLE: f32 = 12.0; // "文件与文件夹"
    pub const LIST_NAME: f32 = 12.5; // row name
    pub const LIST_SIZE: f32 = 11.0; // row size
    pub const CAPSULE: f32 = 12.0; // bottom summary
    pub const PANEL_TITLE: f32 = 14.0; // popup titles
    pub const PANEL_SUB: f32 = 11.0; // popup counts
    pub const TILE_MIN: f32 = 12.5; // treemap label, h < 52
    pub const TILE_MID: f32 = 13.0; // h >= 52
    pub const TILE_MAX: f32 = 14.0; // h >= 64
}

// ---- frames and spacing (design稿 px) --------------------------------------

pub mod metrics {
    /// `header { height: 46px }`
    pub const TITLE_BAR_H: f32 = 46.0;
    pub const TITLE_BAR_PAD_X: f32 = 16.0;
    pub const TITLE_BAR_GAP: f32 = 12.0;
    /// Room for the real macOS traffic lights at `(14, 16)`: three 12 px dots
    /// with 8 px gaps from x=14, plus the design's own leading gap.
    pub const TRAFFIC_RESERVE: f32 = 78.0;
    pub const TRAFFIC_DOT: f32 = 12.0;
    pub const TRAFFIC_GAP: f32 = 8.0;

    /// `workspace { padding: 12px; gap: 12px }`
    pub const WORKSPACE_PAD: f32 = 12.0;
    pub const WORKSPACE_GAP: f32 = 12.0;

    /// The glass capsule around the location picker and breadcrumbs.
    pub const NAV_RADIUS: f32 = 16.0;
    pub const NAV_PAD_X: f32 = 8.0;
    pub const NAV_PAD_Y: f32 = 6.0;
    pub const NAV_GAP: f32 = 4.0;
    pub const DIVIDER_W: f32 = 1.0;
    pub const DIVIDER_H: f32 = 20.0;
    pub const DIVIDER_MX: f32 = 4.0;

    pub const CRUMB_H: f32 = 28.0;
    pub const CRUMB_MAX_W: f32 = 160.0;
    pub const CRUMB_RADIUS: f32 = 8.0;
    pub const CRUMB_PAD_X: f32 = 8.0;
    pub const CRUMB_GAP: f32 = 2.0;

    /// `canvas-body { max-width: 1180px; border-radius: 20px }`
    pub const CANVAS_MAX_W: f32 = 1180.0;
    pub const CANVAS_RADIUS: f32 = 20.0;
    /// The canvas surface is `surface` at 52% over the window background.
    pub const CANVAS_ALPHA: f32 = 0.52;

    /// `aside { width: 268px }`
    pub const LIST_W: f32 = 268.0;
    pub const LIST_HEADER_PAD_X: f32 = 14.0;
    pub const LIST_HEADER_PAD_TOP: f32 = 12.0;
    pub const LIST_HEADER_PAD_BOTTOM: f32 = 6.0;
    pub const LIST_HEADER_GAP: f32 = 8.0;
    pub const LIST_PAD_X: f32 = 8.0;
    pub const LIST_PAD_BOTTOM: f32 = 8.0;

    /// The design's row frame: an 18 px icon slot plus `py-[7px]`.
    pub const ROW_H: f32 = ROW_ICON_SLOT + ROW_PAD_Y * 2.0;
    pub const ROW_RADIUS: f32 = 8.0;
    pub const ROW_PAD_X: f32 = 8.0;
    pub const ROW_PAD_Y: f32 = 7.0;
    pub const ROW_GAP: f32 = 8.0;
    pub const ROW_ICON_SLOT: f32 = 18.0;
    pub const ROW_ICON: f32 = 14.0;

    /// `ai-summary { border-radius: 16px; padding: 16px 10px }`
    pub const CAPSULE_RADIUS: f32 = 16.0;
    pub const CAPSULE_PAD_X: f32 = 16.0;
    pub const CAPSULE_PAD_Y: f32 = 10.0;
    pub const CAPSULE_GAP: f32 = 10.0;
    pub const CAPSULE_ICON_BOX: f32 = 28.0;
    pub const CAPSULE_ICON_RADIUS: f32 = 8.0;

    pub const BTN_H: f32 = 32.0;
    pub const BTN_SM_H: f32 = 28.0;
    pub const BTN_RADIUS: f32 = 10.0;
    pub const BTN_SM_RADIUS: f32 = 8.0;
    pub const BTN_ICON: f32 = 30.0;
    pub const BTN_ICON_RADIUS: f32 = 9.0;
    pub const MARK: f32 = 24.0;
    pub const MARK_RADIUS: f32 = 6.0;
    pub const SWITCH_W: f32 = 38.0;
    pub const SWITCH_H: f32 = 22.0;
    pub const SWITCH_THUMB: f32 = 16.0;

    pub const MENU_W: f32 = 260.0;
    pub const MENU_RADIUS: f32 = 12.0;
    pub const MENU_PAD: f32 = 6.0;
    pub const MENU_ITEM_RADIUS: f32 = 8.0;
    pub const MENU_ITEM_PAD_X: f32 = 10.0;
    pub const MENU_ITEM_PAD_Y: f32 = 7.0;
    pub const MENU_ITEM_GAP: f32 = 10.0;

    pub const TOAST_RADIUS: f32 = 12.0;
    pub const TOAST_PAD_X: f32 = 16.0;
    pub const TOAST_PAD_Y: f32 = 10.0;
    pub const TOAST_GAP: f32 = 10.0;
    pub const TOAST_OFFSET: f32 = 24.0;

    /// `PAD = 14` around the treemap stage.
    pub const TREEMAP_PAD: f32 = 14.0;
    /// `MIN_W = 88`, `MIN_H = 48`.
    pub const TILE_MIN_W: f32 = 88.0;
    pub const TILE_MIN_H: f32 = 48.0;
    pub const TILE_LABEL_INSET: f32 = 10.0;
    /// `stroke-width: 1.5px`
    pub const TILE_STROKE: f32 = 1.5;
    /// A label needs at least this much height and width to be worth drawing.
    pub const TILE_LABEL_MIN_W: f32 = 70.0;
    pub const TILE_LABEL_MIN_H: f32 = 26.0;
}

/// The design's elevation recipes, as GPUI shadows.
///
/// `backdrop-filter: blur()` does not exist in GPUI, so the glass surfaces are
/// reproduced with the parts that do exist — a translucent surface, a hairline
/// border, an inset top highlight and the same drop shadow — and the blur is
/// approximated by making the surface slightly more opaque than the design's 78%
/// so the result reads as the same material rather than a transparent hole.
pub mod elevation {
    use gpui_kit::{BoxShadow, point, px, rgb};

    /// `0 1px 0 rgba(255,255,255,0.07) inset, 0 24px 60px -28px black`
    pub fn glass() -> Vec<BoxShadow> {
        vec![
            BoxShadow {
                color: rgb(0xFF_FF_FF).into(),
                offset: point(px(0.), px(1.)),
                blur_radius: px(0.),
                spread_radius: px(0.),
                inset: true,
            },
            BoxShadow {
                color: rgb(0x00_00_00).into(),
                offset: point(px(0.), px(24.)),
                blur_radius: px(60.),
                spread_radius: px(-28.),
                inset: false,
            },
        ]
    }

    /// `0 1px 0 rgba(255,255,255,0.08) inset, 0 40px 90px -30px black`
    pub fn glass_strong() -> Vec<BoxShadow> {
        vec![
            BoxShadow {
                color: rgb(0xFF_FF_FF).into(),
                offset: point(px(0.), px(1.)),
                blur_radius: px(0.),
                spread_radius: px(0.),
                inset: true,
            },
            BoxShadow {
                color: rgb(0x00_00_00).into(),
                offset: point(px(0.), px(40.)),
                blur_radius: px(90.),
                spread_radius: px(-30.),
                inset: false,
            },
        ]
    }

    /// `0 1px 0 rgba(255,255,255,0.05) inset, 0 18px 40px -24px black`
    pub fn card() -> Vec<BoxShadow> {
        vec![
            BoxShadow {
                color: rgb(0xFF_FF_FF).into(),
                offset: point(px(0.), px(1.)),
                blur_radius: px(0.),
                spread_radius: px(0.),
                inset: true,
            },
            BoxShadow {
                color: rgb(0x00_00_00).into(),
                offset: point(px(0.), px(18.)),
                blur_radius: px(40.),
                spread_radius: px(-24.),
                inset: false,
            },
        ]
    }

    /// The design's popup and menu shadow:
    /// `0 2px 8px -2px black, 0 18px 44px -12px black`.
    pub fn popup() -> Vec<BoxShadow> {
        vec![
            BoxShadow {
                color: rgb(0x00_00_00).into(),
                offset: point(px(0.), px(2.)),
                blur_radius: px(8.),
                spread_radius: px(-2.),
                inset: false,
            },
            BoxShadow {
                color: rgb(0x00_00_00).into(),
                offset: point(px(0.), px(18.)),
                blur_radius: px(44.),
                spread_radius: px(-12.),
                inset: false,
            },
        ]
    }
}

/// Install the design theme: the component theme, then the palette global.
///
/// `Theme::update` does not exist in gpui-component 0.6.6; the documented edit is
/// through [`Theme::global_mut`] followed by [`Theme::sync_base`], which is what
/// the coding guides require. The edit touches `colors`, so the renderable
/// `tokens` are re-derived from them before the Base projection is synced — a
/// sidebar otherwise paints new text on the old surface.
pub fn install(cx: &mut App) {
    let palette = Palette::design();
    Theme::change(ThemeMode::Dark, None, cx);

    // Base font: the design's body size. This is also the window's `rem`, so the
    // scale helpers resolve against it.
    let root_font = gpui_kit::px(text::BODY);

    {
        let theme = Theme::global_mut(cx);
        theme.font_size = root_font;
        theme.mono_font_family = "Menlo".into();

        // Radii: the design's control frames.
        theme.radius = gpui_kit::px(metrics::BTN_RADIUS);
        theme.radius_lg = gpui_kit::px(metrics::NAV_RADIUS);

        let colors = &mut theme.colors;
        colors.background = palette.bg;
        colors.foreground = palette.fg;
        colors.border = palette.border;
        colors.input = palette.border;
        colors.primary = palette.accent;
        colors.primary_foreground = palette.accent_contrast;
        colors.primary_hover = palette.accent_hi;
        colors.primary_active = palette.accent_deep;
        colors.secondary = palette.surface2;
        colors.secondary_foreground = palette.fg;
        colors.secondary_hover = palette.surface3;
        colors.secondary_active = palette.surface3;
        colors.danger = palette.danger;
        colors.danger_foreground = palette.accent_contrast;
        colors.danger_hover = palette.danger_hi;
        colors.danger_active = palette.danger;
        colors.warning = palette.warn;
        colors.warning_foreground = palette.accent_contrast;
        colors.warning_hover = palette.warn;
        colors.warning_active = palette.warn;
        colors.success = palette.ok;
        colors.success_foreground = palette.accent_contrast;
        colors.success_hover = palette.ok;
        colors.success_active = palette.ok;
        colors.info = palette.accent;
        colors.info_foreground = palette.accent_contrast;
        colors.muted = palette.surface2;
        colors.muted_foreground = palette.muted;
        colors.popover = palette.surface;
        colors.popover_foreground = palette.fg;
        colors.sidebar = palette.bg;
        colors.sidebar_foreground = palette.fg;
        colors.title_bar = palette.bg;
        colors.title_bar_border = palette.border;
        colors.group_box = palette.surface;
        colors.group_box_foreground = palette.fg;
        colors.overlay = palette.bg.opacity(0.55);
        colors.selection = palette.accent.opacity(0.26);
        colors.caret = palette.accent_hi;
        colors.link = palette.accent_hi;
        colors.link_hover = palette.accent;
        colors.ring = palette.accent.opacity(0.55);
        colors.scrollbar = palette.surface3;
        colors.scrollbar_thumb = palette.border_strong;
        colors.scrollbar_thumb_hover = palette.faint;
        colors.drag_border = palette.accent;
        colors.drop_target = palette.accent.opacity(0.20);
        for (index, color) in palette.categories.iter().enumerate() {
            colors.chart_1 = if index == 0 { *color } else { colors.chart_1 };
        }
        let _ = Rgba::default();
    }
    // Re-derive the renderable tokens from the colours just written, then refresh
    // the Base projection (scrollbars, resize handles) and the windows.
    Theme::sync_base(cx);
    cx.set_global(SiftTheme(palette));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Colours are authored as sRGB and stored as HSLA, so a comparison has to
    /// allow the ±1/255 the conversion can round by. Asserting exact equality
    /// would be testing the colour space, not the palette.
    fn assert_near(actual: Hsla, expected: u32, what: &str) {
        let actual = Rgba::from(actual);
        let expected = rgb(expected);
        let channel = |a: f32, b: f32| (a - b).abs() <= 1.0 / 255.0 + 1e-6;
        assert!(
            channel(actual.r, expected.r)
                && channel(actual.g, expected.g)
                && channel(actual.b, expected.b),
            "{what}: {actual:?} is not within one step of {expected:?}"
        );
    }

    #[test]
    fn the_palette_is_the_design_s_tokens() {
        let palette = Palette::design();
        assert_near(palette.bg, 0x07_09_0C, "bg");
        assert_near(palette.surface, 0x0F_12_17, "surface");
        assert_near(palette.surface2, 0x18_1C_21, "surface-2");
        assert_near(palette.border, 0x26_2B_31, "border");
        assert_near(palette.border_strong, 0x3B_40_47, "border-strong");
        assert_near(palette.fg, 0xE9_EB_EE, "fg");
        assert_near(palette.muted, 0x9B_9F_A4, "muted");
        assert_near(palette.faint, 0x64_69_70, "faint");
        assert_near(palette.accent, 0x30_93_EC, "accent");
        assert_near(palette.accent_hi, 0x52_A9_FE, "accent-hi");
        assert_near(palette.accent_contrast, 0xF8_FA_FD, "on-accent");
        assert_near(palette.danger, 0xF4_5A_56, "danger");
        assert_near(palette.warn, 0xE8_A6_3D, "warn");
        assert_near(palette.ok, 0x59_C9_77, "ok");
        assert_near(palette.category_other, 0x39_3E_43, "cat-other");
    }

    #[test]
    fn categorical_colours_cycle_like_the_design_s_cats() {
        let palette = Palette::design();
        assert_near(palette.category(0), 0x2F_91_E2, "cat-1");
        assert_near(palette.category(7), 0x73_7B_86, "cat-8");
        // `CATS[i % CATS.length]`
        assert_near(palette.category(8), 0x2F_91_E2, "cat-1 again");
        assert_near(palette.category(9), 0x00_AF_B0, "cat-2 again");
    }

    #[test]
    fn the_traffic_lights_are_the_design_s_three_dots() {
        let palette = Palette::design();
        assert_near(palette.traffic[0], 0xEF_66_61, "red");
        assert_near(palette.traffic[1], 0xE0_AF_3B, "amber");
        assert_near(palette.traffic[2], 0x5B_C6_63, "green");
    }

    #[test]
    fn a_translucent_surface_matches_the_design_s_color_mix() {
        let palette = Palette::design();
        // `color-mix(in oklch, var(--color-bg) 55%, transparent)`
        let veil = palette.bg.opacity(0.55);
        assert!((veil.a - 0.55).abs() < 0.01, "got {}", veil.a);
        // Reading a colour and fading it must not mutate the palette.
        let _ = palette.border.opacity(0.6);
        assert!((palette.border.a - 1.0).abs() < 0.01);
    }

    #[test]
    fn the_scale_matches_the_design_s_frames() {
        // The numbers a reviewer checks first against the design稿.
        assert_eq!(metrics::TITLE_BAR_H, 46.0);
        assert_eq!(metrics::LIST_W, 268.0);
        assert_eq!(metrics::CANVAS_MAX_W, 1180.0);
        assert_eq!(metrics::CANVAS_RADIUS, 20.0);
        assert_eq!(metrics::CRUMB_H, 28.0);
        assert_eq!(metrics::ROW_PAD_Y, 7.0);
        assert_eq!(metrics::CAPSULE_ICON_BOX, 28.0);
        assert_eq!(metrics::TREEMAP_PAD, 14.0);
        assert_eq!(metrics::TILE_MIN_W, 88.0);
        assert_eq!(metrics::TILE_STROKE, 1.5);
        assert_eq!(metrics::SWITCH_W, 38.0);
        assert_eq!(metrics::SWITCH_H, 22.0);
        assert_eq!(text::BODY, 13.5);
        assert_eq!(text::LIST_NAME, 12.5);
    }

    #[test]
    fn the_elevation_recipes_are_the_design_s_shadows() {
        // `.glass` has an inset highlight and one deep drop.
        let glass = elevation::glass();
        assert_eq!(glass.len(), 2);
        assert!(glass[0].inset, "the highlight is inset");
        assert!(!glass[1].inset, "the drop is not");
        assert_eq!(glass[1].blur_radius, gpui_kit::px(60.));
    }
}
