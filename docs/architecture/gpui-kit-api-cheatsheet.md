# GPUI Kit 0.6.6 / GPUI (`gpui-pre`) 0.3.6 — exact API cheat-sheet

Extracted verbatim from vendored sources. Every signature is followed by `file:line`.
Crate roots referenced below (abbreviated):

- `KIT` = `/Users/xiaocy/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-kit-0.6.6`
- `CMP` = `/Users/xiaocy/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-component-0.6.6`
- `BASE` = `/Users/xiaocy/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-base-0.6.6`
- `ASSETS` = `/Users/xiaocy/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-kit-assets-0.6.6`
- `GPUI` = `/Users/xiaocy/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-0.3.6`
- `PLATFORM` = `/Users/xiaocy/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/gpui-pre-platform-0.3.6`

`gpui_kit::*` is a glob re-export of `::gpui::*` (`KIT/src/lib.rs:81`), plus
`gpui_kit::base` = `::gpui_base`, `gpui_kit::component` = `::gpui_component`,
`gpui_kit::assets` = `::gpui_kit_assets`, `gpui_kit::platform` = `::gpui_platform`.

---

## 1. App bootstrap

### 1.1 Entry points

```rust
// KIT/src/lib.rs:146
pub fn init(cx: &mut App)

// PLATFORM/src/gpui_platform.rs:13   (re-exported as gpui_kit::application, KIT/src/lib.rs:140)
pub fn application() -> gpui::Application

// PLATFORM/src/gpui_platform.rs:23
pub fn headless() -> gpui::Application

// GPUI/src/app.rs:199
pub fn with_assets(self, asset_source: impl AssetSource) -> Self

// GPUI/src/app.rs:234   (this is the only run entry used by the crate's own docs)
pub fn run<F>(self, on_finish_launching: F)
where
    F: 'static + FnOnce(&mut App),
```

`gpui_kit::init` calls `gpui_component::init(cx)` when the `component` feature is on (it is on by
default); otherwise `gpui_base::init(cx)` (`KIT/src/lib.rs:146-153`). `gpui_component::init` in
turn initializes theme, global_state, root, `gpui_base`, input, date_picker, dock, sheet, list,
command, carousel, notification, popover, menu, table, tooltip (`CMP/src/lib.rs:131-150`).
**`init` must run before any component is constructed** (all kit tests do `cx.update(gpui_kit::init)`
first, e.g. `KIT/tests/overlays.rs:94`).

Registering assets is a separate call on `Application`:

```rust
// ASSETS/src/lib.rs:22 (doc) / ASSETS/src/native_assets.rs
let app = gpui_platform::application().with_assets(gpui_kit::assets::Assets);
```

### 1.2 Complete compiling-shaped `main()` (from the crate's own doc/test usage)

The exact shape used by `KIT/src/lib.rs:24-41` (the crate's own doc example, verbatim except for
crate-path unqualified names) and by `KIT/tests/overlays.rs:92-101`:

```rust
use gpui_kit::component::Root;
use gpui_kit::*;

struct Hello;

impl Render for Hello {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("Hello, World!")
    }
}

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let view = cx.new(|_| Hello);
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("failed to open window");
        })
        .detach();
    });
}
```

Note carefully: under `run`, `cx` is `&mut App` (`GPUI/src/app.rs:234`), and `App::spawn` is
`pub fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R> where AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R`
(`GPUI/src/app.rs:2036-2039`) — the closure is `async move |cx|` and `cx` is `&mut AsyncApp`; the
returned `Task` is `.detach()`ed. There is **no `background_spawn` method anywhere** on `App`,
`AsyncApp`, `Context<T>` or `Window`; use `cx.background_executor().spawn(future)` instead
(`GPUI/src/app.rs:2012-2015`, `GPUI/src/executor.rs:345`).

### 1.3 `open_window`

```rust
// GPUI/src/app.rs:1347-1351  (on &mut App)
pub fn open_window<V: 'static + Render>(
    &mut self,
    options: crate::WindowOptions,
    build_root_view: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> anyhow::Result<WindowHandle<V>>

// GPUI/src/app/async_context.rs:193-200  (on &AsyncApp — this is what `cx.open_window` resolves to inside cx.spawn)
pub fn open_window<V>(
    &self,
    options: crate::WindowOptions,
    build_root_view: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Result<WindowHandle<V>>
where
    V: 'static + Render,
```

### 1.4 `WindowOptions` fields that matter

`GPUI/src/platform.rs:2173-2243`, `Default` at `GPUI/src/platform.rs:2345-2371`.

```rust
pub struct WindowOptions {
    pub window_bounds: Option<WindowBounds>,        // platform.rs:2178
    pub titlebar: Option<TitlebarOptions>,          // platform.rs:2181  (Default: Some(..))
    pub focus: bool,                                // platform.rs:2184  (Default: true)
    pub show: bool,                                 // platform.rs:2187  (Default: true)
    pub kind: WindowKind,                           // platform.rs:2190
    pub is_movable: bool,                           // platform.rs:2195  (Default: true)
    pub app_owns_titlebar_drag: bool,               // platform.rs:2208  (Default: false)
    pub inactive_frame_interval: Option<Duration>,  // platform.rs:2213
    pub is_resizable: bool,                         // platform.rs:2216  (Default: true)
    pub is_minimizable: bool,                       // platform.rs:2219  (Default: true)
    pub display_id: Option<DisplayId>,              // platform.rs:2223
    pub window_background: WindowBackgroundAppearance, // platform.rs:2226
    pub app_id: Option<String>,                     // platform.rs:2229  <-- app_id EXISTS
    pub window_min_size: Option<Size<Pixels>>,      // platform.rs:2232  <-- min size
    pub window_decorations: Option<WindowDecorations>, // platform.rs:2236
    pub icon: Option<Arc<image::RgbaImage>>,        // platform.rs:2239 (X11 only)
    pub tabbing_identifier: Option<String>,         // platform.rs:2242
}
```

There is **no `title` field on `WindowOptions`** — the title lives in `TitlebarOptions`:

```rust
// GPUI/src/platform.rs:2374-2385
#[derive(Debug, Default)]
pub struct TitlebarOptions {
    pub title: Option<SharedString>,
    pub appears_transparent: bool,
    pub traffic_light_position: Option<Point<Pixels>>,
}

// GPUI/src/platform.rs:2388-2405
pub enum WindowKind { Normal, PopUp, AnchoredPopup(popup::PopupOptions), ... }
```

`TitleBar::window_options()` returns the recommended base for a window that draws a `TitleBar`:

```rust
// CMP/src/title_bar.rs:81-93
pub fn window_options() -> WindowOptions {
    WindowOptions {
        titlebar: Some(Self::title_bar_options()),
        app_owns_titlebar_drag: true,
        ..Default::default()
    }
}

// CMP/src/title_bar.rs:59-66
pub fn title_bar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: None,
        appears_transparent: true,
        traffic_light_position: Some(gpui::point(px(9.0), px(9.0))),
    }
}
```

Doc example for the intended composition (`CMP/src/title_bar.rs:73-80`):

```rust
let options = WindowOptions {
    window_min_size: None,
    ..TitleBar::window_options()
};
```

---

## 2. Root and overlays

### 2.1 `Root`

```rust
// CMP/src/root.rs:37-56   (struct)
/// Root is a view for the App window for as the top level view (Must be the first view in the window).
pub struct Root { /* private fields */ }

// CMP/src/root.rs:100
pub fn new(view: impl Into<AnyView>, window: &mut Window, cx: &mut Context<Self>) -> Self

// CMP/src/root.rs:143
pub fn bordered(mut self, bordered: bool) -> Self

// CMP/src/root.rs:151
pub fn window_shadow_size(mut self, size: impl Into<Pixels>) -> Self

// CMP/src/root.rs:156-159
pub fn update<F, R>(window: &mut Window, cx: &mut App, f: F) -> R
where
    F: FnOnce(&mut Self, &mut Window, &mut Context<Self>) -> R,

// CMP/src/root.rs:176
pub fn read<'a>(window: &'a Window, cx: &'a App) -> &'a Self

// CMP/src/root.rs:488
pub fn view(&self) -> &AnyView
```

`Root::new` panics downstream if the window's first view is not a `Root` — `Root::update` has
`.expect("BUG: window first layer should be a gpui_component::Root.")` (`CMP/src/root.rs:160-163`).
Every kit test that uses overlays opens with `Root::new(view, window, cx)`
(`KIT/tests/overlays.rs:98-101`, `KIT/tests/menu.rs:48-52`, `KIT/tests/search.rs:62-71`).

Three layer renderers must be appended by the app view inside `render` (they return
`Option<impl IntoElement>` and are `None` until something is open):

```rust
// CMP/src/root.rs:185-188
pub fn render_notification_layer(window: &mut Window, cx: &mut App) -> Option<impl IntoElement + use<>>
// CMP/src/root.rs:215-218
pub fn render_sheet_layer(window: &mut Window, cx: &mut App) -> Option<impl IntoElement + use<>>
// CMP/src/root.rs:242-245
pub fn render_dialog_layer(window: &mut Window, cx: &mut App) -> Option<impl IntoElement + use<>>
```

Real usage (`KIT/tests/overlays.rs:20-86`, condensed but exact in call shape):

```rust
fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
    let dialogs = Root::render_dialog_layer(window, cx);
    let sheets = Root::render_sheet_layer(window, cx);
    let notifications = Root::render_notification_layer(window, cx);
    div().size_full()
        .children(dialogs)
        .children(sheets)
        .children(notifications)
}
```

### 2.2 `WindowExt` — exact parameter lists

`CMP/src/window_ext.rs:12-107` (trait), `impl WindowExt for Window` at `CMP/src/window_ext.rs:109`.

```rust
pub trait WindowExt: Sized {
    fn open_sheet<F>(&mut self, cx: &mut App, build: F)
    where F: Fn(Sheet, &mut Window, &mut App) -> Sheet + 'static;                     // :14

    fn open_sheet_at<F>(&mut self, placement: Placement, cx: &mut App, build: F)
    where F: Fn(Sheet, &mut Window, &mut App) -> Sheet + 'static;                     // :18

    fn has_active_sheet(&mut self, cx: &mut App) -> bool;                             // :22
    fn close_sheet(&mut self, cx: &mut App);                                          // :25

    fn open_dialog<F>(&mut self, cx: &mut App, build: F)
    where F: Fn(Dialog, &mut Window, &mut App) -> Dialog + 'static;                   // :28

    fn open_alert_dialog<F>(&mut self, cx: &mut App, build: F)
    where F: Fn(AlertDialog, &mut Window, &mut App) -> AlertDialog + 'static;         // :46

    fn has_active_dialog(&mut self, cx: &mut App) -> bool;                            // :49
    fn close_dialog(&mut self, cx: &mut App);                                         // :52
    fn close_all_dialogs(&mut self, cx: &mut App);                                    // :55

    fn push_notification(&mut self, note: impl Into<Notification>, cx: &mut App);     // :58

    fn remove_notification<T: Sized + 'static>(&mut self, cx: &mut App);              // :62
    fn remove_notification1<T: Sized + 'static>(&mut self, key: impl Into<ElementId>, cx: &mut App); // :65
    fn clear_notifications(&mut self, cx: &mut App);                                  // :68
    fn notifications(&mut self, cx: &mut App) -> Rc<Vec<Entity<Notification>>>;       // :71

    fn focused_input(&mut self, cx: &mut App) -> Option<AnyInputState>;               // :77
    fn has_focused_input(&mut self, cx: &mut App) -> bool;                            // :79
}
```

All of these require `&mut Window` (they are methods on `Window`), so they are only callable from
`Render::render`, `on_click` handlers, `cx.listener`, etc. `open_sheet` = `open_sheet_at(Placement::Right, ..)`
(`CMP/src/window_ext.rs:111-113`).

### 2.3 One real call of each

Dialog (`KIT/tests/overlays.rs:29-63`):

```rust
Button::new("edit").label("Edit…").on_click(move |_, window, cx| {
    let draft = draft.clone();
    let saved = saved.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        let draft = draft.clone();
        let saved = saved.clone();
        dialog
            .title("Edit profile")
            .child(Input::new(&draft).id("name"))
            .footer(DialogFooter::new().child(
                DialogAction::new().child(Button::new("ok").label("Save")),
            ))
            .on_ok(move |_, window, cx| {
                if draft.read(cx).value().is_empty() { return false; }
                let value = draft.read(cx).value();
                saved.update(cx, |input, cx| input.set_value(value, window, cx));
                window.push_notification(Notification::new().message("Profile saved").autohide(false), cx);
                true
            })
    });
})
```

Sheet (`KIT/tests/overlays.rs:68-77`):

```rust
Button::new("inspect").label("Inspect…").on_click(|_, window, cx| {
    window.open_sheet(cx, |sheet, _, _| {
        sheet.title("Inspector").child("Details")
    });
})
```

Notification (`KIT/tests/overlays.rs:81-88`):

```rust
Button::new("notify").label("Notify").on_click(|_, window, cx| {
    window.push_notification(Notification::new().message("Updated").autohide(true), cx);
})
```

Alert dialog (`KIT/tests/overlays.rs:~305-315`):

```rust
window.open_alert_dialog(cx, |dialog, _, _| {
    dialog.title("Stolen focus").confirm()
});
```

### 2.4 `Dialog` builder

`CMP/src/dialog/dialog.rs`.

```rust
pub fn new(cx: &mut App) -> Self                                                     // :287
pub fn trigger(mut self, trigger: impl IntoElement) -> Self                          // :308
pub fn content<F>(mut self, builder: F) -> Self
    where F: Fn(DialogContent, &mut Window, &mut App) -> DialogContent + 'static;    // :314-317
pub fn title(mut self, title: impl IntoElement) -> Self                              // :323
pub fn footer(mut self, footer: impl IntoElement) -> Self                            // :339
pub fn button_props(mut self, button_props: DialogButtonProps) -> Self               // :345
pub fn on_close(mut self, on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self // :358-361
pub fn on_ok(mut self, on_ok: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static) -> Self // :369-372
pub fn on_cancel(mut self, on_cancel: impl Fn(&ClickEvent, &mut Window, &mut App) -> bool + 'static) -> Self // :380-383
pub fn close_button(mut self, close_button: bool) -> Self                            // :389
pub fn margin_top(mut self, margin_top: impl Into<Pixels>) -> Self                   // :395
pub fn w(mut self, width: impl Into<Pixels>) -> Self                                 // :405
pub fn width(mut self, width: impl Into<Pixels>) -> Self                             // :413
pub fn max_w(mut self, max_width: impl Into<Pixels>) -> Self                         // :419
pub fn overlay(mut self, overlay: bool) -> Self                                      // :425
pub fn overlay_closable(mut self, overlay_closable: bool) -> Self                    // :433
pub fn keyboard(mut self, keyboard: bool) -> Self                                    // :439
```

Important: `on_ok`/`on_cancel` return `bool` — returning `false` keeps the dialog open
(`CMP/src/dialog/dialog.rs:366-368`, `:377-379`). Setting `footer(...)` supersedes `button_props`
(`CMP/src/dialog/dialog.rs:335-338`). `Dialog` is also `ParentElement` + `Styled`
(`CMP/src/dialog/dialog.rs:465-477`), which is why `.child(...)` works.

`DialogFooter` / `DialogClose` / `DialogAction` (`CMP/src/dialog/footer.rs`):

```rust
impl DialogFooter { pub fn new() -> Self }                                            // :31
impl ParentElement for DialogFooter                                                    // :39  (so .child/.children)
impl DialogClose  { pub fn new() -> Self }                                            // :70
pub fn trigger<E: IntoElement>(mut self, build: impl FnOnce(Button) -> E) -> Self     // :76-77
impl DialogAction { pub fn new() -> Self }                                            // :115
```

`AlertDialog` builder (`CMP/src/dialog/alert_dialog.rs`): `new(cx: &mut App)` :79, `confirm()` :96,
`trigger(impl IntoElement)` :109, `content<F>(...)` :132, `footer(impl IntoElement)` :146,
`icon(impl IntoElement)` :161, `title(impl IntoElement)` :169, `description(impl IntoElement)` :177,
`button_props(DialogButtonProps)` :199, `width(impl Into<Pixels>)` :206,
`show_cancel(bool)` :212, `overlay_closable(bool)` :219, `close_button(bool)` :224,
`keyboard(bool)` :230, `on_close(..)` :238, `on_ok(..)` :249, `on_cancel(..)` :260.

---

## 3. `TitleBar`

`CMP/src/title_bar.rs`.

```rust
pub const TITLE_BAR_HEIGHT: Pixels = px(34.);                        // :19

#[derive(IntoElement)]
pub struct TitleBar { /* style, children, on_close_window */ }        // :42-46

impl TitleBar {
    pub fn new() -> Self                                              // :50
    pub fn title_bar_options() -> TitlebarOptions                     // :59
    pub fn window_options() -> WindowOptions                          // :81
    pub fn on_close_window(
        mut self,
        f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self                                                          // :95-99  (Linux only; no-op elsewhere)
}

impl Styled for TitleBar      // :296  -> .bg(), .h(), .border_b_1(), ...
impl ParentElement for TitleBar // :302 -> .child(), .children()
impl RenderOnce for TitleBar   // :319
```

**Exact answers to the questions asked:**

- There is **no `TitleBar::title(...)` method**. Grep of `CMP/src/title_bar.rs` shows only
  `new`, `title_bar_options`, `window_options`, `on_close_window`. Set the OS title via
  `WindowOptions { titlebar: Some(TitlebarOptions { title: Some("…".into()), .. }), .. }`, or render
  a title element as a `TitleBar` child.
- There is **no `tooltip(...)` method** and **no platform-style builder** on `TitleBar`.
- There is no explicit "must be inside `Root`" requirement; `TitleBar` is a plain `IntoElement`
  with no `Root` lookup. Its own render creates an inner `div().flex_shrink_0()` so it is normally
  the first child of a `v_flex()` at the window top. It calls `window.start_window_move()`,
  `window.window_decorations()`, `window.titlebar_double_click()` / `window.zoom_window()` at paint
  time (`CMP/src/title_bar.rs:325-410`), and registers a key context/id `"title-bar"` for tests.
- `TitleBar` renders its own window controls (`WindowControls`, `CMP/src/title_bar.rs:252`) and a
  right-side close button; the close path uses `window.remove_window()` unless `on_close_window` is set.

Real usage is not present in `KIT/tests/*.rs`; the only in-repo construction evidence is the doc
snippet at `CMP/src/title_bar.rs:73-80`.

---

## 4. `Button`

`CMP/src/button/button.rs`.

### 4.1 Construction and id type

```rust
pub fn new(id: impl Into<ElementId>) -> Self        // :233
```

`id` is `impl Into<ElementId>`, so `&str`, `String`, `usize`, `ElementId` all work (tests use
`Button::new("edit")` and `Button::new(0usize)` style ids; `IndexPath` implements `Into<ElementId>`
in `BASE/src/index_path.rs`).

### 4.2 Inherent builders (exact)

```rust
pub fn role(mut self, role: impl Into<RoleOverride>) -> Self                                  // :312
pub fn outline(mut self) -> Self                                                              // :318
pub fn rounded(mut self, rounded: impl Into<ButtonRounded>) -> Self                            // :324
pub fn label(mut self, label: impl Into<SharedString>) -> Self                                 // :357
pub fn accessibility_id(mut self, id: impl Into<SharedString>) -> Self                         // :363
pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self                   // :377
pub fn icon(mut self, icon: impl Into<ButtonIcon>) -> Self                                     // :383
pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self                             // :389
pub fn tooltip_placement(mut self, placement: Placement) -> Self                               // :398
pub fn tooltip_with_action(...)                                                                // :404
pub fn loading(mut self, loading: bool) -> Self                                                // :421
pub fn compact(mut self) -> Self                                                               // :427
pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self // :433-436
pub fn on_hover(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self     // :442
pub fn loading_icon(mut self, icon: impl Into<Icon>) -> Self                                   // :450
pub fn tab_index(mut self, tab_index: isize) -> Self                                           // :458
pub fn tab_stop(mut self, tab_stop: bool) -> Self                                              // :466
pub fn dropdown_caret(mut self, dropdown_caret: bool) -> Self                                  // :472
pub fn toggled(mut self, toggled: bool) -> Self                                                // :483 (a11y only)
```

**`on_click` closure signature is exactly `Fn(&ClickEvent, &mut Window, &mut App)`** — argument 1 is
`&ClickEvent` (not `&MouseEvent`), and there is no `cx.listener` sugar on `Button` itself; use
`cx.listener(|this, ev, window, cx| …)` (which yields the same 4-arg shape) as in
`KIT/tests/components.rs:22-26`.

### 4.3 Trait-provided builders (same call syntax)

```rust
// ButtonVariants, CMP/src/button/button.rs:44-96
pub trait ButtonVariants: Sized {
    fn with_variant(self, variant: ButtonVariant) -> Self;
    fn primary(self) -> Self;    // :48
    fn secondary(self) -> Self;  // :53
    fn danger(self) -> Self;     // :58
    fn warning(self) -> Self;    // :63
    fn success(self) -> Self;    // :68
    fn info(self) -> Self;       // :73
    fn ghost(self) -> Self;      // :78
    fn link(self) -> Self;       // :83
    fn text(self) -> Self;       // :88
    fn custom(self, style: ButtonCustomVariant) -> Self;  // :93
}

// CMP/src/button/button.rs:504-507
impl Disableable for Button { fn disabled(mut self, disabled: bool) -> Self }

// CMP/src/button/button.rs:517-523
impl Selectable for Button {
    fn selected(mut self, selected: bool) -> Self;
    fn is_selected(&self) -> bool;
}

// CMP/src/button/button.rs:526-530
impl Sizable for Button { fn with_size(mut self, size: impl Into<Size>) -> Self }

// CMP/src/component_traits.rs (re-exported at CMP/src/lib.rs:? / BASE/src/component_traits.rs:98)
pub trait Disableable { fn disabled(self, disabled: bool) -> Self; }   // BASE/src/component_traits.rs
pub trait Selectable  { fn selected(self, selected: bool) -> Self; fn is_selected(&self) -> bool; }
```

`Sizable` (`CMP/src/sizing.rs:178-202`):

```rust
pub trait Sizable: Sized {
    fn with_size(mut self, size: impl Into<Size>) -> Self;   // :183
    fn xsmall(self) -> Self;                                 // :187  -> Size::XSmall
    fn small(self) -> Self;                                  // :193  -> Size::Small
    fn large(self) -> Self;                                  // :199  -> Size::Large
}

// CMP/src/sizing.rs:6-13
pub enum Size { Size(Pixels), XSmall, Small, #[default] Medium, Large }
```

So `Button::new("x").small()`, `.xsmall()`, `.large()` all exist (they come from `Sizable`).
There is **no `medium()` method** — `Size::Medium` is the default.

`ButtonVariant` (`CMP/src/button/button.rs:142`): `Default, Primary, Secondary, Danger, Warning,
Success, Info, Ghost, Link, Text, Custom(ButtonCustomVariant)`.

`ButtonRounded` (`CMP/src/button/button.rs:20`): `None, Small, Medium, Large, Full` (Medium default,
set at `:243`).

### 4.4 `IconButton`

**There is no public `IconButton` type.** `CMP/src/button/button_icon.rs` defines
`pub struct ButtonIcon` marked `#[doc(hidden)]` (`:8-9`) and `pub enum ButtonIconVariant`
(`:55`); `button_icon` is exported only `pub(crate)` from the module
(`CMP/src/button/mod.rs:9`: `pub(crate) use button_icon::*;`). An icon-only button is spelled
`Button::new("id").icon(IconName::X)` (the button auto-detects icon-only via
`is_icon_only()` at `CMP/src/button/button.rs:308`).

### 4.5 `ButtonGroup` and `DropdownButton`

```rust
// CMP/src/button/button_group.rs
pub struct ButtonGroup                                        // :18
pub fn new(id: impl Into<ElementId>) -> Self                   // :44
pub fn child(mut self, child: Button) -> Self                  // :61
pub fn children(mut self, children: impl IntoIterator<Item = Button>) -> Self // :67
pub fn multiple(mut self, multiple: bool) -> Self              // :73
pub fn layout(mut self, layout: Axis) -> Self                  // :79
pub fn compact(mut self) -> Self                               // :87
pub fn outline(mut self) -> Self                               // :95
pub fn on_click(mut self, handler: impl Fn(&Vec<usize>, &mut Window, &mut App) + 'static) -> Self // :123-128
impl Sizable for ButtonGroup                                   // :134  (.small(), .with_size(..))
impl Styled for ButtonGroup                                    // :141
impl ButtonVariants for ButtonGroup                            // :145

// CMP/src/button/dropdown_button.rs
pub struct DropdownButton                                      // :25
pub fn new(id: impl Into<ElementId>) -> Self                   // :43
pub fn button(mut self, button: Button) -> Self                // :76
pub fn dropdown_menu(/* :82 */)                                // (PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu
pub fn dropdown_menu_with_anchor(/* :91 */)                    // same, with anchor first
pub fn outline(mut self) -> Self                               // :104
impl ButtonVariants for DropdownButton                          // :130
```

---

## 5. Icons

### 5.1 `Icon`

```rust
// CMP/src/icon.rs:59-64
impl<T: IconNamed> From<T> for Icon { fn from(value: T) -> Self { Icon::build(value) } }

// CMP/src/icon.rs:105
pub fn new(icon: impl Into<Icon>) -> Self
// CMP/src/icon.rs:117
pub fn path(mut self, path: impl Into<SharedString>) -> Self
// CMP/src/icon.rs:136
pub fn data(mut self, data: &[u8]) -> Self
// CMP/src/icon.rs:147
pub fn view(self, cx: &mut App) -> Entity<Icon>
// CMP/src/icon.rs:152
pub fn transform(mut self, transformation: gpui::Transformation) -> Self
// CMP/src/icon.rs:157
pub fn empty() -> Self
// CMP/src/icon.rs:164
pub fn rotate(mut self, radians: impl Into<Radians>) -> Self
```

`Icon` is `IntoElement` and `Sizable`, so `Icon::new(IconName::Search).size_4()`,
`.text_color(cx.theme().muted_foreground)`, `.size(px(16.))` are all valid. `IconName` itself is
`IntoElement` and `RenderOnce`, so `.child(IconName::Search)` renders an icon at the current text
size/color:

```rust
// ASSETS/src/icon.rs:23-32
impl RenderOnce for IconName {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let text_style = window.text_style();
        svg().path(self.path()).flex_shrink_0()
            .size(text_style.font_size.to_pixels(window.rem_size()))
            .text_color(text_style.color)
    }
}
```

`IconNamed` trait (`ASSETS/src/icon.rs:5-7`): `pub trait IconNamed { fn path(self) -> SharedString; }`.
`gpui_kit::component::IconName` is a component-compat enum whose variants convert into
`gpui_kit_assets::IconName` (`CMP/src/icon.rs:16-38`), and `IconName::view(self, cx)` is inherent
on the component type (`CMP/src/icon.rs:42`); `IconNameExt::view` covers the assets type
(`CMP/src/icon.rs:67-73`).

### 5.2 Target spellings — exact

Checked against the vendored SVG catalog in
`ASSETS/assets/icons/` (variant name = each `-`-separated part capitalized; `ASSETS/build.rs:22-33`):

| asked for | exact variant | exists? |
| --- | --- | --- |
| folder | `IconName::Folder` | yes |
| hard_drive | `IconName::HardDrive` | yes |
| trash | `IconName::Trash` | yes |
| x | `IconName::X` | yes (also `IconName::Close` from `close.svg`) |
| chevron_right | `IconName::ChevronRight` | yes |
| chevron_down | `IconName::ChevronDown` | yes |
| check | `IconName::Check` | yes (also `CircleCheck`, `CheckCheck`, `CheckLine`) |
| refresh | — | **no `IconName::Refresh`**; the exact spellings are `IconName::RefreshCw`, `RefreshCcw`, `RefreshCwOff`, `RefreshCcwDot`, `RotateCw`, `RotateCcw`, `RotateCwSquare`, `RotateCcwSquare` |
| alert | — | **no `IconName::Alert`**; exact spellings are `TriangleAlert`, `CircleAlert`, `OctagonAlert`, `BadgeAlert`, `BellRing` |
| search | `IconName::Search` | yes (also `SearchAlert`, `SearchCheck`, `SearchCode`, `SearchSlash`, `SearchX`, `FileSearch`) |
| settings | `IconName::Settings` | yes (also `Settings2`, `Cog`) |
| archive | `IconName::Archive` | yes (also `ArchiveX`, `ArchiveRestore`, `FileArchive`, `FolderArchive`) |
| file | `IconName::File` | yes (also `FileText`, `FileCode`, `FilePlus`, `FileX`, …) |
| info | `IconName::Info` | yes (also `BadgeInfo`) |
| sparkles | `IconName::Sparkles` | yes (also `Sparkle`, `WandSparkles`, `PencilSparkles`) |

Note: there is no `Home` variant; the Lucide "home" icon is named `House` (plus `HouseHeart`,
`HousePlug`, `HousePlus`, `HouseWifi`).

### 5.3 Component default icon set (the ~100 variants on `gpui_kit::component::IconName`)

From `ASSETS/default-icons.txt` (exact variant names after PascalCase conversion):

`ALargeSmall, ArrowDown, ArrowLeft, ArrowRight, ArrowUp, Asterisk, BatteryCharging, BatteryFull,
BatteryLow, BatteryMedium, BatteryWarning, Battery, Bell, BookOpen, Bot, Building2, Calendar,
CaseSensitive, ChartPie, Check, ChevronDown, ChevronLeft, ChevronRight, ChevronUp, ChevronsUpDown,
CircleCheck, CircleUser, CircleX, Close, Copy, Cpu, Dash, Delete, EllipsisVertical, Ellipsis,
ExternalLink, EyeOff, Eye, FileText, File, FolderClosed, FolderOpen, Folder, Frame,
GalleryVerticalEnd, Github, Globe, HardDrive, HeartOff, Heart, Inbox, Info, Inspector,
LayoutDashboard, LoaderCircle, Loader, Map, Maximize, MemoryStick, Menu, Minimize, Minus, Moon,
Network, Palette, PanelBottomOpen, PanelBottom, PanelLeftClose, PanelLeftOpen, PanelLeft,
PanelRightClose, PanelRightOpen, PanelRight, Pause, Play, Plus, Redo2, Redo, Replace, ResizeCorner,
RotateCw, Search, Settings2, Settings, SortAscending, SortDescending, SquareTerminal, StarFill,
StarOff, Star, Sun, ThumbsDown, ThumbsUp, TriangleAlert, Undo2, Undo, User, WindowClose,
WindowMaximize, WindowMinimize, WindowRestore`

### 5.4 Full `gpui_kit_assets::IconName` catalog

Generated by `ASSETS/build.rs:9-70` into `$OUT_DIR/icon_name.rs` from every `*.svg` under
`ASSETS/assets/icons/` (1830 files as vendored). The generated enum is
`#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, gpui::IntoElement)] pub enum IconName { … }`
with `IconName::ALL: &'static [Self]` and `IconName::path(self) -> gpui::SharedString`
(the match arms map variant → `"icons/<stem>.svg"`).

The **complete, authoritative variant list** is the set of file stems in
`ASSETS/assets/icons/` with this transform applied:
split the stem on `-`, `_`, `.`; uppercase the first char of each part and lowercase the rest;
concatenate. (e.g. `hard-drive.svg` → `HardDrive`, `arrow-down-01.svg` → `ArrowDown01`,
`git-compare-arrows.svg` → `GitCompareArrows`.)

To enumerate exactly at build time in an app:

```rust
for name in gpui_kit::assets::IconName::ALL { /* name: IconName */ }
```

The 1830 generated variant names (verbatim, comma-separated) are appended at the end of this file
in section 15 so the body of the document stays readable.

---

## 6. Theme tokens

### 6.1 How to read

```rust
// CMP/src/theme/mod.rs:44-46
pub trait ActiveTheme { fn theme(&self) -> &Theme; }
// CMP/src/theme/mod.rs:48-53
impl ActiveTheme for App { fn theme(&self) -> &Theme { Theme::global(self) } }
```

`ActiveTheme` is implemented **only for `App`** (`CMP/src/theme/mod.rs:48`). In `Render::render`,
`cx: &mut Context<Self>` derefs to `App`, so `cx.theme()` works. There is **no `ActiveTheme for
Window` and no `cx.theme()` on `Window`** — inside a `&mut Window`-only callback you must go
through `App`. Kit tests rely on this: `KIT/tests/rendering.rs:61-62`, `KIT/tests/exports.rs:90-91`.

`Theme` derefs to `ThemeColor`, so `cx.theme().background` is field access through `Deref`
(`CMP/src/theme/mod.rs:169-181`), not a method.

```rust
// CMP/src/theme/mod.rs:107-160 (fields)
pub struct Theme {
    pub colors: ThemeColor,
    pub tokens: ThemeTokens,
    pub highlight_theme: Arc<HighlightTheme>,
    pub light_theme: Rc<ThemeConfig>,
    pub dark_theme: Rc<ThemeConfig>,
    pub mode: ThemeMode,
    pub font_family: SharedString,
    pub font_size: Pixels,          // :130  default 16px
    pub mono_font_family: SharedString,
    pub mono_font_size: Pixels,     // default 13px
    pub radius: Pixels,             // :142  "Radius for the general elements"
    pub radius_lg: Pixels,          // :144  "Radius for the large elements, e.g.: Dialog, Notification border radius"
    pub shadow: bool,
    pub focus_ring: bool,           // :152 default true
    pub transparent: Hsla,
    pub scrollbar_mode: ScrollbarMode,
    pub notification: NotificationSettings,
    pub list: ListSettings,
    pub sheet: SheetSettings,
    pub motion: MotionTokens,
}
```

### 6.2 `ThemeColor` — full semantic color field list

`CMP/src/theme/theme_color.rs:59-…` (all fields are `pub …: Hsla`). Grouped for reading:

- Surfaces / text: `background` :67, `foreground` :163, `muted` (muted backgrounds; Skeleton/Switch),
  `muted_foreground`, `popover`, `popover_foreground`, `secondary`, `secondary_foreground`,
  `secondary_hover`, `secondary_active`, `accent`, `accent_foreground`, `group_box`,
  `group_box_foreground`, `skeleton`, `overlay`, `window_border` (Linux only), `transparent`.
- Brand / status: `primary`, `primary_foreground`, `primary_hover`, `primary_active`, `danger`,
  `danger_foreground`, `danger_hover`, `danger_active`, `warning`, `warning_foreground`,
  `warning_hover`, `warning_active`, `success`, `success_foreground`, `success_hover`,
  `success_active`, `info`, `info_foreground`, `info_hover`, `info_active`.
- Lines / fields: `border`, `ring`, `input`, `selection`, `caret`.
- Buttons (explicit overrides; each falls back to the status token): `button`, `button_hover`,
  `button_active`, `button_foreground`, `button_primary`, `button_primary_hover`,
  `button_primary_active`, `button_primary_foreground`, `button_secondary*`, `button_danger*`,
  `button_warning*`, `button_success*`, `button_info*`.
- Sidebar: `sidebar`, `sidebar_foreground`, `sidebar_accent`, `sidebar_accent_foreground`,
  `sidebar_border`, `sidebar_primary`, `sidebar_primary_foreground`.
- List / Table / Tabs: `list`, `list_hover`, `list_active`, `list_active_border`, `list_even`,
  `list_head`, `table`, `table_hover`, `table_active`, `table_active_border`, `table_even`,
  `table_head`, `table_head_foreground`, `table_foot`, `table_foot_foreground`, `table_row_border`,
  `tab`, `tab_active`, `tab_active_foreground`, `tab_foreground`, `tab_bar`, `tab_bar_segmented`.
- Chrome: `title_bar`, `title_bar_border`, `status_bar`, `status_bar_border`.
- Misc: `link`, `link_hover`, `link_active`, `scrollbar`, `scrollbar_thumb`,
  `scrollbar_thumb_hover`, `slider_bar`, `slider_thumb`, `switch`, `switch_thumb`, `progress_bar`,
  `drag_border`, `drop_target`, `accordion`, `description_list_label`,
  `description_list_label_foreground`, `chart_1`..`chart_5`, `chart_bullish`, `chart_bearish`,
  `red`/`red_light`/`green`/`green_light`/`blue`/`blue_light`/`yellow`/….

Hover/active variants confirmed present: `primary_hover`/`primary_active`, `danger_hover`/`danger_active`,
`warning_hover`/`warning_active`, `success_hover`/`success_active`, `info_hover`/`info_active`,
`secondary_hover`/`secondary_active`, `link_hover`/`link_active`, `list_hover`, `table_hover`,
`button_*_hover`/`button_*_active`.

### 6.3 Semantic tokens (gpui-base, newer surface)

```rust
// BASE/src/theme_tokens.rs:11-17
pub struct SemanticThemeTokens {
    pub colors: ColorTokens,
    pub radius: RadiusTokens,
    pub spacing: SpacingTokens,
    pub typography: TypographyTokens,
    pub shadow: ShadowTokens,
}

// BASE/src/theme_tokens.rs:19-39
pub struct ColorTokens {
    pub background, foreground, surface, surface_foreground,
    pub primary, primary_foreground, secondary, secondary_foreground,
    pub muted, muted_foreground, accent, accent_foreground,
    pub destructive, destructive_foreground, border, input, ring, selection: Hsla,
}
```

Access: `cx.theme().semantic_tokens()` (`CMP/src/theme/mod.rs:399`), `radius_tokens()` :471,
`spacing_tokens()` :482, `typography_tokens()` :486, `shadow_tokens()` :495.

```rust
// BASE/src/theme_tokens.rs:109-116
pub struct RadiusTokens { pub none, sm, md, lg, xl, full: Pixels }
// defaults (BASE/src/theme_tokens.rs:120-127): none 0, sm 3, md 6, lg 8, xl 12, full 9999

// BASE/src/theme_tokens.rs:132-140
pub struct SpacingTokens { pub xxs, xs, sm, md, lg, xl, xxl: Pixels }
// defaults (:144-153): 2, 4, 8, 12, 16, 24, 32

// BASE/src/theme_tokens.rs:164-173
pub struct TypographyTokens { pub sans, mono: SharedString, pub xs, sm, md, lg, xl, mono_md: TextStyleToken }
// TextStyleToken { size: Pixels, line_height: Pixels, weight: FontWeight }  (:157-161)
```

### 6.4 Radius accessors — exact answer

On `Theme` (`CMP/src/theme/mod.rs`), the **only** radius members are:

```rust
pub radius: Pixels                                // field, :142
pub radius_lg: Pixels                             // field, :144
pub fn radius_full(&self) -> Pixels               // :445  (px(0.) when theme.radius is zero, else px(9999.))
pub fn radius_2xl(&self) -> Pixels                // :457  = radius * 2.5
pub fn radius_3xl(&self) -> Pixels                // :462  = radius * 3.
pub fn radius_4xl(&self) -> Pixels                // :467  = radius * 3.5
pub fn radius_tokens(&self) -> RadiusTokens       // :471  -> { none: 0, sm: radius/2, md: radius, lg: radius_lg, xl: radius*2, full }
```

**There is no `Theme::radius_sm`, no `Theme::radius_md`, no `Theme::radius_xl`.** Grep of
`CMP/src/**` for `radius_sm`/`radius_md` returns nothing. Read the small/medium radii via
`cx.theme().radius_tokens().sm` / `.md` / `.lg` / `.xl` / `.full`
(`RadiusTokens` at `BASE/src/theme_tokens.rs:109-116`), and the large-surface radius via the
`radius_lg` field.

### 6.5 Font size

`cx.theme().font_size` is a `Pixels` field (`CMP/src/theme/mod.rs:130`), default `px(16.)`.
`cx.theme().mono_font_size` is `px(13.)`. Both are settable directly or through
`ThemeConfig { font_size: Option<f32>, .. }` applied by `Theme::apply_config`
(`CMP/src/theme/schema.rs:1058-1090`).

### 6.6 Mutating the theme

**There is no `Theme::update(cx, |theme| …)` method** in `gpui-component` 0.6.6 (grep of
`CMP/src/**` and `KIT/**` for `Theme::update` returns nothing). The available mutation APIs are:

```rust
// CMP/src/theme/mod.rs:198
pub fn global(cx: &App) -> &Theme
// CMP/src/theme/mod.rs:208
pub fn global_mut(cx: &mut App) -> &mut Theme
// CMP/src/theme/mod.rs:228
pub fn sync_system_appearance(window: Option<&mut Window>, cx: &mut App)
// CMP/src/theme/mod.rs:240
pub fn sync_scrollbar_appearance(cx: &mut App)
// CMP/src/theme/mod.rs:250
pub fn set_scrollbar_mode(mode: ScrollbarMode, cx: &mut App)
// CMP/src/theme/mod.rs:261
pub fn change(mode: impl Into<ThemeMode>, window: Option<&mut Window>, cx: &mut App)
// CMP/src/theme/mod.rs:367
pub fn sync_base(cx: &mut App)
// CMP/src/theme/mod.rs:506
pub fn apply_semantic_tokens(&mut self, tokens: &SemanticThemeTokens)
// CMP/src/theme/mod.rs:547
pub fn resolve_semantic_config(&self, config: &SemanticThemeConfig) -> SemanticThemeTokens
// CMP/src/theme/mod.rs:555
pub fn apply_semantic_config(&mut self, config: &SemanticThemeConfig) -> SemanticThemeTokens
// CMP/src/theme/mod.rs:562
pub fn apply_semantic_config_str(...)
```

The real mutation pattern from the crate's own test (`KIT/tests/rendering.rs:135-136`):

```rust
Theme::global_mut(cx).foreground = cx.theme().transparent;
Theme::sync_base(cx);
```

Note the doc on `global_mut`: changes to fields the Base layer mirrors (radius, colors, fonts)
reach scrollbars/resize handles **only once `Theme::sync_base` runs** (`CMP/src/theme/mod.rs:203-207`).

### 6.7 Spacing/size/text style helpers — confirmed available

These are **generated methods on `gpui::Styled`** (via `gpui_macros` in
`GPUI/src/styled.rs:9-11`, using `padding_style_methods!`, `margin_style_methods!`,
`position_style_methods!`, `overflow_style_methods!`, `border_style_methods!`,
`box_shadow_style_methods!`, `cursor_style_methods!`, and a size/spacing set). They are in scope
through `gpui::prelude::*` (`Styled`). Confirmed present and used by the crate's own tests and
examples:

- `gap_2()` — used at `GPUI/examples/hello_world.rs:35`; the family is `gap_0`…`gap_12` plus
  `gap_1p5()`-style halves.
- `p_4()`, `px_2()`, `py_1()`, `pl_3()`, `pr_*`, `pt_*`, `pb_*`, `m_*` — used at
  `KIT/tests/overlays.rs:27` (`.p_4()`), `CMP/src/title_bar.rs:342` (`.pl(TITLE_BAR_LEFT_PADDING)`),
  `CMP/src/list/list.rs:670` (`.px_1p5()` / `.px_2()`).
- `h_8()`, `w_64()`, `size_4()` — the numeric families exist, but the tests overwhelmingly use the
  explicit `px()` form: `.w(px(240.))`, `.h(px(32.))`, `.size(px(0.))` (`KIT/tests/components.rs:27`,
  `KIT/tests/window.rs:22,43`). There is **no `size_4()` in any vendored test**; prefer
  `.size(px(16.))`.
- `text_sm()`, `text_xs()`, `text_base()`, `text_size(px(..))` — `text_sm` is declared directly at
  `GPUI/src/styled.rs:552`; the small/medium/large mapping used by components is
  `Size::XSmall -> text_xs()`, `Size::Small | Medium -> text_sm()`, `Size::Large -> text_base()`
  (`CMP/src/sizing.rs:226-232`).
- `flex_1()`, `flex()`, `flex_col()`, `flex_row()`, `items_center()`, `justify_between()`,
  `size_full()`, `w_full()`, `h_full()`, `relative()`, `absolute()`, `border_b_1()`, `rounded_*()`.
- `FluentBuilder` comes from `gpui::prelude::FluentBuilder as _` and provides `.when(cond, f)` /
  `.when_some(opt, f)` / `.map(f)` — imported explicitly in
  `CMP/src/root.rs:13`, used at `KIT/tests/window.rs:28-39`.

---

## 7. Lists and virtualization

### 7.1 `ListDelegate` — exact trait

`CMP/src/list/delegate.rs:10-171`.

```rust
pub trait ListDelegate: Sized + 'static {
    type Item: Selectable + IntoElement;                                     // :12  associated type

    // PROVIDED (default Task::ready(()))
    fn perform_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Task<()> { Task::ready(()) }                                        // :16-21

    // PROVIDED (default 1, min 1)
    fn sections_count(&self, cx: &App) -> usize { 1 }                        // :27-29

    // REQUIRED
    fn items_count(&self, section: usize, cx: &App) -> usize;                // :35

    // REQUIRED
    fn render_item(
        &mut self,
        ix: IndexPath,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item>;                                                 // :41-46

    // PROVIDED
    fn render_section_header(&mut self, section: usize, window: &mut Window,
        cx: &mut Context<ListState<Self>>) -> Option<impl IntoElement> { None::<AnyElement> }   // :51-56
    fn render_section_footer(&mut self, section: usize, window: &mut Window,
        cx: &mut Context<ListState<Self>>) -> Option<impl IntoElement> { None::<AnyElement> }   // :60-65
    fn render_empty(&mut self, window: &mut Window,
        cx: &mut Context<ListState<Self>>) -> impl IntoElement                                   // :70-79
    fn render_initial(&mut self, window: &mut Window,
        cx: &mut Context<ListState<Self>>) -> Option<AnyElement> { None }                        // :89-93
    fn loading(&self, cx: &App) -> bool { false }                                                // :96-98
    fn render_loading(&mut self, window: &mut Window,
        cx: &mut Context<ListState<Self>>) -> impl IntoElement { Loading }                       // :101-106

    // REQUIRED
    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    );                                                                       // :109-114

    // PROVIDED
    fn set_right_clicked_index(&mut self, ix: Option<IndexPath>, window: &mut Window,
        cx: &mut Context<ListState<Self>>) {}                                                // :117-122

    // PROVIDED — user clicked an item or pressed Enter
    fn confirm(&mut self, secondary: bool, window: &mut Window,
        cx: &mut Context<ListState<Self>>) {}                                                // :128-129

    // PROVIDED — ESC
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {}          // :132

    // PROVIDED
    fn has_more(&self, cx: &App) -> bool { false }                                           // :137-139
    fn load_more_threshold(&self) -> usize { 20 }                                            // :147-149
    fn load_more(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {}       // :161
}
```

So the **three required methods** are `items_count`, `render_item`, `set_selected_index`;
`perform_search` and `confirm` are provided and optional. `Item` must be
`Selectable + IntoElement` (usually `ListItem`).

### 7.2 `ListState`

`CMP/src/list/list.rs`.

```rust
pub struct ListState<D: ListDelegate> { … }                                  // :70

pub fn new(delegate: D, window: &mut Window, cx: &mut Context<Self>) -> Self  // :94
pub fn searchable(mut self, searchable: bool) -> Self                         // :125
pub fn set_searchable(&mut self, searchable: bool, cx: &mut Context<Self>)    // :130
pub fn selectable(mut self, selectable: bool) -> Self                         // :136
pub fn set_selectable(&mut self, selectable: bool, cx: &mut Context<Self>)    // :142
pub fn delegate(&self) -> &D                                                  // :147
pub fn delegate_mut(&mut self) -> &mut D                                      // :151
pub fn focus(&mut self, window: &mut Window, cx: &mut App)                    // :156
pub fn set_selected_index(&mut self, ix: Option<IndexPath>, window: &mut Window,
    cx: &mut Context<Self>)                                                    // :184
pub fn selected_index(&self) -> Option<IndexPath>                             // :194
pub fn set_right_clicked_index(...)                                           // :199
pub fn right_clicked_index(&self) -> Option<IndexPath>                        // :210
pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) // :215
pub fn set_item_to_measure_index(...)                                         // :228
pub fn scroll_to_item(&mut self, ix: IndexPath, strategy: ScrollStrategy,
    _: &mut Window, cx: &mut Context<Self>)                                    // :239-244
pub fn scroll_handle(&self) -> &VirtualListScrollHandle                       // :259
pub fn scroll_to_selected_item(&mut self, _: &mut Window, cx: &mut Context<Self>) // :263
```

`impl<D> Focusable for ListState<D>` (`:608-612`), `impl<D> EventEmitter<ListEvent>` (`:620`),
`impl<D> Render for ListState<D>` (`:621`).

### 7.3 `List` element

```rust
#[derive(IntoElement)]
pub struct List<D: ListDelegate + 'static> { … }                              // :721
pub fn new(state: &Entity<ListState<D>>) -> Self                              // :732
pub fn scrollbar_visible(mut self, visible: bool) -> Self                     // :741
pub fn search_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self // :747
impl<D> Styled for List<D>                                                    // :757  (.flex_1(), .size(...), .p_4()…)
impl<D> Sizable for List<D>                                                   // :766  (.small(), .with_size(..))
```

### 7.4 Does a `List` need explicit `size`/`flex_1`?

**Yes in practice.** `List::render` produces `div().id("list").role(Role::List).size_full()…`
(`CMP/src/list/list.rs:776-788`), and `ListState::render` produces `v_flex()…size_full()`
(`CMP/src/list/list.rs:660-666`). `size_full()` means `w_full().h_full()`, so the `List` fills
whatever box its parent gives it. If the parent is a plain auto-height `v_flex()`/`div()`, that box
has no definite height and the virtual list has nothing to scroll; the app must give the List a
definite box with `.flex_1()`, `.h(px(..))`, or `.size_full()`. `List::render` also lifts
`style.padding` and `style.max_size.height` out of the style into `ListOptions` before rendering
(`CMP/src/list/list.rs:778-790`), so `.p_4()` and `.max_h(..)` on the `List` element apply to the
inner virtual list, not the outer div.

### 7.5 `gpui_base` virtualization primitives (also re-exported by `gpui_component`)

```rust
// BASE/src/lib.rs:201
pub use virtual_list::{VirtualList, VirtualListScrollHandle, h_virtual_list, v_virtual_list};
// CMP/src/virtual_list.rs:2
pub use gpui_base::{VirtualList, VirtualListScrollHandle, h_virtual_list, v_virtual_list};

// BASE/src/virtual_list.rs:139-148
pub fn v_virtual_list<R, V>(
    view: Entity<V>,
    id: impl Into<ElementId>,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    f: impl 'static + Fn(&mut V, Range<usize>, &mut Window, &mut Context<V>) -> Vec<R>,
) -> VirtualList
where R: IntoElement, V: Render;

// BASE/src/virtual_list.rs:160-169
pub fn h_virtual_list<R, V>(/* same params */) -> VirtualList;

// BASE/src/virtual_list.rs:174-180
pub fn virtual_list<R, V>(view, id, axis: Axis, item_sizes, f) -> VirtualList;

// BASE/src/virtual_list.rs:89 / :102 / :107 / :123
pub fn new() -> Self
pub fn base_handle(&self) -> &ScrollHandle
pub fn scroll_to_item(&self, ix: usize, strategy: ScrollStrategy)
pub fn scroll_to_bottom(&self)

// BASE/src/virtual_list.rs:236 / :243 / :249 / :258
pub fn track_scroll(mut self, scroll_handle: &VirtualListScrollHandle) -> Self
pub fn with_sizing_behavior(mut self, behavior: ListSizingBehavior) -> Self
pub fn with_item_to_measure_index(mut self, index: usize) -> Self
pub fn with_scroll_handle(mut self, scroll_handle: &VirtualListScrollHandle) -> Self
```

---

## 8. Scroll

### 8.1 There is no `Scrollable` trait in GPUI 0.3.6

Scroll is applied via methods on GPUI's `InteractiveElement` trait (`GPUI/src/elements/div.rs:768`),
in scope through `gpui::prelude::*`:

```rust
// GPUI/src/elements/div.rs:1516
fn overflow_scroll(mut self) -> Self
// GPUI/src/elements/div.rs:1522
fn overflow_x_scroll(mut self) -> Self
// GPUI/src/elements/div.rs:1529
fn overflow_y_scroll(mut self) -> Self
// GPUI/src/elements/div.rs:1538
fn restrict_scroll_to_axis(mut self) -> Self
// GPUI/src/elements/div.rs:1543
fn track_scroll(mut self, scroll_handle: &ScrollHandle) -> Self
// GPUI/src/elements/div.rs:1549
fn anchor_scroll(mut self, scroll_anchor: Option<ScrollAnchor>) -> Self
```

`track_scroll` / `overflow_y_scroll` require the element to have an `id` (they are
`StatefulInteractiveElement`/`InteractiveElement` methods; the `ScrollHandle` is only wired when the
element is stateful — tests call `.id("…")` before scrolling, see `KIT/tests/window.rs:17-18`).

### 8.2 Scroll handle

```rust
// GPUI/src/elements/div.rs:4253
pub fn new() -> Self                    // ScrollHandle::new()
// GPUI/src/elements/div.rs:4258
pub fn offset(&self) -> Point<Pixels>
// GPUI/src/elements/div.rs:4263
pub fn max_offset(&self) -> Point<Pixels>
// GPUI/src/elements/div.rs:4268
pub fn top_item(&self) -> usize
// GPUI/src/elements/div.rs:4287
pub fn bottom_item(&self) -> usize
// GPUI/src/elements/div.rs:4394
pub fn set_offset(&self, mut position: Point<Pixels>)
```

Canonical scrollable region:

```rust
let handle = ScrollHandle::new();
div()
    .id("scroller")
    .overflow_y_scroll()
    .track_scroll(&handle)
    .child(long_content)
```

### 8.3 Scrolling to an item

- Virtual lists: `VirtualListScrollHandle::scroll_to_item(&self, ix: usize, strategy: ScrollStrategy)`
  (`BASE/src/virtual_list.rs:107`) and `scroll_to_bottom()` (`:123`). Attach with
  `VirtualList::track_scroll(&handle)` (`BASE/src/virtual_list.rs:236`).
- `ListState`: `scroll_to_item(&mut self, ix: IndexPath, strategy: ScrollStrategy, _: &mut Window, cx: &mut Context<Self>)`
  (`CMP/src/list/list.rs:239`); item index translation is internal (`rows_cache.position_of(&ix)`).
- Component extras: `ScrollableElement` (`CMP/src/scroll/scrollable.rs:16`) adds
  `scrollbar(&H, impl Into<ScrollbarAxis>)` :21, `vertical_scrollbar(&H)` :33,
  `horizontal_scrollbar(&H)` :38, `overflow_scrollbar() -> Scrollable<Self>` :45,
  `overflow_x_scrollbar()` :51, `overflow_y_scrollbar()` :57, all with
  `H: ScrollbarHandle + Clone`. `Scrollable<E>` has `pub fn id(mut self, id: impl Into<ElementId>) -> Self`
  (`CMP/src/scroll/scrollable.rs:90`).
- Scrollbar settings live on the theme: `Theme::set_scrollbar_mode(mode, cx)`
  (`CMP/src/theme/mod.rs:250`) with `ScrollbarMode::{Scrolling, Always, Hover}`.

---

## 9. Custom elements

### 9.1 `Element` trait — full parameter lists for this version

`GPUI/src/element.rs:53-106`.

```rust
pub trait Element: 'static + IntoElement {
    type RequestLayoutState: 'static;                        // :56
    type PrepaintState: 'static;                             // :60

    fn id(&self) -> Option<ElementId>;                                       // :67
    fn source_location(&self) -> Option<&'static panic::Location<'static>>;   // :71

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState);                               // :75-81

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState;                                                // :85-93

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    );                                                                       // :97-106

    // provided a11y hooks
    fn a11y_role(&self) -> Option<accesskit::Role> { None }                   // :114
    fn write_a11y_info(&self, _node: &mut accesskit::Node) {}                 // :122
    fn a11y_synthetic_children(...)                                           // :133
}
```

Confirmed by a real implementor: `impl Element for VirtualList`
(`BASE/src/virtual_list.rs:422-…`) uses exactly
`type RequestLayoutState = VirtualListFrameState; type PrepaintState = Option<Hitbox>;`
and the five signatures above.

### 9.2 Reading bounds during a frame

- `request_layout` receives no bounds. Get them in `prepaint`/`paint` from the **`bounds:
  Bounds<Pixels>` parameter** (the third argument, after the two id options).
- `window.layout_bounds` does **not** exist on `Window`; the layout-bounds accessor on a styled
  element is `Styled::layout_bounds(&self, window: &Window) -> Bounds<Pixels>` (used by kit tests'
  test-support accessors). Inside a custom `Element` you already have `bounds` directly.
- `Bounds<T>` (`GPUI/src/geometry.rs`) provides `origin()`, `size()`, `center()` :470, `left()` :1314,
  `right()`, `top()`, `bottom()`, `contains(&Point<T>)` :1468. Construct with
  `Bounds::new(origin, size)`, `bounds.centered_at(center, size)` :882.
- `Size::new(w, h)`, `size(px(..), px(..))`; `point(px(..), px(..))` is `pub const fn point<T>(x: T, y: T) -> Point<T>`
  (`GPUI/src/geometry.rs:111`).

### 9.3 Painting

```rust
// GPUI/src/window.rs:4510
pub fn paint_quad(&mut self, quad: PaintQuad)
// GPUI/src/window.rs:4581
pub fn paint_path(&mut self, mut path: Path<Pixels>, color: impl Into<Background>)
// GPUI/src/window.rs:4359
pub fn paint_layer<R>(&mut self, bounds: Bounds<Pixels>, f: impl FnOnce(&mut Self) -> R) -> R
// GPUI/src/window.rs:4647-4654
pub fn paint_glyph(
    &mut self,
    origin: Point<Pixels>,
    font_id: FontId,
    glyph_id: GlyphId,
    font_size: Pixels,
    color: Hsla,
) -> Result<()>
// GPUI/src/window.rs:2363
pub fn text_style(&self) -> TextStyle
```

```rust
// GPUI/src/window.rs:7528-7541
pub struct PaintQuad {
    pub bounds: Bounds<Pixels>,
    pub corner_radii: Corners<Pixels>,
    pub background: Background,
    pub border_widths: Edges<Pixels>,
    pub border_color: Hsla,
    pub border_style: BorderStyle,
}

// GPUI/src/window.rs:7597
pub fn fill(bounds: impl Into<Bounds<Pixels>>, background: impl Into<Background>) -> PaintQuad
// GPUI/src/window.rs (immediately after)  pub fn outline(bounds, border_color, border_width) -> PaintQuad
// GPUI/src/window.rs:7545 / :7553
impl PaintQuad { pub fn corner_radii(self, corner_radii: impl Into<Corners<Pixels>>) -> Self;
                 pub fn border_widths(self, border_widths: impl Into<Edges<Pixels>>) -> Self; }
```

`shape_line` is **on `TextSystem`, not `Window`**:

```rust
// GPUI/src/text_system.rs:638-644
pub fn shape_line(
    &self,
    text: SharedString,
    font_size: Pixels,
    runs: &[TextRun],
    force_width: Option<Pixels>,
) -> ShapedLine
// GPUI/src/text_system.rs:689
pub fn shape_line_by_hash(text_hash: u64, text_len: usize, font_size: Pixels,
    runs: &[TextRun], force_width: Option<Pixels>,
    materialize_text: impl FnOnce() -> SharedString) -> ShapedLine
```

Access via `window.text_system().shape_line(..)`. `shape_line` panics on newlines
(`GPUI/src/text_system.rs:646-649`).

### 9.4 Color / geometry types

```rust
// GPUI/src/color.rs:39-47
pub struct Rgba { pub r: f32, pub g: f32, pub b: f32, pub a: f32 }
// GPUI/src/color.rs:334-343
pub struct Hsla { pub h: f32, pub s: f32, pub l: f32, pub a: f32 }   // h,s,l,a all 0..1
// GPUI/src/geometry.rs:2270-2279
pub struct Corners<T: Clone + Debug + Default + PartialEq> {
    pub top_left: T, pub top_right: T, pub bottom_right: T, pub bottom_left: T,
}
// GPUI/src/geometry.rs:3748
pub const fn px(pixels: f32) -> Pixels
```

Theme colors are `Hsla`. `Background` (the `PaintQuad.background` type) has `From<Hsla>` and
`From<Rgba>`; `fill(bounds, cx.theme().border)` works.

### 9.5 `RenderOnce`, `IntoElement`, `ElementId`

- `RenderOnce` produces one element per call; implement
  `fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement` (see
  `CMP/src/dialog/footer.rs:59`, `CMP/src/title_bar.rs:319`, `ASSETS/src/icon.rs:24`).
- `IntoElement` requires `type Element: Element; fn into_element(self) -> Self::Element;` and is
  usually derived with `#[derive(IntoElement)]` (as on `Button`, `List`, `TitleBar`, `DialogFooter`).
- `ElementId` accepts `Into<ElementId>` from `&str`, `String`, `usize`, `SharedString`, and
  `IndexPath`; `ElementId::Name(SharedString)` and `ElementId::CodeLocation(Location)` are the two
  variants used by the component code (`CMP/src/menu/context_menu.rs:33-39`).

---

## 10. Stateful inputs

### 10.1 `InputState`

```rust
// BASE/src/input/input/mod.rs:10
pub type InputState = InputBaseState<InputMode>;

// BASE/src/input/base/state.rs:8958  (also TextareaState :9163, EditorState :9230)
pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self

// BASE/src/input/base/state.rs:766
pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self
// BASE/src/input/base/state.rs:850
pub fn set_placeholder(&mut self, placeholder: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>)
// BASE/src/input/base/state.rs:897-901
pub fn set_value(
    &mut self,
    value: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut Context<Self>,
)
// BASE/src/input/base/state.rs:1181
pub fn default_value(mut self, value: impl Into<SharedString>) -> Self
// BASE/src/input/base/state.rs:1197
pub fn value(&self) -> SharedString
// CMP/src/input/state.rs:274
pub fn value(&self, cx: &App) -> SharedString          // on the component re-export
// BASE/src/input/base/state.rs:1100
pub fn submit_on_enter(mut self, submit: bool) -> Self
// BASE/src/input/base/state.rs:1086
pub fn clean_on_escape(mut self) -> Self
// BASE/src/input/base/state.rs:1255
pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>)
```

**There is no `InputState::on_change` and no `InputState::on_submit`.** Change/enter are observed
via `cx.subscribe` on the state entity and the `InputEvent` enum:

```rust
// BASE/src/input/base/state.rs:122-127
pub enum InputEvent {
    Change,
    PressEnter { secondary: bool, shift: bool },
    Focus,
    Blur,
}
```

Real usage (`CMP/src/setting/fields/string.rs:59-69`):

```rust
let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
let _subscription = cx.subscribe(&input, {
    move |_, input, event: &InputEvent, cx| match event {
        InputEvent::Change => {
            let value = input.read(cx).value();
            set_value(value.into(), cx);
        }
        _ => {}
    }
});
```

Critical trap documented in the source: **`set_value` does not emit `InputEvent::Change`**
(`CMP/src/input/search.rs:144`, `CMP/src/list/list.rs:221`), which is why the settings field
compares `input.read(cx).value() != value` before calling it.

### 10.2 `Input` element

`CMP/src/input/input.rs`.

```rust
pub fn new(state: &Entity<InputState>) -> Self                                   // :180
pub fn id(mut self, id: impl Into<ElementId>) -> Self                            // :174
pub fn accessibility_id(mut self, id: impl Into<SharedString>) -> Self           // :230
pub fn aria_label(mut self, label: impl Into<SharedString>) -> Self              // :235
pub fn prefix(mut self, prefix: impl IntoElement) -> Self                        // :240
pub fn suffix(mut self, suffix: impl IntoElement) -> Self                        // :245
pub fn h_full(mut self) -> Self                                                  // :251
pub fn h(mut self, height: impl Into<DefiniteLength>) -> Self                    // :257
pub fn appearance(mut self, appearance: bool) -> Self                            // :263
pub fn bordered(mut self, bordered: bool) -> Self                                // :269
pub fn focus_bordered(mut self, bordered: bool) -> Self                          // :275
pub fn cleanable(mut self, cleanable: bool) -> Self                              // :281
pub fn mask_toggle(mut self) -> Self                                             // :287
pub fn content_type(mut self, content_type: InputContentType) -> Self            // :296
pub fn role(mut self, role: impl Into<RoleOverride>) -> Self                     // :304
pub fn disabled(mut self, disabled: bool) -> Self                                // :310
pub fn readonly(mut self, readonly: bool) -> Self                                // :320
pub fn tab_index(mut self, index: isize) -> Self                                 // :326
pub fn context_menu(...)                                                         // :334
pub fn on_paste(...)                                                             // :355
```

Search-box shape from the crate's own tests (`KIT/tests/components.rs:27`,
`KIT/tests/overlays.rs:25`, `KIT/tests/rendering.rs:104`):

```rust
// state must be created with a window
let search: Entity<InputState> = cx.new(|cx| InputState::new(window, cx));
// placeholder is builder-only on the state, set at construction:
cx.new(|cx| InputState::new(window, cx).placeholder("Search…"))
// render
Input::new(&self.search).id("search").w(px(240.))
```

Test interactions prove the ided element is what tests target: `window.click("search", cx)` then
`window.input("GPUI 中文 🦀", cx)` and `window.find("search").value()`
(`KIT/tests/components.rs:53-57`).

### 10.3 `Checkbox` / `Switch`

```rust
// CMP/src/checkbox.rs
pub struct Checkbox                                            // :17
pub fn new(id: impl Into<ElementId>) -> Self                    // :39
pub fn role(mut self, role: impl Into<RoleOverride>) -> Self    // :60
pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self // :66
pub fn label(mut self, label: impl Into<Text>) -> Self          // :72
pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self // :80
pub fn checked(mut self, checked: bool) -> Self                 // :86
pub fn on_click(self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self  // :92
pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self // :102
pub fn tab_stop(mut self, tab_stop: bool) -> Self               // :108
pub fn tab_index(mut self, tab_index: isize) -> Self            // :114

// CMP/src/switch.rs
pub struct Switch                                               // :14
pub fn new(id: impl Into<ElementId>) -> Self                    // :31
pub fn checked(mut self, checked: bool) -> Self                 // :49
pub fn label(mut self, label: impl Into<Text>) -> Self          // :55
pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self // :66
pub fn on_click<F>(self, handler: F) -> Self where F: Fn(&bool, &mut Window, &mut App) + 'static  // :72-76
pub fn on_change<F>(mut self, handler: F) -> Self where F: Fn(&bool, &mut Window, &mut App) + 'static // :85-89
pub fn color(mut self, color: impl Into<Hsla>) -> Self          // :95
pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self // :101
```

Both are **controlled**: the handler receives the *requested* value; the owner must write it and
call `cx.notify()` (`CMP/src/switch.rs:78-83`). `on_click` and `on_change` share one callback
slot — chaining replaces instead of adding (`CMP/src/switch.rs:91-94`).

`Radio` (`CMP/src/radio.rs`): `new` :41, `tooltip` :64, `label` :70, `accessibility_label` :80,
`checked` :86, `disabled` :92, `tab_index` :98, `tab_stop` :104, `on_click(impl Fn(&bool, ..))` :110,
`on_change(impl Fn(&bool, ..))` :120. `RadioGroup`: `new` :292, `vertical` :305, `horizontal` :310,
`on_click(impl Fn(&usize, ..))` :321, `on_change(impl Fn(&usize, ..))` :331,
`selected_index(impl Into<Option<usize>>)` :337.

---

## 11. Menus

### 11.1 `PopupMenu`

`CMP/src/menu/popup_menu.rs`.

```rust
pub struct PopupMenu { /* private */ }                                       // :282

// Build an Entity from a builder fn (the only public constructor)
pub fn build(
    window: &mut Window,
    cx: &mut App,
    f: impl FnOnce(Self, &mut Window, &mut Context<PopupMenu>) -> Self,
) -> Entity<Self>                                                            // :358-362

pub fn action_context(mut self, handle: FocusHandle) -> Self                 // :371
pub fn min_w(mut self, width: impl Into<Pixels>) -> Self                     // :429
pub fn max_w(mut self, width: impl Into<Pixels>) -> Self                     // :435
pub fn max_h(mut self, height: impl Into<Pixels>) -> Self                    // :441
pub fn scrollable(mut self, scrollable: bool) -> Self                        // :447
pub fn check_side(mut self, side: Side) -> Self                              // :453
pub fn external_link_icon(mut self, visible: bool) -> Self                   // :459

// Add items
pub fn menu(self, label: impl Into<SharedString>, action: Box<dyn Action>) -> Self           // :465
pub fn menu_with_enable(self, label, action: Box<dyn Action>, enable: bool) -> Self          // :470
pub fn menu_with_disabled(self, label, action: Box<dyn Action>, disabled: bool) -> Self      // :481
pub fn label(mut self, label: impl Into<SharedString>) -> Self                               // :492
pub fn link(self, label: impl Into<SharedString>, href: impl Into<String>) -> Self           // :498
pub fn link_with_disabled(...)                                                               // :503
pub fn link_with_icon(...)                                                                   // :516
pub fn menu_with_icon(...)                                                                   // :543
pub fn menu_with_icon_and_disabled(...)                                                      // :553
pub fn menu_with_check(...)                                                                  // :565
pub fn menu_with_check_and_disabled(...)                                                     // :575
pub fn menu_element<F, E>(self, action: Box<dyn Action>, builder: F) -> Self                 // :587
pub fn menu_element_with_disabled / _with_icon / _with_check(...)                            // :596 / :610 / :624
pub fn separator(mut self) -> Self                                                           // :680
pub fn submenu(self, label: impl Into<SharedString>, window: &mut Window,
    cx: &mut Context<Self>,
    f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static) -> Self  // :694-699
pub fn submenu_with_icon(mut self, icon: Option<Icon>, label, window, cx, f) -> Self         // :705
pub fn item(mut self, item: impl Into<PopupMenuItem>) -> Self                                // :728
pub fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>,
    f: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self)                            // :753
pub fn is_empty(&self) -> bool                                                               // :841
```

```rust
// CMP/src/menu/popup_menu.rs:33
pub enum PopupMenuItem { … }
pub fn new(label: impl Into<SharedString>) -> Self                     // :71
pub fn element<F, E>(builder: F) -> Self                               // :85
pub fn submenu(label: impl Into<SharedString>, menu: Entity<PopupMenu>) -> Self  // :102
pub fn separator() -> Self                                             // :113
pub fn label(label: impl Into<SharedString>) -> Self                   // :119
pub fn icon(mut self, icon: impl Into<Icon>) -> Self                   // :126
pub fn action(mut self, action: Box<dyn Action>) -> Self               // :145
pub fn disabled(mut self, disabled: bool) -> Self                      // :161
pub fn checked(mut self, checked: bool) -> Self                        // :180
pub fn on_click<F>(mut self, handler: F) -> Self                       // :196   (adds an on_click "action" rather than a GPUI Action)
pub fn link(label: impl Into<SharedString>, href: impl Into<String>) -> Self  // :214
```

### 11.2 Anchoring / showing

There is **no `window.open_context_menu`** in `gpui-pre-0.3.6` (grep of `GPUI/src/window.rs` for
`open_context_menu`/`show_context_menu` returns nothing). Menus are attached declaratively:

**Anchored to an element (dropdown):**

```rust
// CMP/src/menu/dropdown_menu.rs:12-32
pub trait DropdownMenu: Styled + Selectable + InteractiveElement + IntoElement + 'static {
    fn dropdown_menu(
        self,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> DropdownMenuPopover<Self> {
        self.dropdown_menu_with_anchor(Anchor::TopLeft, f)
    }

    fn dropdown_menu_with_anchor(
        mut self,
        anchor: impl Into<Anchor>,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> DropdownMenuPopover<Self>
}
impl DropdownMenu for Button {}                                   // :34
```

`DropdownMenuPopover` extras: `pub fn anchor(mut self, anchor: impl Into<Anchor>) -> Self` :67,
`pub fn on_open_change(...)` :81.

Real call (`KIT/tests/menu.rs:26-34`):

```rust
Button::new("commands").label("Commands").dropdown_menu(|menu, window, cx| {
    menu.menu_with_disabled("Unavailable", Box::new(Unavailable), true)
        .menu("Save", Box::new(Save))
        .submenu("More", window, cx, |menu, _, _| {
            menu.menu("Save copy", Box::new(Save))
        })
})
```

**Right-click context menu on an element:**

```rust
// CMP/src/menu/context_menu.rs:13-34
pub trait ContextMenuExt: InteractiveElement + ParentElement + Styled {
    fn context_menu(
        mut self,
        f: impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static,
    ) -> ContextMenu<Self>
    where Self: Sized;
}
impl<E: InteractiveElement + ParentElement + Styled> ContextMenuExt for E {}   // :41

pub struct ContextMenu<E: ParentElement + Styled + Sized> { … }               // :42
pub fn new(id: impl Into<ElementId>, element: E) -> Self                       // :53
```

Call shape: `div().id("row").context_menu(|menu, _, _| menu.menu("Delete", Box::new(Delete)))`.
Note the context menu sets the wrapper to `relative` and inserts an `absolute` child, so it does not
affect the parent's layout (`CMP/src/menu/context_menu.rs:17-20`).

**Native menus:** `window.show_window_menu(ev.position)` exists on `Window` (used by `TitleBar`,
`CMP/src/title_bar.rs:383`). `CMP/src/menu/app_menu_bar.rs` adds `AppMenuBar::new(cx) -> Entity<Self>` :34
and `reload(&mut self, cx)` :47. `CMP/src/native_menu/*` provides the OS menu surface.

---

## 12. Async / background

### 12.1 Spawn signatures

```rust
// GPUI/src/app.rs:2036-2039     cx.spawn inside Application::run (cx: &mut App)
pub fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R>
where
    AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static,
    R: 'static,

// GPUI/src/app.rs:2050-2053
pub fn spawn_with_priority<AsyncFn, R>(&self, priority: Priority, f: AsyncFn) -> Task<R>
where
    AsyncFn: AsyncFnOnce(&mut AsyncApp) -> R + 'static,

// GPUI/src/app/context.rs:230-234     cx.spawn on Context<T>  (this is `async move |this, cx|`)
pub fn spawn<AsyncFn, R>(&self, f: AsyncFn) -> Task<R>
where
    T: 'static,
    AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncApp) -> R + 'static,
    R: 'static,

// GPUI/src/app/context.rs:672-675    cx.spawn_in(window, ..)
pub fn spawn_in<AsyncFn, R>(&self, window: &Window, f: AsyncFn) -> Task<R>
where
    R: 'static,
    AsyncFn: AsyncFnOnce(WeakEntity<T>, &mut AsyncWindowContext) -> R + 'static,

// GPUI/src/window.rs:2653-2656       window.spawn(cx, ..)
pub fn spawn<AsyncFn, R>(&self, cx: &App, f: AsyncFn) -> Task<R>
where
    R: 'static,
    AsyncFn: AsyncFnOnce(&mut AsyncWindowContext) -> R + 'static,
```

So the answer to "does it take `async move |cx|` or `async move |this, cx|`" is **it depends on the
receiver**:

| receiver | closure shape |
| --- | --- |
| `App` (inside `application().run(\|cx\| …)`) | `async move \|cx\|` where `cx: &mut AsyncApp` |
| `AsyncApp` | `async move \|cx\|` where `cx: &mut AsyncApp` |
| `Context<T>` / `cx` inside `Context` callbacks | `async move \|this, cx\|` where `this: WeakEntity<T>`, `cx: &mut AsyncApp` |
| `Context<T>::spawn_in(window, ..)` | `async move \|this, cx\|` where `cx: &mut AsyncWindowContext` |
| `Window::spawn(cx, ..)` | `async move \|cx\|` where `cx: &mut AsyncWindowContext` |

### 12.2 `background_spawn` / `Task` — exact answer

**`cx.background_spawn` does not exist** in this version. There is no method of that name on `App`,
`AsyncApp`, `Context<T>`, `AsyncWindowContext`, or `Window` (grep of `GPUI/src/**` returns nothing
for `background_spawn`). The background executor is reached explicitly:

```rust
// GPUI/src/app.rs:2012-2015
pub fn background_executor(&self) -> &BackgroundExecutor
// GPUI/src/app/async_context.rs:159-161
pub fn background_executor(&self) -> &BackgroundExecutor
// GPUI/src/executor.rs:103
pub fn spawn<R>(&self, future: impl Future<Output = R> + Send + 'static) -> Task<R>
// GPUI/src/executor.rs:345   (ForegroundExecutor)
pub fn spawn<R>(&self, future: impl Future<Output = R> + 'static) -> Task<R>
```

`Task<R>` has `detach()` (used everywhere, e.g. `KIT/tests/interactions.rs:216`);
`TaskExt<T, E>` (`GPUI/src/executor.rs:37-46`) adds `.detach_and_log_err(cx)` for
`Task<Result<T, E>>`.

### 12.3 A real example, exactly as the crate writes it

`KIT/tests/interactions.rs:198-218`:

```rust
#[gpui_kit::test]
async fn wait_for_drives_test_time_and_refreshes_async_changes(cx: &mut TestAppContext) {
    let handle = cx.add_window(|_, _| Loading { ready: false });
    let executor = cx.executor();
    cx.spawn(move |mut cx| async move {
        executor.timer(Duration::from_millis(25)).await;
        handle
            .update(&mut cx, |view, _, cx| {
                view.ready = true;
                cx.notify();
            })
            .unwrap();
    })
    .detach();
    // ...
}
```

The pattern for "model receives a background result and notifies" is therefore:
`cx.background_executor().spawn(async move { … }).await` inside a `cx.spawn(...)` future, or a
`handle.update(&mut cx, |model, _, cx| { model.field = value; cx.notify(); })` hop back to the
foreground. `Entity::update` and `WeakEntity::update` run on the app thread and are the only safe
way to mutate a model from an async task.

### 12.4 Foreground/background executors and timers

```rust
// GPUI/src/app.rs:2018-2023
pub fn foreground_executor(&self) -> &ForegroundExecutor
// GPUI/src/app/async_context.rs:164-166
pub fn foreground_executor(&self) -> &ForegroundExecutor
// GPUI/src/app.rs:1986-1991
pub fn to_async(&self) -> AsyncApp
// GPUI/src/app/async_context.rs:150
pub fn refresh(&self)   // schedules all windows to redraw
```

---

## 13. Actions / keybindings

### 13.1 `actions!`

`gpui_kit::actions!` is a macro exported by the kit crate itself (`KIT/src/lib.rs:49-80`), because
GPUI's derive is spelled `gpui::Action` and does not resolve when GPUI is consumed only through the
facade.

```rust
// KIT/src/lib.rs:50
macro_rules! actions {
    ($namespace:path, [ $( $(#[$attr:meta])* $name:ident),* $(,)? ]) => { … };
    ([ $( $(#[$attr:meta])* $name:ident),* $(,)? ]) => { … };
}
```

It expands each name to a unit struct deriving `Clone, PartialEq, Default, Debug, $crate::Action`
and, in the namespaced form, `#[action(namespace = $namespace)]`.

Real usages, verbatim:

```rust
// KIT/tests/menu.rs:6
actions!(menu_test, [Save, Unavailable]);

// KIT/tests/search.rs:9
gpui_kit::actions!(search_test, [Save]);

// GPUI re-export path also works because `gpui::actions!` exists and gpui_kit glob-re-exports it.
```

Actions are dispatched by type: `.on_action(cx.listener(|this, _: &Save, _, cx| { … }))`
(`KIT/tests/search.rs:25-28`) and referenced in menus as `Box::new(Save)` (`KIT/tests/menu.rs:28`).

### 13.2 `KeyBinding` and `cx.bind_keys`

```rust
// GPUI/src/keymap/binding.rs:33
pub fn new<A: Action>(keystrokes: &str, action: A, context: Option<&str>) -> Self   // panics on parse error

// GPUI/src/app.rs:2344
pub fn bind_keys(&mut self, bindings: impl IntoIterator<Item = KeyBinding>)
// GPUI/src/app.rs:2350
pub fn clear_key_bindings(&mut self)
// GPUI/src/app.rs:2356
pub fn key_bindings(&self) -> Rc<RefCell<Keymap>>
```

Real example with the exact string syntax (`CMP/src/root.rs:26-38`):

```rust
actions!(root, [Tab, TabPrev]);

const CONTEXT: &str = "Root";

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", Tab, Some(CONTEXT)),
        KeyBinding::new("shift-tab", TabPrev, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-c", Copy, Some(CONTEXT)),
    ]);
}
```

Binding-string syntax observed in the sources: modifiers joined with `-` before the key, in the
order `secondary`/`cmd`/`ctrl`/`alt`/`shift`, e.g. `"tab"`, `"shift-tab"`, `"cmd-c"`, `"ctrl-c"`,
`"cmd-shift-k"`, `"ctrl-backspace"`, `"cmd-delete"`, `"alt-delete"`, `"shift-enter"`,
`"secondary-enter"`, `"escape"`, `"down"`, `"pageup"` (`CMP/src/root.rs:26-38`,
`BASE/src/input/base/state.rs:132-185`). `secondary` is the platform's primary modifier and
`cmd`/`ctrl` are literal.

### 13.3 `key_context` and `on_action`

```rust
// GPUI/src/elements/div.rs:830-834
fn key_context<C, E>(mut self, key_context: C) -> Self
where
    C: TryInto<KeyContext, Error = E>,
    E: std::fmt::Display,
```

`on_action` lives on `InteractiveElement`/`StatefulInteractiveElement` and takes
`impl Fn(&Action, &mut Window, &mut App) + 'static` (the `cx.listener(|this, _: &Save, _, cx| …)`
form is the sugar used in tests). `ListState::render` installs
`.key_context("List")` plus `Cancel`/`Confirm`/select-prev/select-next actions on the list
(`CMP/src/list/list.rs:660-712`). `Root` installs key bindings under context `"Root"`
(`CMP/src/root.rs:24-38`).

---

## 14. Notifications / toasts

### 14.1 `Notification` builder

`CMP/src/notification.rs`.

```rust
pub fn new() -> Self                                                    // :167
pub fn message(mut self, message: impl Into<SharedString>) -> Self      // :190
pub fn info(message: impl Into<SharedString>) -> Self                   // :196
pub fn success(message: impl Into<SharedString>) -> Self                // :203
pub fn warning(message: impl Into<SharedString>) -> Self                // :210
pub fn error(message: impl Into<SharedString>) -> Self                  // :217
pub fn id<T: Sized + 'static>(mut self) -> Self                         // :229
pub fn id1<T: Sized + 'static>(mut self, key: impl Into<ElementId>) -> Self // :235
pub fn title(mut self, title: impl Into<SharedString>) -> Self          // :243
pub fn icon(mut self, icon: impl Into<Icon>) -> Self                    // :251
pub fn with_type(mut self, type_: NotificationType) -> Self             // :257
pub fn placement(mut self, placement: Anchor) -> Self                   // :266
pub fn delivery(mut self, delivery: NotificationDelivery) -> Self       // :290
pub fn system(self) -> Self                                             // :299
pub fn in_app_and_system(self) -> Self                                  // :309
pub fn autohide(mut self, autohide: bool) -> Self                       // :314
pub fn on_click(mut self, on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self // :320-323
pub fn on_close(mut self, on_close: impl Fn(&mut Window, &mut App) + 'static) -> Self // :332
pub fn action<F>(mut self, action: F) -> Self
    where F: Fn(&mut Self, &mut Window, &mut Context<Self>) -> Button + 'static         // :340-344
pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>)                 // :350
```

```rust
// CMP/src/notification.rs:31
pub enum NotificationType { … }          // Info / Success / Warning / Error
// CMP/src/notification.rs:53
pub enum NotificationDelivery { … }      // in-app / system
pub fn includes_in_app(&self) -> bool    // :65
pub fn includes_system(&self) -> bool    // :70
```

**There is no `variant` or `level` method.** The level is set by `with_type(NotificationType::…)`
or the `info`/`success`/`warning`/`error` constructors. Setting `action(..)` forces
`autohide = false` (`CMP/src/notification.rs:342-344`). `NotificationSettings` defaults and the
notification list live at `CMP/src/notification.rs:530` (`NotificationSettings`) and `:693`
(`NotificationList`).

### 14.2 `push_notification` usage

```rust
// CMP/src/window_ext.rs:58
fn push_notification(&mut self, note: impl Into<Notification>, cx: &mut App);
```

Real call (`KIT/tests/overlays.rs:83-87`):

```rust
window.push_notification(Notification::new().message("Updated").autohide(true), cx);
```

Also seen inside a dialog `on_ok` (`KIT/tests/overlays.rs:51-57`):

```rust
window.push_notification(
    Notification::new().message("Profile saved").autohide(false),
    cx,
);
```

Behavior verified by the kit tests: the default autohide timeout is five seconds
(`KIT/tests/overlays.rs:~300` comment "The default timeout is five seconds"), the toast mounts in a
container with id `"notification"`, and its close button has id `"close"`
(`KIT/tests/overlays.rs:139-146`). The entrance uses GPUI's wall-clock `Animation` (400 ms), which
is why the test sleeps 410 ms (`KIT/tests/overlays.rs:129-131`). Removal:
`remove_notification::<T>(cx)`, `remove_notification1::<T>(key, cx)`, `clear_notifications(cx)`,
`notifications(cx) -> Rc<Vec<Entity<Notification>>>` (`CMP/src/window_ext.rs:62-71`).

---

## 15. Full `IconName` variant list (1830 names)

Generated from `ASSETS/assets/icons/*.svg` by the algorithm in `ASSETS/build.rs:22-33`. This is the
complete exhaustive set of `gpui_kit_assets::IconName` variants; using a name not in this list will
not compile.

```text
// 1830 variants
AArrowDown, AArrowUp, ALargeSmall, Accessibility, Activity, Ad, AirVent, Airplay, AlarmClock,
AlarmClockCheck, AlarmClockMinus, AlarmClockOff, AlarmClockPlus, AlarmSmoke, Album,
AlignCenterHorizontal, AlignCenterVertical, AlignEndHorizontal, AlignEndVertical,
AlignHorizontalDistributeCenter, AlignHorizontalDistributeEnd, AlignHorizontalDistributeStart,
AlignHorizontalJustifyCenter, AlignHorizontalJustifyEnd, AlignHorizontalJustifyStart,
AlignHorizontalSpaceAround, AlignHorizontalSpaceBetween, AlignStartHorizontal,
AlignStartVertical, AlignVerticalDistributeCenter, AlignVerticalDistributeEnd,
AlignVerticalDistributeStart, AlignVerticalJustifyCenter, AlignVerticalJustifyEnd,
AlignVerticalJustifyStart, AlignVerticalSpaceAround, AlignVerticalSpaceBetween, Ambulance,
Ampersand, Ampersands, Amphora, Anchor, Angle, Antenna, Anvil, Aperture, AppWindow,
AppWindowMac, Apple, Archive, ArchiveRestore, ArchiveX, Armchair, ArrowBigDown,
ArrowBigDownDash, ArrowBigLeft, ArrowBigLeftDash, ArrowBigRight, ArrowBigRightDash, ArrowBigUp,
ArrowBigUpDash, ArrowDown, ArrowDown01, ArrowDown10, ArrowDownAZ, ArrowDownFromLine,
ArrowDownLeft, ArrowDownNarrowWide, ArrowDownRight, ArrowDownToDot, ArrowDownToLine,
ArrowDownUp, ArrowDownWideNarrow, ArrowDownZA, ArrowLeft, ArrowLeftFromLine, ArrowLeftRight,
ArrowLeftToLine, ArrowRight, ArrowRightFromLine, ArrowRightLeft, ArrowRightToLine, ArrowUp,
ArrowUp01, ArrowUp10, ArrowUpAZ, ArrowUpDown, ArrowUpFromDot, ArrowUpFromLine, ArrowUpLeft,
ArrowUpNarrowWide, ArrowUpRight, ArrowUpToLine, ArrowUpWideNarrow, ArrowUpZA, ArrowsUpFromLine,
Asterisk, Astroid, AtSign, Atom, AudioLines, AudioLinesOff, AudioLinesX, AudioWaveform, Award,
Axe, Axis3d, Baby, Backpack, Badge, BadgeAlert, BadgeCent, BadgeCheck, BadgeDollarSign,
BadgeEuro, BadgeIndianRupee, BadgeInfo, BadgeJapaneseYen, BadgeMinus, BadgePercent, BadgePlus,
BadgePoundSterling, BadgeQuestionMark, BadgeRussianRuble, BadgeSwissFranc, BadgeTurkishLira,
BadgeX, BaggageClaim, Balloon, Ban, Banana, Bandage, Banknote, BanknoteArrowDown,
BanknoteArrowUp, BanknoteCheck, BanknoteX, Barcode, Barrel, Baseline, Bath, Battery,
BatteryCharging, BatteryFull, BatteryLow, BatteryMedium, BatteryPlus, BatteryWarning, Beaker,
Bean, BeanOff, Bed, BedDouble, BedSingle, Beef, BeefOff, Beer, BeerOff, Bell, BellCheck,
BellDot, BellElectric, BellMinus, BellOff, BellPlus, BellRing, BetweenHorizontalEnd,
BetweenHorizontalStart, BetweenVerticalEnd, BetweenVerticalStart, BicepsFlexed, Bike, Binary,
Binoculars, Biohazard, Bird, Birdhouse, Bitcoin, Blend, Blender, Blinds, Blocks, Bluetooth,
BluetoothConnected, BluetoothOff, BluetoothSearching, Bold, Bolt, Bomb, Bone, BoneFracture,
Book, BookA, BookAlert, BookAudio, BookCheck, BookCopy, BookDashed, BookDown, BookHeadphones,
BookHeart, BookImage, BookKey, BookLock, BookMarked, BookMinus, BookOpen, BookOpenCheck,
BookOpenText, BookPlus, BookSearch, BookText, BookType, BookUp, BookUp2, BookUser, BookX,
Bookmark, BookmarkCheck, BookmarkMinus, BookmarkOff, BookmarkPlus, BookmarkX, BoomBox, Bot,
BotMessageSquare, BotOff, BottleWine, BowArrow, Box, Boxes, Braces, Brackets, Brain,
BrainCircuit, BrainCog, BrickWall, BrickWallFire, BrickWallShield, Bridge, Briefcase,
BriefcaseBusiness, BriefcaseConveyorBelt, BriefcaseMedical, BringToFront, Broccoli, Broom,
BroomSparkles, Brush, BrushCleaning, Bubbles, Bug, BugOff, BugPlay, Building, Building2, Bus,
BusFront, Cable, CableCar, Cake, CakeSlice, Calculator, Calendar, Calendar1, CalendarArrowDown,
CalendarArrowUp, CalendarCheck, CalendarCheck2, CalendarClock, CalendarCog, CalendarDays,
CalendarFold, CalendarHeart, CalendarMinus, CalendarMinus2, CalendarOff, CalendarPlus,
CalendarPlus2, CalendarRange, CalendarSearch, CalendarSync, CalendarX, CalendarX2, Calendars,
Camera, CameraOff, Can, CanSoda, Candy, CandyCane, CandyOff, Cannabis, CannabisOff, Captions,
CaptionsOff, Car, CarBattery, CarFront, CarTaxiFront, Caravan, CardSim, Carrot, Carton,
CartonOff, CaseLower, CaseSensitive, CaseUpper, CassetteTape, Cast, Castle, Cat, Cctv, CctvOff,
ChartArea, ChartBar, ChartBarBig, ChartBarDecreasing, ChartBarIncreasing, ChartBarStacked,
ChartCandlestick, ChartColumn, ChartColumnBig, ChartColumnDecreasing, ChartColumnIncreasing,
ChartColumnStacked, ChartGantt, ChartLine, ChartNetwork, ChartNoAxesColumn,
ChartNoAxesColumnDecreasing, ChartNoAxesColumnIncreasing, ChartNoAxesCombined, ChartNoAxesGantt,
ChartPie, ChartScatter, ChartSpline, Check, CheckCheck, CheckLine, ChefHat, Cherry, ChessBishop,
ChessKing, ChessKnight, ChessPawn, ChessQueen, ChessRook, ChevronDown, ChevronFirst,
ChevronLast, ChevronLeft, ChevronRight, ChevronUp, ChevronsDown, ChevronsDownUp, ChevronsLeft,
ChevronsLeftRight, ChevronsLeftRightEllipsis, ChevronsRight, ChevronsRightLeft, ChevronsUp,
ChevronsUpDown, Church, Cigarette, CigaretteOff, Circle, CircleAlert, CircleArrowDown,
CircleArrowLeft, CircleArrowOutDownLeft, CircleArrowOutDownRight, CircleArrowOutUpLeft,
CircleArrowOutUpRight, CircleArrowRight, CircleArrowUp, CircleCheck, CircleCheckBig,
CircleChevronDown, CircleChevronLeft, CircleChevronRight, CircleChevronUp, CircleDashed,
CircleDashedCheck, CircleDivide, CircleDollarSign, CircleDot, CircleDotDashed, CircleEllipsis,
CircleEqual, CircleEuro, CircleFadingArrowUp, CircleFadingPlus, CircleGauge, CircleMinus,
CircleOff, CircleParking, CircleParkingOff, CirclePause, CirclePercent, CirclePile, CirclePlay,
CirclePlus, CirclePoundSterling, CirclePower, CircleQuestionMark, CircleSlash, CircleSlash2,
CircleSmall, CircleStar, CircleStop, CircleUser, CircleUserRound, CircleX, CircuitBoard, Citrus,
Clapperboard, Clipboard, ClipboardCheck, ClipboardClock, ClipboardCopy, ClipboardList,
ClipboardMinus, ClipboardPaste, ClipboardPen, ClipboardPenLine, ClipboardPlus, ClipboardType,
ClipboardX, Clock, Clock1, Clock10, Clock11, Clock12, Clock2, Clock3, Clock4, Clock5, Clock6,
Clock7, Clock8, Clock9, ClockAlert, ClockArrowDown, ClockArrowLeft, ClockArrowRight,
ClockArrowUp, ClockCheck, ClockFading, ClockPlus, Close, ClosedCaption, Cloud, CloudAlert,
CloudBackup, CloudCheck, CloudCog, CloudDownload, CloudDrizzle, CloudFog, CloudHail,
CloudLightning, CloudMoon, CloudMoonRain, CloudOff, CloudRain, CloudRainWind, CloudSnow,
CloudSun, CloudSunRain, CloudSync, CloudUpload, Cloudy, Clover, Club, Code, CodeXml, Coffee,
Cog, Coins, Columns2, Columns3, Columns3Cog, Columns4, Combine, Command, Compass, Component,
Computer, ConciergeBell, Cone, Construction, Contact, ContactRound, Container, Contrast, Cookie,
CookingPot, Copy, CopyCheck, CopyMinus, CopyPlus, CopySlash, CopyX, Copyleft, Copyright,
CornerDownLeft, CornerDownRight, CornerLeftDown, CornerLeftUp, CornerRightDown, CornerRightUp,
CornerUpLeft, CornerUpRight, Cpu, CreativeCommons, CreditCard, CreditCardCheck, CreditCardMinus,
CreditCardPlus, CreditCardReader, CreditCardX, Croissant, Crop, Cross, Crosshair, Crown, Cuboid,
CupSoda, Currency, Cylinder, Dam, Dash, Database, DatabaseArrowDown, DatabaseArrowUp,
DatabaseBackup, DatabaseCheck, DatabaseMinus, DatabasePlus, DatabaseSearch, DatabaseX,
DatabaseZap, DecimalsArrowLeft, DecimalsArrowRight, Delete, Dessert, Diameter, Diamond,
DiamondMinus, DiamondPercent, DiamondPlus, Dice1, Dice2, Dice3, Dice4, Dice5, Dice6, Dices,
Diff, Disc, Disc2, Disc3, DiscAlbum, Divide, Dna, DnaOff, Dock, Dog, DollarSign, Dome, Donut,
DoorClosed, DoorClosedLocked, DoorOpen, DoorStairwell, Dot, Download, DraftingCompass, Drama,
Drill, Drone, Droplet, DropletOff, Droplets, Drum, Drumstick, Dumbbell, Ear, EarOff, Earth,
EarthLock, Eclipse, Egg, EggFried, EggOff, Eject, Ellipse, Ellipsis, EllipsisVertical, Engine,
Equal, EqualApproximately, EqualApproximatelyNot, EqualNot, Eraser, EthernetPort, Euro,
EvCharger, Expand, ExternalLink, Eye, EyeClosed, EyeDashed, EyeOff, FaceAngry,
FaceExpressionless, FaceGrinning, FaceNeutral, FaceSlightlyFrowning, FaceSlightlySmiling,
FaceSlightlySmilingPlus, Factory, Fan, FastForward, Feather, Fence, FerrisWheel, File,
FileArchive, FileAxis3d, FileBadge, FileBox, FileBraces, FileBracesCorner, FileChartColumn,
FileChartColumnIncreasing, FileChartLine, FileChartPie, FileCheck, FileCheckCorner, FileClock,
FileCode, FileCodeCorner, FileCog, FileDiff, FileDigit, FileDown, FileExclamationPoint,
FileHeadphone, FileHeart, FileImage, FileInput, FileKey, FileLock, FileMinus, FileMinusCorner,
FileMusic, FileOutput, FilePen, FilePenLine, FilePlay, FilePlus, FilePlusCorner,
FileQuestionMark, FileScan, FileSearch, FileSearchCorner, FileSignal, FileSliders,
FileSpreadsheet, FileStack, FileSymlink, FileTerminal, FileText, FileType, FileTypeCorner,
FileUp, FileUser, FileVideoCamera, FileVolume, FileX, FileXCorner, Files, Film,
FingerprintPattern, FireExtinguisher, Fish, FishOff, FishSymbol, FishingHook, FishingRod, Flag,
FlagOff, FlagTriangleLeft, FlagTriangleRight, Flame, FlameKindling, Flashlight, FlashlightOff,
FlaskConical, FlaskConicalOff, FlaskRound, FlipHorizontal2, FlipVertical2, Flower, Flower2,
Focus, FoldHorizontal, FoldVertical, Folder, FolderArchive, FolderBookmark, FolderCheck,
FolderClock, FolderClosed, FolderCode, FolderCog, FolderDot, FolderDown, FolderGit, FolderGit2,
FolderHeart, FolderInput, FolderKanban, FolderKey, FolderLock, FolderMinus, FolderOpen,
FolderOpenDot, FolderOutput, FolderPen, FolderPlus, FolderRoot, FolderSearch, FolderSearch2,
FolderSymlink, FolderSync, FolderTree, FolderUp, FolderX, Folders, Footprints, Forklift, Form,
Forward, Frame, Fuel, Fullscreen, Funnel, FunnelPlus, FunnelX, Galaxy, GalleryHorizontal,
GalleryHorizontalEnd, GalleryThumbnails, GalleryVertical, GalleryVerticalEnd, Gamepad, Gamepad2,
GamepadDirectional, GapHorizontal, GapVertical, Gauge, Gavel, Gem, GeorgianLari, Germ, GermOff,
Ghost, Gift, GitBranch, GitBranchMinus, GitBranchPlus, GitCommitHorizontal, GitCommitVertical,
GitCompare, GitCompareArrows, GitFork, GitGraph, GitMerge, GitMergeConflict, GitPullRequest,
GitPullRequestArrow, GitPullRequestClosed, GitPullRequestCreate, GitPullRequestCreateArrow,
GitPullRequestDraft, Github, GlassWater, Glasses, Globe, GlobeCheck, GlobeLock, GlobeOff,
GlobeX, Goal, Gpu, GraduationCap, Grape, Grid2x2, Grid2x2Check, Grid2x2Plus, Grid2x2X, Grid3x2,
Grid3x3, Grip, GripHorizontal, GripVertical, Group, Guitar, Ham, Hamburger, Hammer, Hand,
HandCoins, HandFist, HandGrab, HandHeart, HandHelping, HandMetal, HandPlatter, Handbag,
Handshake, HardDrive, HardDriveDownload, HardDriveUpload, HardHat, Hash, HatGlasses, Haze, Hd,
HdmiPort, Heading, Heading1, Heading2, Heading3, Heading4, Heading5, Heading6, HeadphoneOff,
Headphones, Headset, Heart, HeartCrack, HeartHandshake, HeartMinus, HeartOff, HeartPlus,
HeartPulse, HeartX, Heater, Helicopter, Hexagon, Highlighter, Hop, HopOff, Hospital, Hotel,
Hourglass, House, HouseHeart, HousePlug, HousePlus, HouseWifi, IceCreamBowl, IceCreamCone,
IdCard, IdCardLanyard, Image, ImageDown, ImageMinus, ImageOff, ImagePlay, ImagePlus, ImageUp,
ImageUpscale, Images, Import, Inbox, IndianRupee, Infinity, Info, InspectionPanel, Inspector,
Italic, IterationCcw, IterationCw, JapaneseYen, Joystick, Kanban, Kayak, Key, KeyRound,
KeySquare, Keyboard, KeyboardMusic, KeyboardOff, Lamp, LampCeiling, LampDesk, LampFloor,
LampWallDown, LampWallUp, LandPlot, Landmark, Languages, Laptop, LaptopMinimal,
LaptopMinimalCheck, Lasso, LassoSelect, LayerArrowDown, LayerArrowUp, Layers, Layers2,
LayersArrowDown, LayersArrowUp, LayersMinus, LayersPlus, LayoutDashboard, LayoutFreeform,
LayoutGrid, LayoutList, LayoutPanelLeft, LayoutPanelTop, LayoutTemplate, Leaf, LeafyGreen,
Lectern, LensConcave, LensConvex, Library, LibraryBig, LifeBuoy, Ligature, Lightbulb,
LightbulbOff, Lighthouse, LineDotRightHorizontal, LineSquiggle, LineStyle, Link, Link2,
Link2Off, List, ListCheck, ListChecks, ListChevronsDownUp, ListChevronsUpDown, ListClock,
ListCollapse, ListEnd, ListFilter, ListFilterPlus, ListIndentDecrease, ListIndentIncrease,
ListMinus, ListMusic, ListOrdered, ListPlus, ListRestart, ListSortAscending, ListSortDescending,
ListStart, ListTodo, ListTree, ListVideo, ListX, Loader, LoaderCircle, LoaderPinwheel, Locate,
LocateFixed, LocateOff, Lock, LockKeyhole, LockKeyholeOpen, LockOpen, LogIn, LogOut, Logs,
Lollipop, Luggage, Magnet, Mail, MailBadge, MailCheck, MailClock, MailMinus, MailOpen, MailPen,
MailPlus, MailQuestionMark, MailSearch, MailWarning, MailX, Mailbox, Mails, Map, MapMinus,
MapPin, MapPinCheck, MapPinCheckInside, MapPinHouse, MapPinMinus, MapPinMinusInside, MapPinOff,
MapPinPen, MapPinPlus, MapPinPlusInside, MapPinSearch, MapPinX, MapPinXInside, MapPinned,
MapPlus, Mars, MarsStroke, Martini, Maximize, Maximize2, Medal, Megaphone, MegaphoneOff,
MemoryStick, Menu, Merge, MessageCircle, MessageCircleCheck, MessageCircleCode,
MessageCircleDashed, MessageCircleDashedCheck, MessageCircleHeart, MessageCircleMore,
MessageCircleOff, MessageCirclePlus, MessageCircleQuestionMark, MessageCircleReply,
MessageCircleWarning, MessageCircleX, MessageSquare, MessageSquareCheck, MessageSquareCode,
MessageSquareDashed, MessageSquareDiff, MessageSquareDot, MessageSquareHeart, MessageSquareLock,
MessageSquareMore, MessageSquareOff, MessageSquarePlus, MessageSquareQuote, MessageSquareReply,
MessageSquareShare, MessageSquareText, MessageSquareWarning, MessageSquareX, MessagesSquare,
Metronome, Mic, MicAudioLines, MicOff, MicSignal, MicVocal, Microchip, Microscope, Microwave,
MidiPort, Milestone, Milk, MilkOff, Minimize, Minimize2, Minus, MirrorRectangular, MirrorRound,
Monitor, MonitorCheck, MonitorCloud, MonitorCog, MonitorDot, MonitorDown, MonitorOff,
MonitorPause, MonitorPlay, MonitorSmartphone, MonitorSpeaker, MonitorStop, MonitorUp, MonitorX,
Moon, MoonStar, Mop, MopSparkles, Mosque, Motorbike, Mountain, MountainSnow, Mouse, MouseLeft,
MouseOff, MousePointer, MousePointer2, MousePointer2Off, MousePointerBan, MousePointerClick,
MouseRight, Move, Move3d, MoveDiagonal, MoveDiagonal2, MoveDown, MoveDownLeft, MoveDownRight,
MoveHorizontal, MoveLeft, MoveRight, MoveUp, MoveUpLeft, MoveUpRight, MoveVertical, Music,
Music2, Music3, Music4, Navigation, Navigation2, Navigation2Off, NavigationOff, Network,
Newspaper, Nfc, NonBinary, Notebook, NotebookPen, NotebookTabs, NotebookText, NotepadText,
NotepadTextDashed, Nut, NutOff, Octagon, OctagonAlert, OctagonMinus, OctagonPause, OctagonX,
Omega, Option, Orbit, Origami, Package, Package2, PackageCheck, PackageMinus, PackageOpen,
PackagePlus, PackageSearch, PackageX, PaintBucket, PaintRoller, Paintbrush, PaintbrushVertical,
Palette, Panda, PanelBottom, PanelBottomClose, PanelBottomDashed, PanelBottomOpen, PanelLeft,
PanelLeftClose, PanelLeftDashed, PanelLeftOpen, PanelLeftRightDashed, PanelRight,
PanelRightClose, PanelRightDashed, PanelRightOpen, PanelTop, PanelTopBottomDashed,
PanelTopClose, PanelTopDashed, PanelTopOpen, PanelsLeftBottom, PanelsRightBottom, PanelsTopLeft,
PaperBag, Paperclip, Parasol, Parentheses, ParkingMeter, PartyPopper, Pause, PawPrint, PcCase,
Pen, PenLine, PenOff, PenTool, Pencil, PencilLine, PencilOff, PencilRuler, PencilSparkles,
Pentagon, Percent, PersonStanding, Phi, PhilippinePeso, Phone, PhoneCall, PhoneForwarded,
PhoneIncoming, PhoneMissed, PhoneOff, PhoneOutgoing, Pi, Piano, Pickaxe, PictureInPicture,
PictureInPicture2, PiggyBank, Pilcrow, PilcrowLeft, PilcrowRight, Pill, PillBottle, Pin, PinOff,
Pipette, Pizza, Plane, PlaneLanding, PlaneTakeoff, Play, PlayOff, PlayingCard, PlayingCards,
PlayingCardsFan, Plug, Plug2, PlugZap, Plus, PocketKnife, Podium, Pointer, PointerOff, Popcorn,
Popsicle, PoundSterling, Power, PowerOff, Presentation, Printer, PrinterCheck, PrinterX,
Projector, Proportions, Puzzle, Pyramid, QrCode, Quote, Rabbit, Radar, Radiation, Radical,
Radio, RadioOff, RadioReceiver, RadioTower, Radius, Rainbow, Rat, Ratio, Receipt, ReceiptCent,
ReceiptEuro, ReceiptIndianRupee, ReceiptJapaneseYen, ReceiptPoundSterling, ReceiptRussianRuble,
ReceiptSwissFranc, ReceiptText, ReceiptTurkishLira, RectangleCircle, RectangleEllipsis,
RectangleGoggles, RectangleHorizontal, RectangleVertical, Recycle, Redo, Redo2, RedoDot,
RefreshCcw, RefreshCcwDot, RefreshCw, RefreshCwOff, Refrigerator, Regex, RemoveFormatting,
Repeat, Repeat1, Repeat2, RepeatOff, Replace, ReplaceAll, Reply, ReplyAll, ResizeCorner, Rewind,
Ribbon, Road, RobotArm, RobotVacuum, Rocket, RockingChair, RollerCoaster, Rose, Rotate3d,
RotateCcw, RotateCcwClock, RotateCcwKey, RotateCcwSquare, RotateCw, RotateCwFadingClock,
RotateCwSquare, Route, RouteOff, Router, Rows2, Rows3, Rows4, Rss, Ruler, RulerDimensionLine,
RussianRuble, Sailboat, Salad, Sandwich, Satellite, SatelliteDish, SaudiRiyal, Save, SaveAll,
SaveCheck, SaveOff, SavePen, SavePlus, Scale, Scale3d, Scaling, Scan, ScanBarcode, ScanBox,
ScanEye, ScanFace, ScanHeart, ScanLine, ScanQrCode, ScanSearch, ScanSquare, ScanText, School,
Scissors, ScissorsLineDashed, Scooter, ScreenShare, ScreenShareOff, Scroll, ScrollText, Search,
SearchAlert, SearchCheck, SearchCode, SearchSlash, SearchX, Section, Send, SendHorizontal,
SendToBack, SeparatorHorizontal, SeparatorVertical, Server, ServerCog, ServerCrash, ServerOff,
ServerPlus, Settings, Settings2, Shapes, Share, Share2, Sheet, Shell, ShelvingUnit, Shield,
ShieldAlert, ShieldBan, ShieldCheck, ShieldCog, ShieldCogCorner, ShieldEllipsis, ShieldHalf,
ShieldKeyhole, ShieldLock, ShieldMinus, ShieldOff, ShieldPlus, ShieldQuestionMark, ShieldUser,
ShieldX, Ship, ShipCargo, ShipWheel, Shirt, ShoppingBag, ShoppingBasket, ShoppingCart,
ShoppingCartMinus, ShoppingCartPlus, Shovel, ShowerHead, Shredder, Shrimp, ShrimpOff, Shrink,
Shrub, Shuffle, Sigma, Signal, SignalHigh, SignalLow, SignalMedium, SignalZero, Signature,
Signpost, SignpostBig, Siren, SkipBack, SkipForward, Skull, Slash, Slice, SlidersHorizontal,
SlidersVertical, Smartphone, SmartphoneCharging, SmartphoneNfc, Snail, Snowflake,
SoapDispenserDroplet, Sofa, SolarPanel, SortAscending, SortDescending, Soup, Space, Spade,
Sparkle, Sparkles, Speaker, Speech, SpellCheck, SpellCheck2, Spline, SplinePointer, Split,
Spool, SportShoe, Spotlight, SprayCan, Sprout, Square, SquareActivity, SquareArrowDown,
SquareArrowDownLeft, SquareArrowDownRight, SquareArrowLeft, SquareArrowOutDownLeft,
SquareArrowOutDownRight, SquareArrowOutUpLeft, SquareArrowOutUpRight, SquareArrowRight,
SquareArrowRightEnter, SquareArrowRightExit, SquareArrowUp, SquareArrowUpLeft,
SquareArrowUpRight, SquareAsterisk, SquareBottomDashedScissors,
SquareCenterlineDashedHorizontal, SquareCenterlineDashedVertical, SquareChartGantt, SquareCheck,
SquareCheckBig, SquareChevronDown, SquareChevronLeft, SquareChevronRight, SquareChevronUp,
SquareCode, SquareDashed, SquareDashedBottom, SquareDashedBottomCode, SquareDashedKanban,
SquareDashedMousePointer, SquareDashedText, SquareDashedTopSolid, SquareDimensions,
SquareDivide, SquareDot, SquareEqual, SquareExclamationPoint, SquareFunction, SquareKanban,
SquareLibrary, SquareM, SquareMenu, SquareMinus, SquareMousePointer, SquareOff, SquareParking,
SquareParkingOff, SquarePause, SquarePen, SquarePercent, SquarePi, SquarePilcrow, SquarePlay,
SquarePlus, SquarePower, SquareRadical, SquareRoundCorner, SquareScissors, SquareSigma,
SquareSlash, SquareSplitHorizontal, SquareSplitVertical, SquareSquare, SquareStack, SquareStar,
SquareStop, SquareTerminal, SquareText, SquareUser, SquareUserRound, SquareX, SquaresExclude,
SquaresIntersect, SquaresSubtract, SquaresUnite, Squircle, SquircleDashed, Squirrel, Stamp,
Star, StarCheck, StarFill, StarHalf, StarMinus, StarOff, StarPlus, StarX, StepBack, StepForward,
Stethoscope, Sticker, StickyNote, StickyNoteCheck, StickyNoteMinus, StickyNoteOff,
StickyNotePlus, StickyNoteX, StickyNotes, Stone, Store, StretchHorizontal, StretchVertical,
Strikethrough, Subscript, Summary, Sun, SunDim, SunMedium, SunMoon, SunSnow, Sunrise, Sunset,
Superscript, SwatchBook, SwissFranc, SwitchCamera, Sword, Swords, Syringe, Table, Table2,
TableCellsMerge, TableCellsSplit, TableColumnsSplit, TableOfContents, TableProperties,
TableRowsSplit, Tablet, TabletSmartphone, Tablets, Tag, TagPlus, TagX, Tags, Tally1, Tally2,
Tally3, Tally4, Tally5, Tangent, Target, Telescope, Tent, TentTree, Terminal, TestTube,
TestTubeDiagonal, TestTubes, TextAlignCenter, TextAlignEnd, TextAlignJustify, TextAlignStart,
TextCursor, TextCursorInput, TextInitial, TextQuote, TextSearch, TextWrap, Theater, Thermometer,
ThermometerSnowflake, ThermometerSun, ThumbsDown, ThumbsUp, TicTacToe, Ticket, TicketCheck,
TicketMinus, TicketPercent, TicketPlus, TicketSlash, TicketX, Tickets, TicketsPlane, Timeline,
Timer, TimerOff, TimerReset, ToggleLeft, ToggleRight, Toilet, ToolCase, Toolbox, Tornado, Torus,
Touchpad, TouchpadOff, TowelRack, TowerControl, ToyBrick, Tractor, TrafficCone, Trailer,
TrainFront, TrainFrontTunnel, TrainTrack, TramFront, Transgender, Trash, TrashOff,
TreeDeciduous, TreePalm, TreePine, Trees, TrendingDown, TrendingUp, TrendingUpDown, Triangle,
TriangleAlert, TriangleDashed, TriangleRight, Trophy, Truck, TruckElectric, TurkishLira,
Turntable, Turtle, Tv, TvMinimal, TvMinimalPlay, Type, TypeOutline, Umbrella, UmbrellaOff,
Underline, Undo, Undo2, UndoDot, UnfoldHorizontal, UnfoldVertical, Ungroup, University, Unlink,
Unlink2, Unplug, Upload, Usb, UsbCPort, User, UserCheck, UserCog, UserGroup, UserKey, UserLock,
UserMinus, UserPen, UserPlus, UserRound, UserRoundArrowLeft, UserRoundCheck, UserRoundCog,
UserRoundGroup, UserRoundKey, UserRoundMinus, UserRoundPen, UserRoundPlus, UserRoundSearch,
UserRoundX, UserSearch, UserShield, UserStar, UserX, Users, UsersRound, Utensils,
UtensilsCrossed, UtilityPole, Van, Variable, Vault, VectorPolygon, VectorSquare, Vegan,
VenetianMask, Venus, VenusAndMars, Vibrate, VibrateOff, Video, VideoOff, Videotape, View, Virus,
VirusOff, Voicemail, Volleyball, Volume, Volume1, Volume2, VolumeOff, VolumeX, Vote, Wallet,
WalletCards, WalletMinimal, Wallpaper, Wand, WandSparkles, Warehouse, WashingMachine, Watch,
WavesArrowDown, WavesArrowUp, WavesHorizontal, WavesLadder, WavesVertical, Waypoints, Webcam,
WebcamOff, Webhook, WebhookOff, Weight, WeightTilde, Wheat, WheatOff, Whistle, WholeWord, Wifi,
WifiCog, WifiHigh, WifiLow, WifiOff, WifiPen, WifiSync, WifiZero, Wind, WindArrowDown,
WindowClose, WindowMaximize, WindowMinimize, WindowRestore, Wine, WineOff, Workflow, Worm,
Wrench, WrenchOff, X, XLineTop, Zap, ZapOff, ZodiacAquarius, ZodiacAries, ZodiacCancer,
ZodiacCapricorn, ZodiacGemini, ZodiacLeo, ZodiacLibra, ZodiacOphiuchus, ZodiacPisces,
ZodiacSagittarius, ZodiacScorpio, ZodiacTaurus, ZodiacVirgo, ZoomIn, ZoomOut
```

---

## 16. Gotchas

1. **`Root` must be the window's first view.** `Root::new(view, window, cx)` has to be what the
   `open_window` closure returns; every `WindowExt` overlay method goes through
   `Root::update`, which panics with `"BUG: window first layer should be a gpui_component::Root."`
   if it isn't (`CMP/src/root.rs:160-163`). `Root::read` instead panics with
   `"The window root view should be of type \`ui::Root\`."` (`CMP/src/root.rs:179`).
2. **Overlay layers must be rendered by the app view.** `Root::render_dialog_layer`,
   `render_sheet_layer`, `render_notification_layer` all return `None` until something is pushed;
   if the view never `.children(...)` them, dialogs/sheets/notifications are created and later
   dismissed but never painted (`KIT/tests/overlays.rs:20-22, 82-85`).
3. **`gpui_kit::init(cx)` must run before constructing any component.** Every kit test starts with
   `cx.update(gpui_kit::init)` (`KIT/tests/overlays.rs:94`, `KIT/tests/menu.rs:39`,
   `KIT/tests/search.rs:59`, `KIT/tests/components.rs:46`). Without it, `Theme` is not a global and
   `cx.theme()` panics inside `Theme::global`.
4. **`WindowExt` methods need `&mut Window`; `ActiveTheme` needs `&App`.** There is no
   `ActiveTheme for Window`, so a function that only has `&Window` cannot read the theme; and
   `open_dialog`/`push_notification` cannot be called from `cx`-only callbacks. The combination
   `|_, window, cx|` in `Button::on_click` is the only place both are available
   (`CMP/src/window_ext.rs:1-10`, `CMP/src/theme/mod.rs:44-53`).
5. **`Theme` has only `radius` and `radius_lg`.** There is no `radius_sm`/`radius_md`/`radius_xl`
   on `Theme`. Use `cx.theme().radius_tokens().{sm,md,lg,xl,full}` or the
   `radius_full()`/`radius_2xl()`… helpers (`CMP/src/theme/mod.rs:445-471`).
6. **There is no `Theme::update(cx, …)`.** Use `Theme::global_mut(cx)` and then
   `Theme::sync_base(cx)`; without the sync, scrollbars and resize handles keep the old colors,
   radius and fonts (`KIT/tests/rendering.rs:135-136`, `CMP/src/theme/mod.rs:203-207`).
7. **There is no `cx.background_spawn`.** `cx.background_executor().spawn(..)` is the only path;
   the kit tests use `cx.executor()` inside `TestAppContext` (`KIT/tests/interactions.rs:200`).
8. **`cx.spawn` arity changes with the receiver.** `App`/`AsyncApp` → `|cx|` (one `&mut AsyncApp`);
   `Context<T>` → `|this, cx|` (a `WeakEntity<T>` plus `&mut AsyncApp`)
   (`GPUI/src/app.rs:2036`, `GPUI/src/app/context.rs:230`). Do not mix them up.
9. **`Button::on_click` gives `&ClickEvent`, not `&MouseEvent`,** and the order is
   `(event, window, app)`. `on_hover` gives `&bool` first, not the event.
10. **`Switch`/`Checkbox`/`Radio` are controlled.** The callback receives the requested value;
    nothing changes unless the owner stores it and calls `cx.notify()`. `on_click` and `on_change`
    are the same slot and the last one registered wins (`CMP/src/switch.rs:78-94`).
11. **`InputState` has no `on_change`/`on_submit`.** Subscribe to the entity and match
    `InputEvent::{Change, PressEnter{secondary,shift}, Focus, Blur}`
    (`BASE/src/input/base/state.rs:122-127`, `CMP/src/setting/fields/string.rs:59-69`).
12. **`InputState::set_value` does not emit `InputEvent::Change`.** List/search code documents this
    and manually kicks off the search (`CMP/src/input/search.rs:144`, `CMP/src/list/list.rs:221`).
    Guard programmatic writes by comparing the current value first.
13. **`Dialog::on_ok`/`on_cancel` must return `bool`.** `false` keeps the dialog open; returning
    `()` will not compile (`CMP/src/dialog/dialog.rs:369-383`).
14. **`DialogFooter` overrides `button_props`.** Setting `.footer(...)` disables the automatic
    ok/cancel buttons (`CMP/src/dialog/dialog.rs:335-338`).
15. **`set_value` on a `List`-owned query input and `ListState::set_query` are different entry
    points**; `set_query` is also documented to start the search because `set_value` doesn't emit
    (`CMP/src/list/list.rs:215-226`).
16. **A `List` still needs a definite parent box.** `List`/`ListState` render `size_full()`; under
    an auto-height parent the virtual list measures 0 and never scrolls. Give it `.flex_1()`,
    `.h(..)`, or `.size_full()` (`CMP/src/list/list.rs:660-666, 776-788`).
17. **`List::render` moves padding and `max_size.height` out of the element style into
    `ListOptions`** and clears them, so `.p_4()` / `.max_h(..)` on the `List` apply to the inner
    virtual list, not the outer `div` (`CMP/src/list/list.rs:778-790`).
18. **`TitleBar` has no `title`/`tooltip` builders.** It only has `new()`, `on_close_window()`,
    `title_bar_options()`, `window_options()` and the `Styled`/`ParentElement` traits. Set the OS
    title via `WindowOptions.titlebar: Some(TitlebarOptions{ title, .. })`, and put your own title
    element in as a child (`CMP/src/title_bar.rs:48-117, 296-310`).
19. **`TitleBar::on_close_window` is Linux-only.** On other platforms the closure is dropped
    (`if cfg!(target_os = "linux")`), and the close button calls `window.remove_window()`
    (`CMP/src/title_bar.rs:95-105`).
20. **A window with a custom `TitleBar` should use `TitleBar::window_options()`** so
    `app_owns_titlebar_drag: true` and the traffic-light offset are set; otherwise AppKit delays
    titlebar clicks and handles double-clicks itself (`CMP/src/title_bar.rs:67-93`).
21. **There is no `window.open_context_menu`.** Menus are attached with
    `DropdownMenu::dropdown_menu(..)` on an element, or `ContextMenuExt::context_menu(..)`
    (`CMP/src/menu/dropdown_menu.rs:12-34`, `CMP/src/menu/context_menu.rs:13-34`).
22. **`PopupMenu::build` is the only constructor**; `PopupMenu::new` is `pub(crate)`
    (`CMP/src/menu/popup_menu.rs:333` vs `:358`).
23. **`Submenu` and `menu_element*` need `window` and `cx` at call time**, so submenu-building
    closures are `|menu, window, cx|` and cannot be stored as a plain list of items
    (`CMP/src/menu/popup_menu.rs:694-699`; `KIT/tests/menu.rs:32-34`).
24. **`Icon`/`IconName` need an asset source.** `IconName::path()` returns `"icons/<stem>.svg"`;
    without `.with_assets(gpui_kit::assets::Assets)` (or `icon_assets!(..)`), SVG loads fail and the
    icon renders blank (`ASSETS/src/lib.rs:20-24`, `KIT/tests/rendering.rs:27-45`).
25. **There is no public `IconButton`** — `button_icon` is `pub(crate)`. Use
    `Button::new(id).icon(..)`; an icon with no label becomes icon-only automatically
    (`CMP/src/button/mod.rs:9`, `CMP/src/button/button.rs:308`).
26. **`Element::request_layout` returns raw layout state; `prepaint` and `paint` are the only
    methods that receive `bounds: Bounds<Pixels>`.** There is no `window.layout_bounds` field.
    Using `cx` (an `&mut App`) inside `paint` to read the theme is fine; mutating entities during
    `paint` is not.
27. **`shape_line` is on `TextSystem`, not `Window`** — call
    `window.text_system().shape_line(..)`; it panics if the text contains `\n`
    (`GPUI/src/text_system.rs:638-649`).
28. **`Image`/`Rgba` vs `Hsla`:** theme tokens are `Hsla`; `PaintQuad.background` is `Background`
    (built from `Hsla`/`Rgba`); `PaintQuad.border_color` is `Hsla`. Mixing the two requires an
    explicit `.into()`.
29. **`push_notification` requires `&mut Window`** even though it is conceptually app-level
    (`CMP/src/window_ext.rs:58`).
30. **The default notification autohide is 5 s** and the toast entrance animation is 400 ms of
    wall-clock time; tests must sleep past it because the test clock does not drive it
    (`KIT/tests/overlays.rs:127-131, ~298-302`).
31. **`ListDelegate::render_item` must render equal-height items.** The trait doc states
    "Every item should have same height" (and the same for section headers/footers)
    (`CMP/src/list/delegate.rs:38-40, 52-54, 61-63`).
32. **`ListState::scroll_to_item` takes an `IndexPath`, not a `usize`**, and window/context
    (`CMP/src/list/list.rs:239`); `VirtualListScrollHandle::scroll_to_item` takes a `usize` index
    (`BASE/src/virtual_list.rs:107`).
33. **Async entity access from a task must go through `update`/`update_in`.** `WeakEntity::update`
    is what every async example uses before `cx.notify()`
    (`KIT/tests/interactions.rs:208-214`); there is no way to borrow an entity across `await`.
34. **`Entity<T>` handles are cheap clones; the kit tests always `clone()` them into closures**
    before `move` closures (`KIT/tests/overlays.rs:30-63`). Forgetting the clone produces a
    "use of moved value" at the second closure.
