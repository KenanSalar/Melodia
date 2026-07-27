---
paths:
  - ui/**/*.slint
  - src/ui/**/*.rs
  - src/boot/**/*.rs
  - src/themes/**/*.rs
  - melodia-ui/build.rs
---

# Slint Best Practices

## Project Layout

- Use `.slint` files compiled via `melodia-ui/build.rs` (`slint_build::compile_with_config("../ui/app-window.slint", …)`) — **do not** use the `slint::slint!{}` macro outside of toy demos. Files give you syntax highlighting, partial rebuilds, and proper diagnostics.
- One root component per app, additional components per file. Group by folder: `ui/layout/`, `ui/views/`, `ui/components/`.
- A single `theme.slint` global holds design tokens (brushes, sizes, durations) — every other component imports `Theme` and reads from it.
- Mirror Rust struct definitions used at the boundary in a `models.slint` file (`export struct TrackRow { … }`); both sides must agree exactly on field names and types.

## Properties

- Be explicit about direction at component boundaries:
  - `in property <T>` — Rust → Slint (read-only on Slint side).
  - `out property <T>` — Slint → Rust (read-only on Rust side, set internally by the component).
  - `in-out property <T>` — two-way bound, used for things like sliders and toggles.
  - default (no qualifier) — internal/private to the component.
- **Bind, don't mutate**: prefer derived expressions over imperative `set_*` calls. Push the source of truth into one property and let dependent properties compute from it.
- Two-way binding inside `.slint` uses `<=>`: `value <=> root.volume;` keeps a child slider's `value` and a parent's `volume` synchronized.
- Property names in `.slint` are kebab-case (`current-track-id`); generated Rust APIs convert to snake_case (`get_current_track_id`, `set_current_track_id`).

## Callbacks

- Slint → Rust events go through callbacks declared as `callback name(args) -> ret;`. Register from Rust with `ui.on_name(move |args| { … });`.
- **Always capture `ui.as_weak()`**, not the strong handle, to avoid reference cycles between the closure and the UI:
  ```rust
  let weak = ui.as_weak();
  ui.on_play_pause(move || {
      let Some(ui) = weak.upgrade() else { return; };
      // use ui
  });
  ```
- Callbacks run on the **UI thread**. Don't do blocking work inside them — `runtime.spawn(async move { … })` and push results back via channels or `invoke_from_event_loop`.
- Naming: kebab-case in `.slint` (`play-pause`), snake_case in Rust (`on_play_pause`, `invoke_play_pause`).

## Models for Lists

- Lists in `.slint` are typed as `[T]` (a model). The standard backing type from Rust is `Rc<VecModel<T>>` wrapped via `ModelRc::from(rc.clone())`.
- Hand the `ModelRc` to the UI: `ui.set_tracks(ModelRc::from(model_rc.clone()));`. Keep the `Rc<VecModel<_>>` somewhere you can mutate later.
- **Mutating from a background thread**: use the `weak.upgrade_in_event_loop` + `as_any().downcast_ref` pattern:
  ```rust
  let weak = ui.as_weak();
  weak.upgrade_in_event_loop(move |ui| {
      let m = ui.get_tracks();
      let vec = m.as_any().downcast_ref::<VecModel<TrackRow>>().expect("VecModel<TrackRow>");
      vec.set_vec(new_tracks);
  });
  ```
- For very large lists, use `ListView` which only instantiates rows in view. `for item in items` inside a `ListView` is virtualized.
- Replacing the entire model is fine for small/medium changes; for incremental updates use `VecModel::push`, `set_row_data`, `remove`.

## Globals

- `export global Theme { out property <brush> base: #1e1e2e; }` — define cross-cutting state and tokens.
- Access in `.slint`: `Theme.base`. Access from Rust:
  - read: `ui.global::<Theme>().get_base()`
  - write: `ui.global::<Theme>().set_base(slint::Brush::from(color))`
  - register callback: `ui.global::<Theme>().on_some_callback(move || …)`
- Use globals for Theme, Player view-model, current-route, and anything else that many components read.

## Cross-Thread Updates

- The Slint event loop is **single-threaded** — only the thread that ran `slint::run_event_loop()` may call UI APIs.
- From a background thread (e.g. tokio worker, std thread), choose:
  1. `slint::invoke_from_event_loop(move || { … })` — fire-and-forget closure to the UI thread. Best for one-shot updates.
  2. `weak_handle.upgrade_in_event_loop(move |ui| { … })` — same as (1) but auto-handles a dropped UI; preferred when you have a `Weak<MyApp>`.
  3. `slint::spawn_local(async_compat::Compat::new(async { … }))` — spawn a Future **on the UI thread** that may `.await` tokio futures (the `Compat` adapter provides the tokio runtime context). Best for reactive update loops that consume `tokio::sync::watch::Receiver` / `mpsc::Receiver`.
- Never call `ui.set_*` from a background thread directly — it will panic.

## Tokio Integration

- Build a `tokio::runtime::Runtime` (multi-thread) in `main.rs`, hold an `Arc` (or pass a `Handle`) into anything that needs to spawn async work.
- The Slint event loop runs on the **main thread**. Don't run the tokio runtime on the same thread; instead let it own its own worker threads, and use `slint::spawn_local(async_compat::Compat::new(...))` to bridge in.
- For periodic UI updates driven by tokio (position ticks, toast timers), prefer `tokio::sync::watch` + a `spawn_local` consumer over `tokio::time::interval` running on the UI thread.

## Layout

- Use `VerticalLayout` / `HorizontalLayout` / `GridLayout` for structural layout. Reserve absolute positioning (`x:`/`y:`) for overlays, popups, and computed positions.
- Use `horizontal-stretch` / `vertical-stretch` to control how extra space is distributed; default is 0 (no stretch).
- `min-width` / `max-width` / `preferred-width` (and the height equivalents) drive sizing. The window's own `min-width`/`min-height` set the resize floor.
- For scrollable regions: wrap content in `ScrollView { … }`. For virtualized data: `ListView { for item in items: Row { … } }`.

## Animations

- `animate <property> { duration: 200ms; easing: ease-in-out; }` placed inside a component animates whenever that property changes.
- Match the design tokens defined in `Theme` (e.g. `Theme.duration-fast: 200ms`, `Theme.duration-medium: 250ms`).
- Use `states` and `transitions` for multi-property state machines (e.g. hover/pressed/selected) instead of stitching individual `animate` blocks.

## Custom Widgets

- Create a component (e.g. `IconButton`) by composing `Rectangle` + `TouchArea` + `Image` + `Text`. Expose `in property` for inputs, `callback clicked` for events.
- Reach for `std-widgets.slint` (`Button`, `Slider`, `LineEdit`, `ScrollView`, `ListView`, `StandardTableView`) when their look fits; replace with custom components when the design demands it.
- A custom slider that needs precise styling: `Rectangle { TouchArea { moved => { … } } }` reading `mouse-x` / `mouse-y` deltas.

## Keyboard & Focus

- Wrap content needing keyboard handlers in `FocusScope { … }`. Listen via `key-pressed(event)` and `key-released(event)` callbacks; return `accept` / `reject` from the event handler.
- The root component should generally be a `FocusScope` so global shortcuts always have a target.
- Use `forward-focus: child;` to delegate focus into a specific child on activation.

## Popups & Dialogs

- `PopupWindow { … }` for context menus, dropdowns, tooltips. They overlay the app and dismiss on click outside.
- For modal dialogs (confirm, edit playlist, etc.), build a `Rectangle` with a backdrop and a centered card; gate visibility with `visible: state.dialog-open;` and animate via `states`.

## Images & Fonts

- **Images at compile time**: `Image { source: @image-url("assets/icons/play.svg"); }` — bakes the asset into the binary, no I/O at runtime.
- **Images at runtime**: `slint::Image::load_from_path(path)` (e.g. for album artwork on disk) and assign to an `Image`'s `source` property.
- Use SVGs for icons — they scale, take less RAM than rasterized variants, and tree-shake naturally per file.
- **Fonts**: bundle UI font(s) under `ui/assets/fonts/`. Register at app startup with `slint::FontFile::load_from_path("…")` (or `load_from_data` for embedded fonts) so `default-font-family: "Inter";` resolves.

## Animations & Performance

- Slint redraws on a property change schedule — avoid setting properties from a tight loop on the UI thread.
- For lists with thousands of rows, use `ListView`'s virtualization. Don't put thousands of plain `for` items inside a non-virtualizing parent — Slint will instantiate them all.
- Avoid heavy expressions inside frequently-rendered components; precompute in Rust and feed via a property.
- `Image`s are cached by `source`; reusing the same `@image-url` doesn't double allocate.

## Testing

- The `slint::testing` module exposes `send_keyboard_string_sequence`, `send_mouse_click`, etc. for synthetic input on a built window.
- Treat the UI as a thin reactive shell — test the **library** layer (Rust functions) thoroughly and keep UI tests light.
- Run the app under `RUST_BACKTRACE=1` during dev so panics from inside the UI thread (e.g. wrong `downcast_ref::<VecModel<_>>`) are debuggable.

## Common Pitfalls

- **Holding a strong `ComponentHandle` in a callback** — creates a reference cycle that leaks the UI on close. Always `as_weak()`.
- **Calling `ui.set_*` from a background thread** — panics. Use `invoke_from_event_loop` / `upgrade_in_event_loop` / `spawn_local`.
- **Mismatched struct fields between Rust and `.slint`** — silent: extra fields in Rust are ignored, missing ones default. Keep the `models.slint` file alongside the Rust definitions and review changes together.
- **Forgetting `ModelRc::from(rc.clone())`** — passing the `Rc` directly doesn't compile; wrap in `ModelRc`.
- **Using `slint::slint!` for non-trivial code** — gives no incremental rebuild benefit and pollutes diff readability. Stick to `.slint` files + `melodia-ui/build.rs`.
- **Blocking the UI thread** with synchronous DB queries or HTTP — drop frames, freeze the app. Always do I/O on the tokio runtime.
