//! The native menu bar.
//!
//! Items carry actions rather than callbacks, so the bar and the keyboard go
//! through one path: the menu dispatches to whatever has focus, and GPUI prints
//! each item's shortcut from the keymap. That is why the bindings live in
//! `AppView::with_store` next to the actions they name — a menu item with no
//! binding shows no shortcut, and a binding with no item is invisible.
//!
//! macOS owns the first menu's name (the bundle's), so the first submenu here is
//! the application menu: settings and quit belong in it by convention.

use gpui_kit::{Menu, MenuItem, SystemMenuType};

use crate::app::{
    AnalyzeNow, CleanSelected, GoUp, OpenAiSettings, Quit, Rescan, SelectNext, SelectPrev,
    ShowAllVolumes, ToggleCandidates, ToggleMonitor,
};

/// The application's menus, in bar order.
pub fn app_menus() -> Vec<Menu> {
    vec![
        Menu::new("Sift").items(vec![
            MenuItem::action("AI 设置…", OpenAiSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("服务", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("退出 Sift", Quit),
        ]),
        Menu::new("文件").items(vec![
            MenuItem::action("重新扫描", Rescan),
            MenuItem::separator(),
            // The ellipsis is a promise: this opens the candidate list rather
            // than deleting anything by itself.
            MenuItem::action("清理所选…", CleanSelected),
        ]),
        Menu::new("查看").items(vec![
            MenuItem::action("上移一层", GoUp),
            MenuItem::separator(),
            MenuItem::action("上一项", SelectPrev),
            MenuItem::action("下一项", SelectNext),
            MenuItem::separator(),
            MenuItem::action("显示/隐藏清理候选", ToggleCandidates),
            MenuItem::action("所有磁盘", ShowAllVolumes),
        ]),
        Menu::new("AI").items(vec![
            MenuItem::action("立即分析", AnalyzeNow),
            MenuItem::action("自动清理开关", ToggleMonitor),
            MenuItem::separator(),
            MenuItem::action("AI 设置…", OpenAiSettings),
        ]),
    ]
}

/// The Dock menu: the actions worth reaching without raising the window.
///
/// GPUI exposes no status-bar item, so the Dock icon is the only place a
/// long-running watch can offer a shortcut from outside the window.
pub fn dock_menu() -> Vec<MenuItem> {
    vec![
        MenuItem::action("重新扫描", Rescan),
        MenuItem::action("显示/隐藏清理候选", ToggleCandidates),
        MenuItem::separator(),
        MenuItem::action("AI 设置…", OpenAiSettings),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_has_items_and_a_name() {
        let menus = app_menus();
        assert!(!menus.is_empty());
        for menu in &menus {
            assert!(!menu.name.is_empty(), "a menu needs a name");
            assert!(!menu.items.is_empty(), "{} is empty", menu.name);
        }
        // The application menu comes first, as macOS expects.
        assert_eq!(menus[0].name.as_ref(), "Sift");
    }

    #[test]
    fn the_dock_menu_offers_the_watch_actions() {
        let items = dock_menu();
        assert!(items.len() >= 3, "a Dock menu with nothing in it is noise");
    }
}
