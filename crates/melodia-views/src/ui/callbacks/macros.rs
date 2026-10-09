//! Shared spawn / wire macros for every per-view `wire_*` module. Re-exported at the
//! bottom; sibling files bring them in with `use super::macros::*;`.

/// Spawn `$fut` on `$state`'s runtime, logging any error with `$label`. The caller must
/// already have a local binding `$state` (typically `let s = s.clone();` at closure entry)
/// so `$fut`'s `&s` token resolves to the moved-in clone.
///
/// `$label` is any `Display` expression, evaluated only on failure, so a generic caller can
/// pass a `format_args!` naming its view.
macro_rules! spawn_logged {
    ($state:ident, $label:expr, $fut:expr) => {{
        $state.runtime.clone().spawn(async move {
            if let Err(e) = $fut.await {
                log::warn!("{}: {}", $label, melodia_core::error::describe(&e));
            }
        });
    }};
}

/// Sync variant of `spawn_logged!` for `library::*` functions that are not `async`
/// **and do no file I/O** — the transport calls, which take a `parking_lot::Mutex` and
/// reach the playback engine. Onto the runtime, so the UI thread isn't blocked. A `views.json` /
/// `settings.json` write wants [`spawn_blocking_logged!`]; the pool is the difference
/// and the two are not interchangeable.
macro_rules! spawn_logged_sync {
    ($state:ident, $label:literal, $expr:expr) => {{
        $state.runtime.clone().spawn(async move {
            if let Err(e) = $expr {
                log::warn!("{}: {}", $label, melodia_core::error::describe(&e));
            }
        });
    }};
}

/// The persist-and-forget shape: a synchronous `library::settings::*` write on the
/// **blocking** pool, warning on failure. Separate from `spawn_logged_sync!` because the
/// pool is the whole difference — these bodies open, rewrite and fsync a JSON file, which
/// an async worker must not be parked on.
///
/// **The label is a literal, which is the one reason a site legitimately stays
/// hand-rolled**: `Nav.persist-selected-index` interpolates the index it failed to store.
///
/// Every site writes `views.json`, which is what the `debug` line can call it — column
/// toggles and widths, sort, browse path and view mode leave no other trace.
/// `AppState::persist_blocking` is the `settings.json` half of the same idea.
macro_rules! spawn_blocking_logged {
    ($state:ident, $label:literal, $expr:expr) => {{
        // Before the spawn, so a write that hangs still says what it was.
        log::debug!("view state: {}", $label);
        $state.runtime.spawn_blocking(move || {
            if let Err(e) = $expr {
                log::warn!("{}: {}", $label, melodia_core::error::describe(&e));
            }
        });
    }};
}

/// Sync wire for `library::*` functions taking `&AppState`. Still hops onto the runtime
/// so the UI thread doesn't stall on the callback body's lock and engine call.
macro_rules! wire_sync {
    ($target:expr, $method:ident, $state:expr, $label:literal, $libfn:path) => {{
        let s = $state.clone();
        $target.$method(move || {
            let s = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = $libfn(&s) {
                    log::warn!("{}: {}", $label, melodia_core::error::describe(&e));
                }
            });
        });
    }};
}

/// Playback-context async wire. The wrapped `library::playback::*` function takes
/// `&PlaybackContext`, snapshotted inside the spawned future and passed by reference so
/// the future doesn't hold a borrow across `.await`.
macro_rules! wire_pb {
    ($target:expr, $method:ident, $state:expr, $label:literal, $libfn:path) => {{
        let s = $state.clone();
        $target.$method(move || {
            let s = s.clone();
            s.runtime.clone().spawn(async move {
                let ctx = s.playback_ctx();
                if let Err(e) = $libfn(&ctx).await {
                    log::warn!("{}: {}", $label, melodia_core::error::describe(&e));
                }
            });
        });
    }};
}

/// Sync variant of [`wire_pb!`].
macro_rules! wire_sync_pb {
    ($target:expr, $method:ident, $state:expr, $label:literal, $libfn:path) => {{
        let s = $state.clone();
        $target.$method(move || {
            let s = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = $libfn(&s.playback_ctx()) {
                    log::warn!("{}: {}", $label, melodia_core::error::describe(&e));
                }
            });
        });
    }};
}

/// Wire an `on_toggle_row_favorite` / `on_set_row_rating`-shaped callback: collect ids,
/// bail on an empty set, hop onto the runtime, run the async setter, then run `after` for
/// the optimistic per-view patch. The Slint side passes an `[int]`, so a single-row click
/// and a multi-select batch share one path.
///
/// `captures:` lists the local bindings `after` needs, each cloned once into the callback
/// and once per invocation. `$label` is `spawn_logged!`'s.
macro_rules! wire_row_flag {
    (
        $target:expr, $method:ident, $state:expr, $label:expr,
        $setter:path, $collect:path,
        captures: [$($cap:ident),* $(,)?],
        after: |$ids:ident, $val:ident| $after:block
    ) => {{
        let s = $state.clone();
        $(let $cap = $cap.clone();)*
        $target.$method(move |ids, value| {
            let $ids = $collect(&ids);
            if $ids.is_empty() {
                return;
            }
            let s = s.clone();
            $(let $cap = $cap.clone();)*
            s.runtime.clone().spawn(async move {
                if let Err(e) = $setter(&s, $ids.clone(), value).await {
                    log::warn!("{}: {}", $label, melodia_core::error::describe(&e));
                    return;
                }
                let $val = value;
                $after
            });
        });
    }};
}

pub(in crate::ui) use {
    spawn_blocking_logged, spawn_logged, spawn_logged_sync, wire_pb, wire_row_flag, wire_sync,
    wire_sync_pb,
};
