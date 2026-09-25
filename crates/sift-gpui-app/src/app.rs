//! The window's root view and its regions.
//!
//! `AppView` owns the retained state (the workspace model entity, the service
//! wiring, the side panel choice) and renders one shell. Every region lives in a
//! named `render_*` helper, so a render function never grows into a second place
//! where product rules can hide.
//!
//! This layer renders the model and forwards intents. It does not scan, account,
//! analyze, or decide what is deletable — [`crate::model::WorkspaceModel`] holds
//! every display decision, and the `sift-*` crates hold every product rule.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, WindowExt as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, Selectable as _, Sizable as _, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, Context, Entity, FocusHandle, KeyBinding, Render, Window, div};
use sift_core::NodeKey;
use sift_store::Store;

use crate::model::{ToastLevel, WorkspaceModel};
use crate::services::Services;
use crate::treemap_element::TreemapElement;

gpui_kit::actions!(sift, [Rescan, Activate, SelectPrev, SelectNext, GoUp, Dismiss]);

/// How many rows a panel draws. Both panels are the current directory's children
/// or the analysis conclusions, which the model already filters and sorts; the
/// cap keeps a pathological directory from building a frame's worth of elements,
/// and the panel states how many are hidden rather than truncating silently.
const ROW_LIMIT: usize = 200;

/// Which column the right-hand panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidePanel {
    Files,
    Findings,
}

/// The application's root view.
pub struct AppView {
    model: Entity<WorkspaceModel>,
    services: Services,
    focus_handle: FocusHandle,
    panel: SidePanel,
    /// The newest toast already surfaced as a notification.
    notified_toast: u64,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The store is opened before the first frame so its file I/O never
        // competes with rendering; load problems surface as notifications.
        let (store, warnings) = Store::open_default();
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
            panel: SidePanel::Files,
            notified_toast: 0,
        }
    }

    pub fn model(&self) -> &Entity<WorkspaceModel> {
        &self.model
    }

    pub fn services(&self) -> &Services {
        &self.services
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

    fn on_dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        // Escape clears the queue first (the lighter commitment), then the
        // highlight; it never closes the window.
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
        // Re-prioritize the running scan toward what the user just opened.
        if !path.as_os_str().is_empty() {
            self.services.focus_scan(path);
        }
    }

    // ---- regions -----------------------------------------------------------

    fn render_title_bar(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Copy the colours out of the theme: `cx.theme()` borrows the app, and
        // the builder chains below also call `cx.listener`, which needs `cx`
        // mutably. Hsla values are `Copy`, so a local ends the borrow.
        let (_border, muted, _success, _warning) = {
            let t = cx.theme();
            (t.border, t.muted_foreground, t.success, t.warning)
        };
        let (scanning, status, monitor_running) = {
            let model = self.model.read(cx);
            (
                model.is_scanning(),
                model.status_line(),
                model.monitor().running,
            )
        };

        TitleBar::new().child(
            h_flex()
                .flex_1()
                .items_center()
                .gap_3()
                .px_3()
                .child(div().text_sm().child("Sift"))
                .child(div().flex_1())
                .child(div().text_sm().text_color(muted).child(status))
                .child(
                    Button::new("monitor")
                        .label(if monitor_running { "停止值守" } else { "开始值守" })
                        .small()
                        .on_click(cx.listener(|view, _, _, cx| {
                            let running = view.model.read(cx).monitor().running;
                            view.services.set_monitor_running(!running, cx);
                        })),
                )
                .child(
                    Button::new("scan")
                        .label(if scanning { "扫描中…" } else { "扫描" })
                        .small()
                        .primary()
                        .disabled(scanning)
                        .on_click(cx.listener(|view, _, _, cx| view.services.restart_scan(cx))),
                )
                .child(
                    Button::new("panel")
                        .label(match self.panel {
                            SidePanel::Files => "结论",
                            SidePanel::Findings => "文件",
                        })
                        .small()
                        .ghost()
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.panel = match view.panel {
                                SidePanel::Files => SidePanel::Findings,
                                SidePanel::Findings => SidePanel::Files,
                            };
                            cx.notify();
                        })),
                ),
        )
    }

    fn render_volume_row(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Copy the colours out of the theme: `cx.theme()` borrows the app, and
        // the builder chains below also call `cx.listener`, which needs `cx`
        // mutably. Hsla values are `Copy`, so a local ends the borrow.
        let (border, _muted, _success, _warning) = {
            let t = cx.theme();
            (t.border, t.muted_foreground, t.success, t.warning)
        };
        let (volumes, current) = {
            let model = self.model.read(cx);
            (model.volumes().to_vec(), model.current_volume_id())
        };

        let mut row = h_flex().flex_shrink_0().items_center().gap_1().px_3().pb_2();
        for volume in volumes {
            let is_current = Some(volume.id) == current;
            let id = volume.id;
            row = row.child(
                Button::new(format!("volume-{}", volume.id.raw()))
                    .label(format!(
                        "{} · {}",
                        volume.name,
                        sift_core::format_bytes(volume.available_bytes)
                    ))
                    .xsmall()
                    .when(is_current, |button| button.selected(true))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.services.select_volume(id, cx);
                    })),
            );
        }

        // Breadcrumbs navigate inside the app, so they are Buttons, never Links.
        let crumbs: Vec<(NodeKey, String)> = {
            let model = self.model.read(cx);
            model
                .breadcrumbs()
                .iter()
                .map(|node| (node.key, node.name.clone()))
                .collect()
        };
        let mut trail = h_flex().gap_1().items_center().px_3().pb_2();
        for (index, (key, name)) in crumbs.iter().enumerate() {
            // Copy before the `move` closure so the id does not borrow `crumbs`.
            let key = *key;
            let is_last = index + 1 == crumbs.len();
            trail = trail.child(
                Button::new(format!("crumb-{index}"))
                    .label(name.clone())
                    .xsmall()
                    .when(is_last, |button| button.selected(true))
                    .on_click(cx.listener(move |view, _, _, cx| view.drill(key, cx))),
            );
        }

        v_flex()
            .flex_shrink_0()
            .border_b_1()
            .border_color(border)
            .child(row)
            .child(trail)
    }

    fn render_body(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Copy the colours out of the theme: `cx.theme()` borrows the app, and
        // the builder chains below also call `cx.listener`, which needs `cx`
        // mutably. Hsla values are `Copy`, so a local ends the borrow.
        let (border, _muted, _success, _warning) = {
            let t = cx.theme();
            (t.border, t.muted_foreground, t.success, t.warning)
        };
        h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .p_2()
                    .child(self.render_treemap(cx)),
            )
            .child(
                v_flex()
                    .w_80()
                    .flex_shrink_0()
                    .min_h_0()
                    .border_l_1()
                    .border_color(border)
                    .child(self.render_panel(cx)),
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

    fn render_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        match self.panel {
            SidePanel::Files => self.render_files(cx).into_any_element(),
            SidePanel::Findings => self.render_findings(cx).into_any_element(),
        }
    }

    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Copy the colours out of the theme: `cx.theme()` borrows the app, and
        // the builder chains below also call `cx.listener`, which needs `cx`
        // mutably. Hsla values are `Copy`, so a local ends the borrow.
        let (border, muted, success, _warning) = {
            let t = cx.theme();
            (t.border, t.muted_foreground, t.success, t.warning)
        };
        let rows: Vec<(NodeKey, String, String, bool, bool)> = {
            let model = self.model.read(cx);
            model
                .visible_entries()
                .iter()
                .take(ROW_LIMIT)
                .map(|node| {
                    (
                        node.key,
                        node.name.clone(),
                        node.size_label(),
                        node.is_dir,
                        model.is_selected(node.key),
                    )
                })
                .collect()
        };
        let total = self.model.read(cx).visible_entries().len();

        let mut list = v_flex().flex_1().min_h_0().p_1();
        for (key, name, size, is_dir, selected) in rows {
            let label = format!("{name}   {size}");
            list = list.child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new(format!("row-{}", key.raw()))
                            .label(label)
                            .xsmall()
                            .when(selected, |button| button.selected(true))
                            .on_click(cx.listener(move |view, _, _, cx| view.activate(key, cx))),
                    )
                    .when(selected, |row| {
                        row.child(div().text_xs().text_color(success).child("✓"))
                    })
                    .when(is_dir, |row| {
                        row.child(div().text_xs().text_color(muted).child("›"))
                    }),
            );
        }
        if total > ROW_LIMIT {
            list = list.child(
                div()
                    .p_2()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("还有 {} 项未显示（按大小排序）", total - ROW_LIMIT)),
            );
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .child(div().text_xs().child("文件与文件夹"))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(total.to_string()),
                    ),
            )
            .child(list)
    }

    fn render_findings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // Copy the colours out of the theme: `cx.theme()` borrows the app, and
        // the builder chains below also call `cx.listener`, which needs `cx`
        // mutably. Hsla values are `Copy`, so a local ends the borrow.
        let (border, muted, success, warning) = {
            let t = cx.theme();
            (t.border, t.muted_foreground, t.success, t.warning)
        };
        let analyzing = self.model.read(cx).is_analyzing();
        let findings: Vec<(NodeKey, String, String, u64, bool, bool)> = {
            let model = self.model.read(cx);
            model
                .findings()
                .iter()
                .map(|finding| {
                    (
                        finding.key,
                        finding.name.clone(),
                        render_reason(&finding.reason),
                        finding.size,
                        finding.safety == sift_analyze::Safety::Safe,
                        finding.approved_for_auto,
                    )
                })
                .collect()
        };
        let reclaimable = self.model.read(cx).reclaimable_bytes();
        let safe = self.model.read(cx).safe_bytes();

        let mut list = v_flex().flex_1().min_h_0().p_1();
        if findings.is_empty() {
            list = list.child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .p_4()
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child("还没有分析结论"),
                    )
                    .child(
                        Button::new("analyze-empty")
                            .label("开始分析")
                            .small()
                            .disabled(analyzing)
                            .on_click(cx.listener(|view, _, _, cx| view.services.run_analysis(cx))),
                    ),
            );
        }
        for (key, name, reason, size, is_safe, approved) in findings {
            let color = if is_safe { success } else { warning };
            list = list.child(
                v_flex().w_full().gap_1().p_2().child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().text_sm().child(name))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if is_safe { color } else { warning })
                                .child(if is_safe { "可直接清理" } else { "建议确认" }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(sift_core::format_bytes(size)),
                        )
                        .when(is_safe, |row| {
                            row.child(
                                Button::new(format!("approve-{}", key.raw()))
                                    .label(if approved { "取消自动" } else { "允许自动" })
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.services.set_auto_approval(key, !approved, cx);
                                    })),
                            )
                        }),
                )
                .child(div().text_xs().text_color(muted).child(reason)),
            );
        }

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .child(div().text_xs().child("分析结论"))
                    .child(div().flex_1())
                    .child(
                        div().text_xs().text_color(muted).child(format!(
                            "可回收 {} · 免确认 {}",
                            sift_core::format_bytes(reclaimable),
                            sift_core::format_bytes(safe)
                        )),
                    ),
            )
            .child(list)
    }

    fn render_footer(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Copy the colours out of the theme: `cx.theme()` borrows the app, and
        // the builder chains below also call `cx.listener`, which needs `cx`
        // mutably. Hsla values are `Copy`, so a local ends the borrow.
        let (border, muted, _success, _warning) = {
            let t = cx.theme();
            (t.border, t.muted_foreground, t.success, t.warning)
        };
        let (selected_count, selected_bytes, status) = {
            let model = self.model.read(cx);
            (
                model.selection().len(),
                model.selected_bytes(),
                model.status_line(),
            )
        };

        h_flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_t_1()
            .border_color(border)
            .child(div().text_sm().text_color(muted).child(status))
            .child(div().flex_1())
            .child(
                Button::new("analyze")
                    .label("分析")
                    .small()
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.services.run_analysis(cx);
                        view.panel = SidePanel::Findings;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("clean")
                    .label(if selected_count == 0 {
                        "清理".to_string()
                    } else {
                        format!(
                            "清理 {} 项 · {}",
                            selected_count,
                            sift_core::format_bytes(selected_bytes)
                        )
                    })
                    .small()
                    // A destructive commitment, but a reversible one: the items
                    // move to the trash, so the commit is a danger-styled button
                    // and the result is a notification naming what moved, rather
                    // than a modal.
                    .danger()
                    .disabled(selected_count == 0)
                    .on_click(cx.listener(|view, _, _, cx| view.services.trash_selection(cx))),
            )
    }

    /// Surface new model messages as notifications.
    ///
    /// This reads and advances a view-local counter during render. It does not
    /// call `notify`, so it cannot schedule another frame; the alternative would
    /// be threading a `Window` into the background signal pump, putting
    /// presentation state inside a service.
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
            let note = Notification::new().message(message).autohide(true);
            let note = match level {
                ToastLevel::Warning | ToastLevel::Error => note.title("Sift"),
                _ => note,
            };
            window.push_notification(note, cx);
        }
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_notifications(window, cx);

        let dialogs = gpui_kit::component::Root::render_dialog_layer(window, cx);
        let sheets = gpui_kit::component::Root::render_sheet_layer(window, cx);
        let notifications = gpui_kit::component::Root::render_notification_layer(window, cx);

        v_flex()
            .size_full()
            .key_context("Sift")
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::on_rescan))
            .on_action(cx.listener(Self::on_go_up))
            .on_action(cx.listener(Self::on_dismiss))
            .on_action(cx.listener(Self::on_activate))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_select_next))
            .child(self.render_title_bar(window, cx))
            .child(self.render_volume_row(window, cx))
            .child(self.render_body(window, cx))
            .child(self.render_footer(window, cx))
            .children(dialogs)
            .children(sheets)
            .children(notifications)
    }
}

/// Render a reason: a stable key goes through the local table, adjudicator-written
/// text is shown verbatim.
fn render_reason(reason: &sift_analyze::Reason) -> String {
    use sift_analyze::Reason;
    match reason {
        Reason::Text(text) => text.clone(),
        Reason::Key { key, params } => {
            let template = match key.as_str() {
                "reason.rebuildableCache" => "{} 的可重建目录，重新构建即可恢复",
                "reason.cacheDirectory" => "{} 的缓存，可自动重建",
                "reason.packageInstaller" => "{} 安装包，安装完成后通常不再需要",
                "reason.archive" => "{} 压缩包，确认内容后可删除",
                "reason.staleLargeFile" => "{} 天未修改的大文件",
                "reason.duplicate" => "与其它副本同名同大小（{}）",
                "reason.trash" => "回收站内容，清空即释放",
                "reason.tempFile" => "系统临时文件，可安全删除",
                "reason.log" => "{} 日志，排查完成后可删除",
                "reason.insideBundle" => "{} 位于应用包内部，删除会破坏该应用",
                "reason.aiConfirmed" => "经模型确认可删除",
                "reason.aiLowConfidence" => "模型置信度不足，建议人工确认",
                "reason.aiNotAuthorized" => "该类型不允许由模型判定为可直接清理",
                "reason.aiConsentRequired" => "尚未同意发送文件元数据，模型未参与判定",
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
        assert!(text.contains("重建"), "{text}");

        let unknown = Reason::key("reason.somethingNew");
        assert_eq!(render_reason(&unknown), "见规则说明");

        let verbatim = Reason::text("这是一个旧安装包");
        assert_eq!(render_reason(&verbatim), "这是一个旧安装包");
    }

    #[test]
    fn a_key_without_params_still_reads_as_a_sentence() {
        let reason = Reason::key("reason.trash");
        let text = render_reason(&reason);
        assert!(text.contains("回收站"), "{text}");
    }
}
