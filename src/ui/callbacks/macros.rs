//! Shared spawn / wire macros used by every per-view `wire_*` module.
//! Re-exported with `pub(super) use` at the bottom; sibling files bring
//! them in with `use super::macros::*;`.

/// Spawn `$fut` (an async expression yielding `AppResult<()>`) on the runtime
/// owned by `$state`, logging any error with `$label`. The caller must already
/// have a local binding `$state` (typically `let s = s.clone();` at closure
/// entry) so that `$fut`'s `&s` token resolves to the moved-in clone.
macro_rules! spawn_logged {
    ($state:ident, $label:literal, $fut:expr) => {{
        $state.runtime.clone().spawn(async move {
            if let Err(e) = $fut.await {
                log::warn!("{}: {e}", $label);
            }
        });
    }};
}

/// Like [`spawn_logged!`] but ALSO surfaces the failure to the user as an
/// error toast (localized "Something went wrong" title + the error as the
/// body) via the `services::toast` bridge. Reserve this for user-initiated
/// operations whose silent failure is confusing — a folder scan or import that
/// appears to do nothing. Routine / low-value failures (favorite toggles, nav)
/// keep using [`spawn_logged!`] so the toast stack isn't spammed.
macro_rules! spawn_logged_toast {
    ($state:ident, $label:literal, $fut:expr) => {{
        $state.runtime.clone().spawn(async move {
            if let Err(e) = $fut.await {
                log::warn!("{}: {e}", $label);
                $crate::services::toast::notify(
                    $crate::services::toast::ToastKind::OperationFailed,
                    e.to_string(),
                );
            }
        });
    }};
}

/// Sync variant of `spawn_logged!` for `library::*` functions that are not
/// `async`. Spawns the call onto the runtime so the UI thread isn't blocked
/// (the body still acquires a `parking_lot::Mutex` and calls Rodio).
macro_rules! spawn_logged_sync {
    ($state:ident, $label:literal, $expr:expr) => {{
        $state.runtime.clone().spawn(async move {
            if let Err(e) = $expr {
                log::warn!("{}: {e}", $label);
            }
        });
    }};
}

/// Sync wire for `library::*` functions taking `&AppState`. Collapses
/// the 7-line "clone state, register closure, clone again, spawn,
/// dispatch, log" boilerplate. Still hops onto the runtime so the UI
/// thread does not stall on the callback body (lock + Rodio call).
macro_rules! wire_sync {
    ($target:expr, $method:ident, $state:expr, $label:literal, $libfn:path) => {{
        let s = $state.clone();
        $target.$method(move || {
            let s = s.clone();
            s.runtime.clone().spawn(async move {
                if let Err(e) = $libfn(&s) {
                    log::warn!("{}: {e}", $label);
                }
            });
        });
    }};
}

/// Playback-context async wire. The wrapped `library::playback::*`
/// function takes `&PlaybackContext` instead of `&AppState`; we snapshot
/// one inside the spawned future and pass it by reference so the future
/// doesn't hold a borrow across `.await`.
macro_rules! wire_pb {
    ($target:expr, $method:ident, $state:expr, $label:literal, $libfn:path) => {{
        let s = $state.clone();
        $target.$method(move || {
            let s = s.clone();
            s.runtime.clone().spawn(async move {
                let ctx = s.playback_ctx();
                if let Err(e) = $libfn(&ctx).await {
                    log::warn!("{}: {e}", $label);
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
                    log::warn!("{}: {e}", $label);
                }
            });
        });
    }};
}

/// Wire an `on_toggle_row_favorite` / `on_set_row_rating`-shaped callback:
/// collect the track ids with `$collect`, bail on an empty set, hop onto the
/// runtime, run the async `library::*` setter (warn + abort on error), then
/// run the `after` block for the optimistic per-view UI patch. The Slint side
/// passes an `[int]` so a single-row click and a multi-select batch share one
/// code path; `$val` rebinds the callback's second argument (the flag /
/// rating) for use inside `after`.
///
/// `captures:` lists the existing local bindings the `after` block needs —
/// each is cloned once into the callback and once per invocation, matching
/// the hand-rolled shape this replaces. The Search results surface stays
/// hand-rolled on purpose: it is deliberately NON-optimistic (see the
/// comments at its call site), which this macro does not model.
macro_rules! wire_row_flag {
    (
        $target:expr, $method:ident, $state:expr, $label:literal,
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
                    log::warn!("{}: {e}", $label);
                    return;
                }
                let $val = value;
                $after
            });
        });
    }};
}

/// Reset one detail global's hero-Image properties (`cover` plus the dual-slot
/// `blur-img-a` / `blur-img-b`) to `Image::default()` and clear `has-blur`, so
/// the backing `SharedPixelBuffer` Arcs release and `FemtoVG` can reclaim the
/// GPU textures on the next render. `$g` is a Slint detail-global handle
/// (`AlbumDetail`, `ArtistDetail`, `PlaylistDetail`).
///
/// A macro rather than a `fn` for the usual reason: the three are distinct
/// generated types with no trait between them. Reach for
/// [`release_detail_hero_images`] instead unless you are handing back several
/// globals at once and want the two shared resets run once — My Library's
/// deferred hero teardown is the only such caller.
macro_rules! release_hero_slots {
    ($g:expr) => {{
        let detail = &$g;
        detail.set_cover(::slint::Image::default());
        detail.set_blur_img_a(::slint::Image::default());
        detail.set_blur_img_b(::slint::Image::default());
        detail.set_has_blur(false);
    }};
}

/// [`release_hero_slots`] for one detail global, plus the two resets every hero
/// teardown owes. `$ui` is the `AppWindow`.
///
/// Also re-solves the shared `HeroBackdrop` set back to the gradient floor and
/// drops the shared `HeroChips` row. Clearing `has-blur` makes that floor the
/// whole backdrop, and the six heroes share both globals — so without this,
/// backing out of (say) a Genre detail leaves its hash-derived stops painted
/// and its counts spelled out under the *next* hero, for the frames before
/// that one's own decode and fetch land.
macro_rules! release_detail_hero_images {
    ($ui:expr, $g:expr) => {{
        $crate::ui::callbacks::macros::release_hero_slots!($g);
        $crate::ui::hero_backdrop::reset(&$ui);
        $crate::ui::hero_chips::clear(&$ui);
    }};
}

pub(super) use {
    release_detail_hero_images, release_hero_slots, spawn_logged, spawn_logged_sync,
    spawn_logged_toast, wire_pb, wire_row_flag, wire_sync, wire_sync_pb,
};

