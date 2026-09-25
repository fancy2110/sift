//! The application theme: deep ink surfaces with one azure accent.
//!
//! This module is the only place in the application that names a raw colour.
//! Every call site reads a semantic role from `cx.theme()`; this file exists so
//! that the palette has one owner and a dark/light switch cannot leave a
//! component painting a half-migrated surface.
//!
//! `Theme::update(cx, ...)` does not exist in gpui-component 0.6.6. The
//! documented equivalent is an edit through [`Theme::global_mut`] followed by
//! [`Theme::sync_base`], which is what the coding guide requires: the edit
//! touches `colors`, so this module also re-derives the renderable `tokens`
//! from those colours before syncing the Base projection.

use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, ThemeTokens};
use gpui_kit::{App, hsla, px};

/// Install the Sift theme as the application's only theme.
///
/// Call once, after `gpui_kit::init(cx)` and before opening the window, so the
/// first frame is already themed.
pub fn install(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);

    // ---- surfaces ---------------------------------------------------------
    let background = hsla(224. / 360., 0.20, 0.075, 1.);
    let foreground = hsla(220. / 360., 0.16, 0.90, 1.);
    let muted = hsla(224. / 360., 0.16, 0.14, 1.);
    let muted_foreground = hsla(222. / 360., 0.10, 0.58, 1.);
    let border = hsla(224. / 360., 0.14, 0.185, 1.);
    let panel = hsla(224. / 360., 0.19, 0.105, 1.);
    let popover = hsla(224. / 360., 0.19, 0.115, 1.);
    let input = hsla(224. / 360., 0.16, 0.15, 1.);

    // ---- one azure accent -------------------------------------------------
    let accent = hsla(205. / 360., 0.90, 0.55, 1.);
    let accent_hover = hsla(205. / 360., 0.90, 0.62, 1.);
    let accent_active = hsla(205. / 360., 0.90, 0.47, 1.);
    // Dark ink on azure: the accent is light enough that dark text reads and
    // white text does not.
    let on_accent = hsla(212. / 360., 0.45, 0.08, 1.);

    let secondary = hsla(224. / 360., 0.14, 0.18, 1.);
    let secondary_hover = hsla(224. / 360., 0.14, 0.23, 1.);
    let secondary_active = hsla(224. / 360., 0.14, 0.27, 1.);
    let accent_soft = hsla(205. / 360., 0.38, 0.22, 1.);
    let accent_soft_foreground = hsla(205. / 360., 0.34, 0.92, 1.);

    // ---- semantic states --------------------------------------------------
    let danger = hsla(2. / 360., 0.72, 0.58, 1.);
    let danger_hover = hsla(2. / 360., 0.72, 0.64, 1.);
    let danger_active = hsla(2. / 360., 0.72, 0.50, 1.);
    let warning = hsla(38. / 360., 0.92, 0.58, 1.);
    let warning_hover = hsla(38. / 360., 0.92, 0.64, 1.);
    let warning_active = hsla(38. / 360., 0.92, 0.50, 1.);
    let success = hsla(152. / 360., 0.52, 0.46, 1.);
    let success_hover = hsla(152. / 360., 0.52, 0.52, 1.);
    let success_active = hsla(152. / 360., 0.52, 0.39, 1.);
    let on_state = hsla(220. / 360., 0.40, 0.07, 1.);

    let row_hover = hsla(224. / 360., 0.16, 0.16, 1.);
    let row_active = hsla(205. / 360., 0.30, 0.22, 1.);
    let selection = hsla(205. / 360., 0.80, 0.55, 0.26);
    let chart_1 = hsla(205. / 360., 0.86, 0.58, 1.);
    let chart_2 = hsla(199. / 360., 0.72, 0.50, 1.);
    let chart_3 = hsla(212. / 360., 0.62, 0.47, 1.);
    let chart_4 = hsla(190. / 360., 0.55, 0.44, 1.);
    let chart_5 = hsla(222. / 360., 0.45, 0.42, 1.);

    {
        let theme = Theme::global_mut(cx);

        // A compact, precise radius scale; surfaces read one tier softer than
        // the controls they contain.
        theme.radius = px(6.);
        theme.radius_lg = px(10.);

        let colors = &mut theme.colors;
        colors.background = background;
        colors.foreground = foreground;

        colors.muted = muted;
        colors.muted_foreground = muted_foreground;
        colors.border = border;
        colors.input = input;
        colors.popover = popover;
        colors.popover_foreground = foreground;
        colors.overlay = hsla(224. / 360., 0.30, 0.03, 0.55);
        colors.window_border = border;
        colors.drag_border = accent;

        colors.primary = accent;
        colors.primary_hover = accent_hover;
        colors.primary_active = accent_active;
        colors.primary_foreground = on_accent;
        colors.ring = accent;
        colors.selection = selection;

        colors.secondary = secondary;
        colors.secondary_hover = secondary_hover;
        colors.secondary_active = secondary_active;
        colors.secondary_foreground = foreground;

        colors.accent = accent_soft;
        colors.accent_foreground = accent_soft_foreground;

        colors.danger = danger;
        colors.danger_hover = danger_hover;
        colors.danger_active = danger_active;
        colors.danger_foreground = on_state;
        colors.warning = warning;
        colors.warning_hover = warning_hover;
        colors.warning_active = warning_active;
        colors.warning_foreground = on_state;
        colors.success = success;
        colors.success_hover = success_hover;
        colors.success_active = success_active;
        colors.success_foreground = on_state;
        colors.info = accent;
        colors.info_hover = accent_hover;
        colors.info_active = accent_active;
        colors.info_foreground = on_accent;

        // Button surfaces follow the same roles so a component variant and an
        // application surface cannot drift apart.
        colors.button = secondary;
        colors.button_hover = secondary_hover;
        colors.button_active = secondary_active;
        colors.button_foreground = foreground;
        colors.button_primary = accent;
        colors.button_primary_hover = accent_hover;
        colors.button_primary_active = accent_active;
        colors.button_primary_foreground = on_accent;
        colors.button_secondary = secondary;
        colors.button_secondary_hover = secondary_hover;
        colors.button_secondary_active = secondary_active;
        colors.button_secondary_foreground = foreground;
        colors.button_danger = danger;
        colors.button_danger_hover = danger_hover;
        colors.button_danger_active = danger_active;
        colors.button_danger_foreground = on_state;
        colors.button_warning = warning;
        colors.button_warning_hover = warning_hover;
        colors.button_warning_active = warning_active;
        colors.button_warning_foreground = on_state;
        colors.button_success = success;
        colors.button_success_hover = success_hover;
        colors.button_success_active = success_active;
        colors.button_success_foreground = on_state;
        colors.button_info = accent;
        colors.button_info_hover = accent_hover;
        colors.button_info_active = accent_active;
        colors.button_info_foreground = on_accent;

        // Data surfaces: one hairline boundary, hover and selection distinct.
        colors.list = background;
        colors.list_even = panel;
        colors.list_head = panel;
        colors.list_hover = row_hover;
        colors.list_active = row_active;
        colors.list_active_border = accent;
        colors.table = background;
        colors.table_even = panel;
        colors.table_head = panel;
        colors.table_head_foreground = muted_foreground;
        colors.table_foot = panel;
        colors.table_foot_foreground = muted_foreground;
        colors.table_hover = row_hover;
        colors.table_active = row_active;
        colors.table_active_border = accent;
        colors.table_row_border = border;

        colors.group_box = panel;
        colors.group_box_foreground = foreground;
        colors.accordion = panel;
        colors.sidebar = panel;
        colors.sidebar_foreground = foreground;
        colors.sidebar_border = border;
        colors.sidebar_accent = accent_soft;
        colors.sidebar_accent_foreground = accent_soft_foreground;
        colors.sidebar_primary = accent;
        colors.sidebar_primary_foreground = on_accent;

        colors.title_bar = hsla(224. / 360., 0.20, 0.09, 1.);
        colors.title_bar_border = border;
        colors.status_bar = panel;
        colors.status_bar_border = border;

        colors.switch = secondary_active;
        colors.switch_thumb = foreground;
        colors.caret = accent;
        colors.progress_bar = accent;
        colors.skeleton = muted;
        colors.slider_bar = secondary_active;
        colors.slider_thumb = foreground;
        colors.scrollbar = background;
        colors.scrollbar_thumb = border;
        colors.scrollbar_thumb_hover = muted_foreground;

        colors.link = accent;
        colors.link_hover = accent_hover;
        colors.link_active = accent_active;

        // The treemap and the analysis panel draw from one accent family.
        colors.chart_1 = chart_1;
        colors.chart_2 = chart_2;
        colors.chart_3 = chart_3;
        colors.chart_4 = chart_4;
        colors.chart_5 = chart_5;
        colors.chart_bullish = success;
        colors.chart_bearish = danger;

        colors.description_list_label = panel;
        colors.description_list_label_foreground = muted_foreground;
        colors.drop_target = accent;
        colors.tab = background;
        colors.tab_bar = panel;
        colors.tab_bar_segmented = secondary;
        colors.tab_foreground = muted_foreground;
        colors.tab_active = panel;
        colors.tab_active_foreground = foreground;
    }

    // `Theme::global_mut` leaves the renderable token snapshot stale; derive it
    // from the colours we just wrote before projecting to the Base layer.
    let tokens = ThemeTokens::from(&Theme::global(cx).colors);
    Theme::global_mut(cx).tokens = tokens;
    Theme::sync_base(cx);
}

/// The azure accent mixed for a treemap tile of the given rank.
///
/// `rank` is `0.0` for the largest tile and `1.0` for the smallest; the ramp
/// stays inside the accent family so the treemap reads as one material rather
/// than a rainbow.
pub fn tile_tint(cx: &App, rank: f32) -> gpui_kit::Hsla {
    let rank = rank.clamp(0.0, 1.0);
    let mut tint = cx.theme().chart_1;
    tint.l = (tint.l - 0.20 * rank).clamp(0.0, 1.0);
    tint.s = (tint.s - 0.12 * rank).clamp(0.0, 1.0);
    tint.alpha(0.30 + 0.28 * (1.0 - rank))
}
