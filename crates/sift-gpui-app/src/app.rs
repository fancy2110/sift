//! The window's root view, laid out to the design稿.
//!
//! The structure is the design's (`src/views/CleanerView.svelte`,
//! `src/lib/components/TitleBar.svelte`):
//!
//! ```text
//! ┌ title bar ─────────────────────────── 46px, bg 55%, hairline bottom ─┐
//! │ ●●●(real)  ▨Sift                             [switch] [scan]         │
//! ├ workspace ────────────────────────────── padding 12, gap 12 ─────────┤
//! │ ▢ glass capsule: [location ▾ │ crumb › crumb]                        │
//! │ ┌ canvas 1180 max, radius 20, surface 52% ──────────────────────────┐ │
//! │ │ treemap stage (flex 1, pad 14)        │ file list (268px)         │ │
//! │ └───────────────────────────────────────────────────────────────────┘ │
//! │ ▢ glass capsule: [▨] 摘要 … ›                       (opens the popup) │
//! └───────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! This layer renders the model and forwards intents; it holds no product rule.
//! Every colour, size and gap comes from [`crate::theme`], which is a
//! transcription of the design's tokens, so the fidelity question has one place
//! to be reviewed.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::{
    Disableable as _, Selectable as _, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    App, AnyElement, BoxShadow, Context, Entity, FocusHandle, FontWeight, Hsla, KeyBinding, Render,
    Window, div, point, px, rgb,
};
use sift_core::{NodeKey, Volume, format_bytes};
use sift_store::Store;

use crate::model::{ToastLevel, WorkspaceModel};
use crate::services::Services;
use crate::theme::{elevation, metrics, palette, text};
use crate::treemap_element::TreemapElement;

gpui_kit::actions!(sift, [Rescan, Activate, SelectPrev, SelectNext, GoUp, Dismiss]);

/// How many rows the file list draws before it says how many are hidden. The
/// design renders the whole list; the cap keeps a pathological directory from
/// building a frame's worth of elements, and the count is stated, not hidden.
const ROW_LIMIT: usize = 300;

/// The design's font weights, exactly as `app.css` and the components ask for
/// them. GPUI's `FontWeight` is a plain number, so there is no reason to round
/// the design's 480/550/560/600/650 to the nearest named constant.
pub mod weight {
    use gpui_kit::FontWeight;
    /// `.btn` — every button label.
    pub const BUTTON: FontWeight = FontWeight(550.0);
    /// A file-list row's name (`font-[480]`).
    pub const ROW_NAME: FontWeight = FontWeight(480.0);
    /// A list or popup heading (`font-[650]`).
    pub const HEADING: FontWeight = FontWeight(650.0);
    /// A candidate's name in the popup (`font-[560]`).
    pub const CANDIDATE: FontWeight = FontWeight(560.0);
    /// A candidate's size in the popup (`font-[600]`).
    pub const CANDIDATE_SIZE: FontWeight = FontWeight(600.0);
    /// The brand wordmark (`font-[700]`).
    pub const BRAND: FontWeight = FontWeight(700.0);
}

/// The capsule is pad 10 + a 28 px icon box + pad 10, so the popup that sits
/// above it starts at the workspace pad plus the capsule plus the design's gap.
const CAPSULE_H: f32 = metrics::CAPSULE_ICON_BOX + metrics::CAPSULE_PAD_Y * 2.0;
const POPUP_BOTTOM: f32 = metrics::WORKSPACE_PAD + CAPSULE_H + 8.0;
const POPUP_W: f32 = 520.0;
const POPUP_MAX_H: f32 = 560.0;
/// The design's window height. The popup's height is derived from it, so this
/// is the number to change if the window's default size ever moves.
const POPUP_WINDOW_H: f32 = 832.0;

/// The application's root view.
pub struct AppView {
    model: Entity<WorkspaceModel>,
    services: Services,
    focus_handle: FocusHandle,
    /// The newest toast already surfaced as a notification.
    notified_toast: u64,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The store is opened before the first frame so its file I/O never
        // competes with rendering; load problems surface as notifications.
        let (store, warnings) = Store::open_default();
        Self::with_store(store, warnings, window, cx)
    }

    /// Build the view on an explicit store.
    ///
    /// The entry point for tests, and for a future "open a specific workspace"
    /// flow: the store decides where conclusions live, so it belongs to the
    /// caller rather than being discovered inside the view.
    pub fn with_store(
        store: Store,
        warnings: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let model = cx.new(|_| WorkspaceModel::new());
        let focus_handle = cx.focus_handle();
        let services = Services::new(model.clone(), Arc::new(store), warnings, cx);
        focus_handle.focus(window, cx);

        // Bind before anything reads the keymap.
        cx.bind_keys([
            KeyBinding::new("cmd-r", Rescan, None),
            KeyBinding::new("escape", Dismiss, None),
            KeyBinding::new("up", SelectPrev, None),
            KeyBinding::new("down", SelectNext, None),
            KeyBinding::new("enter", Activate, None),
            KeyBinding::new("backspace", GoUp, None),
        ]);

        Self {
            model,
            services,
            focus_handle,
            notified_toast: 0,
        }
    }

    pub fn model(&self) -> &Entity<WorkspaceModel> {
        &self.model
    }

    pub fn services(&self) -> &Services {
        &self.services
    }

    /// Whether the cleanup candidate popup is open (the design's `drawerOpen`).
    ///
    /// The state lives in the model, which already owns it: two copies of a
    /// drawer flag is how a window ends up with a popup that is open in one place
    /// and closed in another.
    pub fn candidates_open(&self, cx: &App) -> bool {
        self.model.read(cx).drawer_open()
    }

    /// Open the cleanup candidate popup (the design's `drawerOpen = true`).
    pub fn open_candidates(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            model.set_drawer_open(true);
            cx.notify();
        });
    }

    /// Close the popup. Escape, the scrim, the close control and a committed
    /// cleanup all call this.
    pub fn close_candidates(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            model.set_drawer_open(false);
            cx.notify();
        });
    }

    pub fn services_mut(&mut self) -> &mut Services {
        &mut self.services
    }

    // ---- commands ----------------------------------------------------------

    fn on_rescan(&mut self, _: &Rescan, _: &mut Window, cx: &mut Context<Self>) {
        self.services.restart_scan(cx);
    }

    fn on_go_up(&mut self, _: &GoUp, _: &mut Window, cx: &mut Context<Self>) {
        let parent = self
            .model
            .read(cx)
            .current_node()
            .and_then(|node| node.parent);
        if let Some(parent) = parent {
            self.drill(parent, cx);
        }
    }

    fn on_dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        // Escape dismisses the topmost layer first: the candidate popup, then the
        // window's own overlays, and only then the in-window state. It never
        // closes the window.
        if self.model.read(cx).drawer_open() {
            self.close_candidates(cx);
            return;
        }
        if window.has_active_dialog(cx) {
            window.close_dialog(cx);
            return;
        }
        if window.has_active_sheet(cx) {
            window.close_sheet(cx);
            return;
        }
        self.model.update(cx, |model, cx| {
            if model.selection().is_empty() {
                model.set_focus(None);
            } else {
                model.clear_selection();
            }
            cx.notify();
        });
    }

    fn on_activate(&mut self, _: &Activate, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(key) = self.model.read(cx).focus_key() {
            self.activate(key, cx);
        }
    }

    fn on_select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.move_focus(-1, cx);
    }

    fn on_select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_focus(1, cx);
    }

    fn move_focus(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| {
            let rows: Vec<NodeKey> = model.visible_entries().iter().map(|node| node.key).collect();
            if rows.is_empty() {
                return;
            }
            let current = model
                .focus_key()
                .and_then(|key| rows.iter().position(|row| *row == key));
            let next = match current {
                Some(index) => (index as i32 + delta).clamp(0, rows.len() as i32 - 1) as usize,
                None if delta >= 0 => 0,
                None => rows.len() - 1,
            };
            model.set_focus(Some(rows[next]));
            cx.notify();
        });
    }

    fn activate(&mut self, key: NodeKey, cx: &mut Context<Self>) {
        let is_dir = self
            .model
            .read(cx)
            .node(key)
            .map(|node| node.is_dir)
            .unwrap_or(false);
        if is_dir {
            self.drill(key, cx);
        } else {
            self.model.update(cx, |model, cx| {
                model.toggle_selected(key);
                cx.notify();
            });
        }
    }

    fn drill(&mut self, key: NodeKey, cx: &mut Context<Self>) {
        let path = self.model.update(cx, |model, cx| {
            if model.drill_into(key) {
                cx.notify();
            }
            model
                .current_node()
                .map(|node| node.path.clone())
                .unwrap_or_default()
        });
        if !path.as_os_str().is_empty() {
            self.services.focus_scan(path);
        }
    }

    // ---- title bar (46 px) -------------------------------------------------

    fn render_title_bar(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (scanning, monitor_running) = {
            let model = self.model.read(cx);
            (model.is_scanning(), model.monitor().running)
        };

        h_flex()
            .h(px(metrics::TITLE_BAR_H))
            .flex_shrink_0()
            .items_center()
            .gap(px(metrics::TITLE_BAR_GAP))
            .px(px(metrics::TITLE_BAR_PAD_X))
            .bg(p.bg.opacity(0.55))
            .border_b_1()
            .border_color(p.border.opacity(0.6))
            // The real macOS traffic lights live here (x = 14), so the design's
            // leading space is reserved rather than drawn twice.
            .child(div().w(px(metrics::TRAFFIC_RESERVE)))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .size(px(metrics::MARK))
                            .items_center()
                            .justify_center()
                            .rounded(px(metrics::MARK_RADIUS))
                            .bg(p.accent)
                            .shadow(vec![BoxShadow {
                                color: p.accent.opacity(0.55),
                                offset: point(px(0.), px(4.)),
                                blur_radius: px(12.),
                                spread_radius: px(-4.),
                                inset: false,
                            }])
                            .child(
                                icon("layers", 13., p.accent_contrast),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(text::TITLE))
                            .font_weight(weight::BRAND)
                            .child("Sift"),
                    ),
            )
            .child(div().flex_1())
            .child(self.render_auto_switch(monitor_running, cx))
            .child(
                // The design's only primary action, disabled while a scan runs so
                // a second click cannot start a competing walk.
                // `.label` rather than a child: the label is what an
                // accessibility client reads, and the component exposes no way to
                // set a label's weight, so the design's 550 stays the one
                // approximation here.
                // The design's button carries an icon before its label: a spark
                // when idle, a spinner while walking (`Icon name="spark"` /
                // `name="refresh"` in TitleBar.svelte).
                gpui_kit::component::button::Button::new("scan")
                    .icon(
                        gpui_kit::component::Icon::default()
                            .path(crate::assets::path(if scanning { "refresh" } else { "spark" })),
                    )
                    .label(if scanning { "正在扫描" } else { "智能扫描" })
                    .small()
                    .primary()
                    .disabled(scanning)
                    .on_click(cx.listener(|view, _, _, cx| view.services.restart_scan(cx))),
            )
    }

    /// The design's `switch`: 38 × 22 with a 16 px thumb.
    fn render_auto_switch(&self, on: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        div()
            .id("auto-switch")
            .w(px(metrics::SWITCH_W))
            .h(px(metrics::SWITCH_H))
            .flex_shrink_0()
            .rounded(px(999.))
            .bg(if on { p.accent } else { p.surface3 })
            .border_1()
            .border_color(if on { p.accent } else { p.border })
            .flex()
            .items_center()
            .when(on, |el| {
                el.shadow(vec![BoxShadow {
                    color: p.accent.opacity(0.6),
                    offset: point(px(0.), px(2.)),
                    blur_radius: px(8.),
                    spread_radius: px(-2.),
                    inset: false,
                }])
            })
            .child(
                div()
                    .size(px(metrics::SWITCH_THUMB))
                    .rounded(px(999.))
                    .bg(rgb(0xFF_FF_FF))
                    .ml(px(if on {
                        metrics::SWITCH_W - metrics::SWITCH_THUMB - 4.0
                    } else {
                        2.0
                    }))
                    .shadow(vec![BoxShadow {
                        color: Hsla::from(rgb(0x00_00_00)).opacity(0.3),
                        offset: point(px(0.), px(1.)),
                        blur_radius: px(3.),
                        spread_radius: px(0.),
                        inset: false,
                    }]),
            )
            .on_click(cx.listener(|view, _, _, cx| {
                let running = view.model.read(cx).monitor().running;
                view.services.set_monitor_running(!running, cx);
            }))
    }

    // ---- top navigation: one pill holding the picker and the crumbs --------

    fn render_nav(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let volumes = self.model.read(cx).volumes().to_vec();
        let current = self.model.read(cx).current_volume_id();
        let crumbs: Vec<(NodeKey, String)> = self
            .model
            .read(cx)
            .breadcrumbs()
            .iter()
            .map(|node| (node.key, node.name.clone()))
            .collect();

        let current_volume = current.and_then(|id| volumes.iter().find(|volume| volume.id == id).cloned());
        let this = cx.entity();
        let volumes_for_menu: Rc<Vec<Volume>> = Rc::new(volumes);

        h_flex()
            .self_start()
            .flex_shrink_0()
            .items_center()
            .gap(px(metrics::NAV_GAP))
            .rounded(px(metrics::NAV_RADIUS))
            .px(px(metrics::NAV_PAD_X))
            .py(px(metrics::NAV_PAD_Y))
            // `.glass`: translucent surface, hairline border, inset highlight and
            // a deep drop shadow.
            .bg(p.surface.opacity(0.86))
            .border_1()
            .border_color(p.border_strong.opacity(0.7))
            .shadow(elevation::glass())
            .child({
                let current_for_menu = current;
                let volumes_menu = volumes_for_menu.clone();
                let this = this.clone();
                let removable = current_volume.as_ref().is_some_and(|v| v.is_removable);
                let mut label: Vec<AnyElement> = Vec::new();
                label.push(
                    icon("hard-drive", 14., if removable { p.accent_hi } else { p.fg })
                        .into_any_element(),
                );
                label.push(
                    div()
                        .text_size(px(text::NAV_NAME))
                        .font_weight(weight::BUTTON)
                        .child(
                            current_volume
                                .as_ref()
                                .map(|volume| volume.name.clone())
                                .unwrap_or_else(|| "—".to_string()),
                        )
                        .into_any_element(),
                );
                if let Some(volume) = current_volume.as_ref() {
                    label.push(
                        div()
                            .font_family("Menlo")
                            .text_size(px(text::NAV_SUB))
                            .text_color(p.faint)
                            .child(format!("{} 可用", format_bytes(volume.available_bytes)))
                            .into_any_element(),
                    );
                }
                label.push(
                    icon("chevron-down", 12., p.muted)
                        .into_any_element(),
                );

                let mut button = gpui_kit::component::button::Button::new("volume-picker")
                    .ghost()
                    .small();
                for element in label {
                    button = button.child(element);
                }
                button.dropdown_menu(move |mut menu, _window, _cx| {
                        for volume in volumes_menu.iter() {
                            let id = volume.id;
                            let selected = Some(id) == current_for_menu;
                            let item_label = format!(
                                "{} · {} / {}",
                                volume.name,
                                format_bytes(volume.available_bytes),
                                format_bytes(volume.total_bytes)
                            );
                            let this = this.clone();
                            menu = menu.item(
                                PopupMenuItem::new(item_label)
                                    .checked(selected)
                                    .on_click(move |_, _window, cx| {
                                        this.update(cx, |view, cx| {
                                            view.services.select_volume(id, cx);
                                        });
                                    }),
                            );
                        }
                        let this = this.clone();
                        menu.separator().item(
                            PopupMenuItem::new("所有磁盘").on_click(move |_, _window, cx| {
                                this.update(cx, |view, cx| view.services.show_all_volumes(cx));
                            }),
                        )
                    })
            })
            // The design's divider: 1 × 20 with a 4 px margin either side.
            .child(
                div()
                    .w(px(metrics::DIVIDER_W))
                    .h(px(metrics::DIVIDER_H))
                    .mx(px(metrics::DIVIDER_MX))
                    .bg(p.border_strong),
            )
            .child({
                let mut trail = h_flex().items_center().gap(px(metrics::CRUMB_GAP)).pr_1();
                for (index, (key, name)) in crumbs.iter().enumerate() {
                    let key = *key;
                    if index > 0 {
                        trail = trail.child(
                            icon("chevron-right", 11., p.faint),
                        );
                    }
                    let is_last = index + 1 == crumbs.len();
                    trail = trail.child(
                        gpui_kit::component::button::Button::new(format!("crumb-{index}"))
                            .ghost()
                            .xsmall()
                            .when(is_last, |button| button.selected(true))
                            .child(
                                div()
                                    .max_w(px(metrics::CRUMB_MAX_W))
                                    .text_size(px(text::CRUMB))
                                    .text_color(if is_last { p.fg } else { p.muted })
                                    .child(name.clone()),
                            )
                            .on_click(cx.listener(move |view, _, _, cx| view.drill(key, cx))),
                    );
                }
                trail
            })
    }

    // ---- body: the canvas holding the treemap and the 268 px list ----------

    fn render_body(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let pending = self.model.read(cx).is_scanning();
        let started = self.model.read(cx).has_started();

        h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .justify_center()
            .child(
                h_flex()
                    .w_full()
                    .max_w(px(metrics::CANVAS_MAX_W))
                    .min_h_0()
                    .items_stretch()
                    .rounded(px(metrics::CANVAS_RADIUS))
                    .overflow_hidden()
                    .bg(p.surface.opacity(metrics::CANVAS_ALPHA))
                    .child(
                        // The stage: the treemap plus the design's pending hint.
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .child(self.render_treemap(cx))
                            // Nothing has been walked yet: say so instead of
                            // showing an empty map, which reads as an empty disk.
                            .when(!started, |stage| {
                                stage.child(
                                    v_flex()
                                        .absolute()
                                        .inset_0()
                                        .items_center()
                                        .justify_center()
                                        .gap_2()
                                        .child(icon("spark", 26., p.faint))
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .text_color(p.muted)
                                                .child("点击「智能扫描」开始"),
                                        ),
                                )
                            })
                            .when(pending, |stage| {
                                stage.child(
                                    h_flex()
                                        .absolute()
                                        .left(px(12.))
                                        .top(px(8.))
                                        .items_center()
                                        .gap_1()
                                        .text_size(px(10.5))
                                        .text_color(p.faint)
                                        .child(
                                            icon("refresh", 11., p.faint),
                                        )
                                        .child("正在扫描…"),
                                )
                            }),
                    )
                    .child(self.render_file_list(cx)),
            )
    }

    fn render_treemap(&self, cx: &mut Context<Self>) -> impl IntoElement {
        TreemapElement::new(
            self.model.clone(),
            {
                let view = cx.entity();
                move |key: NodeKey, _window: &mut Window, cx: &mut App| {
                    view.update(cx, |view, cx| view.activate(key, cx));
                }
            },
            {
                let view = cx.entity();
                move |key: NodeKey, _window: &mut Window, cx: &mut App| {
                    view.update(cx, |view, cx| {
                        view.model.update(cx, |model, cx| {
                            model.set_focus(Some(key));
                            cx.notify();
                        });
                    });
                }
            },
        )
    }

    /// The design's `immersive-list`: 268 px, a 30% background and two inset
    /// shadows that make it read as a recessed pane rather than a card.
    fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let focus = self.model.read(cx).focus_key();

        let rows: Vec<RowData> = {
            let model = self.model.read(cx);
            let findings = model.findings();
            model
                .visible_entries()
                .iter()
                .take(ROW_LIMIT)
                .map(|node| RowData {
                    key: node.key,
                    name: node.name.clone(),
                    size: node.size_label(),
                    is_dir: node.is_dir,
                    selected: model.is_selected(node.key),
                    safety: findings
                        .iter()
                        .find(|finding| finding.key == node.key)
                        .map(|finding| finding.safety),
                })
                .collect()
        };
        let total = self.model.read(cx).visible_entries().len();

        // The design's list is the pane's scroll area (`overflow-y-auto`): a
        // directory with more entries than fit must scroll, not clip.
        let mut list = v_flex()
            .id("file-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(metrics::LIST_PAD_X))
            .pb(px(metrics::LIST_PAD_BOTTOM));
        if rows.is_empty() {
            let started = self.model.read(cx).has_started();
            list = list.child(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_size(px(12.))
                    .text_color(p.muted)
                    .child(if started {
                        icon("check", 20., p.ok)
                    } else {
                        icon("spark", 20., p.faint)
                    })
                    // "Scanned and found nothing" and "never scanned" are
                    // different claims about the disk; only one of them is true
                    // before the user presses the control.
                    .child(if started {
                        "此文件夹没有可显示的内容"
                    } else {
                        "尚未扫描"
                    }),
            );
        }
        for row in rows {
            list = list.child(self.render_row(row, focus, cx));
        }
        if total > ROW_LIMIT {
            list = list.child(
                div()
                    .px_2()
                    .py_2()
                    .text_size(px(text::LIST_SIZE))
                    .text_color(p.faint)
                    .child(format!("还有 {} 项未显示（按大小排序）", total - ROW_LIMIT)),
            );
        }

        v_flex()
            .w(px(metrics::LIST_W))
            .flex_shrink_0()
            .min_h_0()
            .bg(p.bg.opacity(0.3))
            .shadow(vec![
                BoxShadow {
                    color: Hsla::from(rgb(0x00_00_00)).opacity(0.55),
                    offset: point(px(14.), px(18.)),
                    blur_radius: px(22.),
                    spread_radius: px(-18.),
                    inset: true,
                },
                BoxShadow {
                    color: Hsla::from(rgb(0x00_00_00)).opacity(0.45),
                    offset: point(px(10.), px(0.)),
                    blur_radius: px(14.),
                    spread_radius: px(-12.),
                    inset: true,
                },
            ])
            .child(
                // The design's list header: 12 px/650 title, 11 px mono count.
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(metrics::LIST_HEADER_GAP))
                    .px(px(metrics::LIST_HEADER_PAD_X))
                    .pt(px(metrics::LIST_HEADER_PAD_TOP))
                    .pb(px(metrics::LIST_HEADER_PAD_BOTTOM))
                    .child(
                        div()
                            .text_size(px(text::LIST_TITLE))
                            .font_weight(weight::HEADING)
                            .text_color(p.muted)
                            .child("文件与文件夹"),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .font_family("Menlo")
                            .text_size(px(text::LIST_SIZE))
                            .text_color(p.faint)
                            .child(total.to_string()),
                    ),
            )
            .child(list)
    }

    fn render_row(
        &self,
        row: RowData,
        focus: Option<NodeKey>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = palette(cx);
        let is_focused = focus == Some(row.key);
        let dim = focus.is_some() && !is_focused;
        let safety_color = row.safety.map(|safety| match safety {
            sift_analyze::Safety::Safe => p.ok,
            sift_analyze::Safety::Review => p.warn,
            sift_analyze::Safety::Keep => p.faint,
        });
        let approval = {
            let model = self.model.read(cx);
            model
                .findings()
                .iter()
                .find(|finding| finding.key == row.key)
                .map(|finding| finding.approved_for_auto)
        };
        let this = cx.entity();
        let key = row.key;
        let selected = row.selected;

        let row_menu = move |mut menu: gpui_kit::component::menu::PopupMenu,
                             _window: &mut Window,
                             _cx: &mut gpui_kit::Context<gpui_kit::component::menu::PopupMenu>| {
            let toggler = this.clone();
            menu = menu.item(
                PopupMenuItem::new(if selected {
                    "从删除队列移除"
                } else {
                    "加入删除队列"
                })
                .on_click(move |_, _, cx| {
                    toggler.update(cx, |view, cx| {
                        view.model.update(cx, |model, cx| {
                            model.toggle_selected(key);
                            cx.notify();
                        });
                    });
                }),
            );
            if let Some(approved) = approval {
                let approver = this.clone();
                menu = menu.item(
                    PopupMenuItem::new(if approved {
                        "取消允许自动清理"
                    } else {
                        "允许自动清理"
                    })
                    .on_click(move |_, _, cx| {
                        approver.update(cx, |view, cx| {
                            view.services.set_auto_approval(key, !approved, cx);
                        });
                    }),
                );
            }
            menu
        };

        h_flex()
            .id(format!("row-wrapper-{}", row.key.raw()))
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                // `onmouseenter` / `onmouseleave` set the shared focus, which is
                // what dims the other rows and the other tiles at once.
                view.model.update(cx, |model, cx| {
                    if *hovered {
                        model.set_focus(Some(key));
                    } else if model.focus_key() == Some(key) {
                        model.set_focus(None);
                    }
                    cx.notify();
                });
            }))
            .context_menu(row_menu)
            .w_full()
            .h(px(metrics::ROW_H))
            .items_center()
            .gap(px(metrics::ROW_GAP))
            .rounded(px(metrics::ROW_RADIUS))
            .px(px(metrics::ROW_PAD_X))
            .py(px(metrics::ROW_PAD_Y))
            // `.entry-row.row-dim { opacity: 0.34 }`: the design dims the others
            // rather than highlighting the hovered row, and the treemap tile is
            // where the highlight is read from.
            .when(dim, |el| el.opacity(0.34))
            // The design's 18 px icon slot keeps names on one spine whether the
            // icon differs in width or is absent.
            .child(
                // `h-[18px] w-[18px]`: the slot's height is what sets the design's
                // 32 px row (18 px of content plus 7 px above and below).
                div()
                    .flex()
                    .size(px(metrics::ROW_ICON_SLOT))
                    .items_center()
                    .justify_center()
                    .text_color(p.faint)
                    .child(icon(
                        // The design's pair: a folder for a directory, a disk for
                        // a file (`entry.isDir ? 'folder' : 'hardDrive'`).
                        if row.is_dir { "folder" } else { "hard-drive" },
                        metrics::ROW_ICON,
                        p.faint,
                    )),
            )
            .child(
                // The design's name button: `flex min-w-0 flex-1`, no padding of
                // its own, the name truncating and the chevron pushed to the
                // button's trailing edge with `ml-auto`.
                gpui_kit::component::button::Button::new(format!("row-{}", row.key.raw()))
                    .ghost()
                    .xsmall()
                    .flex_1()
                    .min_w_0()
                    .px(px(0.))
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(text::LIST_NAME))
                                    .font_weight(weight::ROW_NAME)
                                    .text_color(p.fg)
                                    .child(row.name.clone()),
                            )
                            .when(row.is_dir, |el| {
                                el.child(
                                    div()
                                        .ml_auto()
                                        .flex_shrink_0()
                                        .child(icon("chevron-right", 12., p.faint)),
                                )
                            }),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| view.activate(key, cx))),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    // An AI conclusion reads as a marker on the row, which is what
                    // makes the treemap and the list one judgement view.
                    .when_some(safety_color, |el, color| {
                        el.child(div().size(px(6.)).rounded(px(999.)).bg(color))
                    })
                    .when(selected, |el| {
                        el.child(
                            icon("check", 12., p.accent),
                        )
                    })
                    .child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(text::LIST_SIZE))
                            .text_color(p.muted)
                            .child(row.size.clone()),
                    ),
            )
    }

    // ---- bottom capsule: the design's summary, and the popup's anchor ------

    fn render_capsule(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let (scanning, files, dirs, selected_bytes, selected_count, available) = {
            let model = self.model.read(cx);
            (
                model.is_scanning(),
                model.progress().files,
                model.progress().dirs,
                model.selected_bytes(),
                model.selection().len(),
                model.current_volume().map(|volume| volume.available_bytes),
            )
        };

        let summary: AnyElement = if scanning {
            h_flex()
                .gap_1()
                .text_size(px(text::CAPSULE))
                .child("正在扫描：")
                .child(
                    div()
                        .font_family("Menlo")
                        .font_weight(weight::HEADING)
                        .child(files.to_string()),
                )
                .child(" 文件 · ")
                .child(
                    div()
                        .font_family("Menlo")
                        .font_weight(weight::HEADING)
                        .child(dirs.to_string()),
                )
                .child(" 文件夹")
                .into_any_element()
        } else if selected_count > 0 {
            h_flex()
                .gap_1()
                .text_size(px(text::CAPSULE))
                .child("待清理 ")
                .child(
                    div()
                        .font_family("Menlo")
                        .font_weight(weight::HEADING)
                        .text_color(p.accent_hi)
                        .child(format_bytes(selected_bytes)),
                )
                .child(
                    div()
                        .text_color(p.faint)
                        .child(format!(" · {selected_count} 项")),
                )
                .into_any_element()
        } else {
            h_flex()
                .gap_1()
                .text_size(px(text::CAPSULE))
                .child(
                    div()
                        .font_family("Menlo")
                        .font_weight(weight::HEADING)
                        .child(format_bytes(available.unwrap_or(0))),
                )
                .child(div().text_color(p.faint).child(" 可用空间"))
                .into_any_element()
        };

        h_flex()
            .id("ai-summary")
            .self_start()
            .flex_shrink_0()
            .items_center()
            .gap(px(metrics::CAPSULE_GAP))
            .rounded(px(metrics::CAPSULE_RADIUS))
            .px(px(metrics::CAPSULE_PAD_X))
            .py(px(metrics::CAPSULE_PAD_Y))
            .bg(p.surface.opacity(0.86))
            .border_1()
            .border_color(p.border_strong.opacity(0.7))
            .shadow(elevation::glass())
            .child(
                div()
                    .flex()
                    .size(px(metrics::CAPSULE_ICON_BOX))
                    .items_center()
                    .justify_center()
                    .rounded(px(metrics::CAPSULE_ICON_RADIUS))
                    .bg(p.accent.opacity(0.22))
                    .child(
                        icon("layers", 15., p.accent_hi),
                    ),
            )
            .child(summary)
            .child(
                icon("chevron-right", 14., p.faint),
            )
            .on_click(cx.listener(|view, _, _, cx| {
                if view.model.read(cx).drawer_open() {
                    view.close_candidates(cx);
                } else {
                    view.open_candidates(cx);
                }
            }))
    }

    /// The design's backdrop: a scrim over the workspace that closes the popup.
    fn render_scrim(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.model.read(cx).drawer_open() {
            return None;
        }
        Some(
            div()
                .id("popup-scrim")
                .absolute()
                .inset_0()
                // `oklch(0 0 0 / 0.34)` with a blur. GPUI has no backdrop blur,
                // so the scrim carries slightly more weight instead.
                .bg(Hsla::from(rgb(0x00_00_00)).opacity(0.42))
                .on_click(cx.listener(|view, _, _, cx| view.close_candidates(cx)))
                .into_any_element(),
        )
    }

    /// The design's candidate popup: anchored above the capsule, 520 × 560 max,
    /// with a header, the candidate rows and a footer that commits.
    fn render_candidates(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.model.read(cx).drawer_open() {
            return None;
        }
        let p = palette(cx);
        // `min(560, capsule top - workspace top - 8)`: the design's popup is a
        // fixed-height frame, so an empty list still shows the panel a user
        // expects rather than a box that shrinks to its content.
        let popup_height = (POPUP_WINDOW_H - metrics::TITLE_BAR_H - metrics::WORKSPACE_PAD * 2.0
            - CAPSULE_H
            - 8.0)
            .min(POPUP_MAX_H);
        let (queued, suggestions, selected_bytes) = {
            let model = self.model.read(cx);
            let queued: Vec<(NodeKey, String, String, u64)> = model
                .selected_items()
                .iter()
                .map(|node| {
                    (
                        node.key,
                        node.name.clone(),
                        node.path.to_string_lossy().into_owned(),
                        node.size.reclaimable(),
                    )
                })
                .collect();
            // The analysis's own nominations, offered as one-click suggestions.
            let suggestions: Vec<(NodeKey, String, String, u64)> = model
                .removable_findings()
                .iter()
                .filter(|finding| !model.is_selected(finding.key))
                .take(50)
                .map(|finding| {
                    (
                        finding.key,
                        finding.name.clone(),
                        render_reason(&finding.reason),
                        finding.size,
                    )
                })
                .collect();
            (queued, suggestions, model.selected_bytes())
        };
        let count = queued.len();

        let mut rows = v_flex().flex_1().min_h_0().px(px(12.)).pt_2();
        for (key, name, detail, size) in &queued {
            let key = *key;
            let (name, detail, size) = (name.clone(), detail.clone(), *size);
            rows = rows.child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_3()
                    .rounded(px(metrics::ROW_RADIUS))
                    .px(px(10.))
                    .py_2()
                    .child(
                        // The design's checked box: an accent square with a tick.
                        div()
                            .id(format!("unqueue-{}", key.raw()))
                            .flex()
                            .size(px(18.))
                            .flex_shrink_0()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .bg(p.accent)
                            .child(
                                icon("check", 12., p.accent_contrast),
                            )
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.model.update(cx, |model, cx| {
                                    model.toggle_selected(key);
                                    cx.notify();
                                });
                            })),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(px(text::LIST_NAME))
                            .font_weight(weight::CANDIDATE)
                            .child(name),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family("Menlo")
                            .text_size(px(text::LIST_SIZE))
                            .text_color(p.faint)
                            .child(detail),
                    )
                    .child(
                        div()
                            .w(px(64.))
                            .flex_shrink_0()
                            .text_right()
                            .font_family("Menlo")
                            .text_size(px(12.))
                            .font_weight(weight::CANDIDATE_SIZE)
                            .child(format_bytes(size)),
                    ),
            );
        }

        if !suggestions.is_empty() {
            rows = rows.child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px(px(10.))
                    .pt_3()
                    .pb_1()
                    .child(
                        div()
                            .text_size(px(text::PANEL_SUB))
                            .font_weight(weight::HEADING)
                            .text_color(p.muted)
                            .child("分析建议"),
                    )
                    .child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(text::PANEL_SUB))
                            .text_color(p.faint)
                            .child(format!("{} 项", suggestions.len())),
                    ),
            );
            for (key, name, detail, size) in &suggestions {
                let key = *key;
                let (name, detail, size) = (name.clone(), detail.clone(), *size);
                rows = rows.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_3()
                        .rounded(px(metrics::ROW_RADIUS))
                        .px(px(10.))
                        .py_2()
                        .child(
                            div()
                                .id(format!("queue-{}", key.raw()))
                                .flex()
                                .size(px(18.))
                                .flex_shrink_0()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .border_1()
                                .border_color(p.border_strong)
                                .child(
                                    icon("check", 11., p.muted),
                                )
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.model.update(cx, |model, cx| {
                                        model.toggle_selected(key);
                                        cx.notify();
                                    });
                                })),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_size(px(text::LIST_NAME))
                                .font_weight(FontWeight::MEDIUM)
                                .child(name),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family("Menlo")
                                .text_size(px(text::LIST_SIZE))
                                .text_color(p.faint)
                                .child(detail),
                        )
                        .child(
                            div()
                                .w(px(64.))
                                .flex_shrink_0()
                                .text_right()
                                .font_family("Menlo")
                                .text_size(px(12.))
                                .child(format_bytes(size)),
                        ),
                );
            }
        }

        let body: AnyElement = if queued.is_empty() && suggestions.is_empty() {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .py(px(56.))
                .text_size(px(12.))
                .text_color(p.faint)
                .child("暂未选择项目，可在左侧区块或列表中右键加入")
                .into_any_element()
        } else {
            rows.into_any_element()
        };

        Some(
            v_flex()
                .id("candidate-popup")
                .absolute()
                .left(px(metrics::WORKSPACE_PAD))
                .bottom(px(POPUP_BOTTOM))
                .w(px(POPUP_W))
                .h(px(popup_height))
                .min_h_0()
                .rounded(px(metrics::CANVAS_RADIUS))
                .overflow_hidden()
                .bg(p.surface.opacity(0.95))
                .border_1()
                .border_color(p.border_strong)
                .shadow(elevation::glass_strong())
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_2()
                        .px(px(16.))
                        .pt(px(14.))
                        .child(
                            div()
                                .text_size(px(text::PANEL_TITLE))
                                .font_weight(weight::HEADING)
                                .child("清理候选"),
                        )
                        .child(
                            div()
                                .font_family("Menlo")
                                .text_size(px(text::PANEL_SUB))
                                .text_color(p.faint)
                                .child(format!("{count} 项")),
                        )
                        .child(
                            div()
                                .id("candidates-close")
                                .ml_auto()
                                .flex()
                                .size(px(metrics::BTN_ICON))
                                .items_center()
                                .justify_center()
                                .rounded(px(metrics::BTN_ICON_RADIUS))
                                .text_color(p.muted)
                                .child(icon("x", 15., p.muted))
                                .on_click(cx.listener(|view, _, _, cx| view.close_candidates(cx))),
                        ),
                )
                .child(body)
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_3()
                        .px(px(16.))
                        .py(px(12.))
                        .border_t_1()
                        .border_color(p.border)
                        .child(
                            div()
                                .text_size(px(text::PANEL_SUB))
                                .text_color(p.faint)
                                .child("移入回收站，可恢复"),
                        )
                        .child(
                            gpui_kit::component::button::Button::new("clean")
                                .label(format!("清理 {}", format_bytes(selected_bytes)))
                                .small()
                                .primary()
                                .disabled(count == 0)
                                .ml_auto()
                                .min_w(px(150.))
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.services.trash_selection(cx);
                                    view.close_candidates(cx);
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Surface new model messages as notifications.
    ///
    /// This reads and advances a view-local counter during render. It does not
    /// call `notify`, so it cannot schedule another frame.
    fn sync_notifications(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let toasts: Vec<(u64, String, ToastLevel)> = self
            .model
            .read(cx)
            .toasts()
            .iter()
            .map(|toast| (toast.id, toast.message.clone(), toast.level))
            .collect();
        for (id, message, level) in toasts {
            if id <= self.notified_toast {
                continue;
            }
            self.notified_toast = id;
            let note = gpui_kit::component::notification::Notification::new()
                .message(message)
                .autohide(true);
            let note = match level {
                ToastLevel::Warning | ToastLevel::Error => note.title("Sift"),
                _ => note,
            };
            window.push_notification(note, cx);
        }
    }
}

/// An icon from the design's own set, at an exact size and colour.
///
/// `sift-icons/*.svg` carries the design's paths (24x24, stroked at 1.7 with
/// round caps) and `stroke="currentColor"`, so the colour is the element's text
/// colour — which is also what makes an icon follow the same theme token as the
/// label beside it.
fn icon(name: &str, size: f32, color: Hsla) -> impl IntoElement {
    gpui_kit::svg()
        .path(crate::assets::path(name))
        .flex_shrink_0()
        .size(px(size))
        .text_color(color)
}

/// One file-list row, resolved from the model before rendering.
struct RowData {
    key: NodeKey,
    name: String,
    size: String,
    is_dir: bool,
    selected: bool,
    safety: Option<sift_analyze::Safety>,
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_notifications(window, cx);
        let p = palette(cx);

        let dialogs = gpui_kit::component::Root::render_dialog_layer(window, cx);
        let sheets = gpui_kit::component::Root::render_sheet_layer(window, cx);
        let notifications = gpui_kit::component::Root::render_notification_layer(window, cx);
        let scrim = self.render_scrim(cx);
        let candidates = self.render_candidates(cx);

        v_flex()
            .size_full()
            .key_context("Sift")
            .track_focus(&self.focus_handle)
            .bg(p.bg)
            .text_color(p.fg)
            .text_size(px(text::BODY))
            .on_action(cx.listener(Self::on_rescan))
            .on_action(cx.listener(Self::on_go_up))
            .on_action(cx.listener(Self::on_dismiss))
            .on_action(cx.listener(Self::on_activate))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_select_next))
            .child(self.render_title_bar(window, cx))
            // The design's workspace: 12 px padding, 12 px between the three
            // bands, with the popup anchored inside it.
            .child(
                v_flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .gap(px(metrics::WORKSPACE_GAP))
                    .p(px(metrics::WORKSPACE_PAD))
                    .child(self.render_nav(window, cx))
                    .child(self.render_body(window, cx))
                    .child(self.render_capsule(cx))
                    .children(scrim)
                    .children(candidates),
            )
            .children(dialogs)
            .children(sheets)
            .children(notifications)
    }
}

/// Render a reason: a stable key goes through the local table,
/// adjudicator-written text is shown verbatim.
fn render_reason(reason: &sift_analyze::Reason) -> String {
    use sift_analyze::Reason;
    match reason {
        Reason::Text(text) => text.clone(),
        Reason::Key { key, params } => {
            let template = match key.as_str() {
                "reason.rebuildableCache" => "{} 的可重建目录",
                "reason.cacheDirectory" => "{} 的缓存",
                "reason.packageInstaller" => "{} 安装包",
                "reason.archive" => "{} 压缩包",
                "reason.staleLargeFile" => "{} 天未修改的大文件",
                "reason.duplicate" => "同名同大小的副本",
                "reason.trash" => "回收站内容",
                "reason.tempFile" => "系统临时文件",
                "reason.log" => "{} 日志",
                "reason.insideBundle" => "{} 位于应用包内部",
                "reason.aiConfirmed" => "经模型确认可删除",
                "reason.aiLowConfidence" => "模型置信度不足",
                "reason.aiNotAuthorized" => "该类型不允许由模型判定",
                "reason.aiConsentRequired" => "尚未同意发送文件元数据",
                "reason.unjudged" => "尚未判定",
                _ => "见规则说明",
            };
            match params.first() {
                Some(param) => template.replacen("{}", param, 1),
                None => template.replace("{}", "该项"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_analyze::Reason;

    #[test]
    fn reasons_render_through_the_local_table() {
        let keyed = Reason::key_with("reason.rebuildableCache", vec!["npm".into()]);
        let text = render_reason(&keyed);
        assert!(text.contains("npm"), "{text}");

        let unknown = Reason::key("reason.somethingNew");
        assert_eq!(render_reason(&unknown), "见规则说明");

        let verbatim = Reason::text("这是一个旧安装包");
        assert_eq!(render_reason(&verbatim), "这是一个旧安装包");
    }

    #[test]
    fn the_popup_sits_above_the_capsule() {
        // 12 (workspace pad) + 48 (capsule) + 8 (design gap) = 68.
        assert_eq!(CAPSULE_H, 48.0);
        assert_eq!(POPUP_BOTTOM, 68.0);
    }

    #[test]
    fn text_sizes_match_the_design() {
        assert_eq!(text::BODY, 13.5);
        assert_eq!(text::LIST_NAME, 12.5);
        assert_eq!(text::LIST_SIZE, 11.0);
        assert_eq!(text::PANEL_SUB, 11.0);
        assert_eq!(text::CRUMB, 12.0);
        let _ = Hsla::default();
    }
}
