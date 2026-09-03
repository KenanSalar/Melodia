//! Shared spawn / wire macros for every per-view `wire_*` module. Re-exported at the
//! bottom; sibling files bring them in with `use super::macros::*;`.

/// Spawn `$fut` on `$state`'s runtime, logging any error with `$label`. The caller must
/// already have a local binding `$state` (typically `let s = s.clone();` at closure entry)
/// so `$fut`'s `&s` token resolves to the moved-in clone.
macro_rules! spawn_logged {
    ($state:ident, $label:literal, $fut:expr) => {{
        $state.runtime.clone().spawn(async move {
            if let Err(e) = $fut.await {
                log::warn!("{}: {e}", $label);
            }
        });
    }};
}

/// [`spawn_logged!`] plus an error toast through the `utils::toast` bridge. Reserve it
/// for user-initiated operations whose silent failure is confusing — a folder scan or
/// import that appears to do nothing; routine failures keep [`spawn_logged!`].
macro_rules! spawn_logged_toast {
    ($state:ident, $label:literal, $fut:expr) => {{
        $state.runtime.clone().spawn(async move {
            if let Err(e) = $fut.await {
                log::warn!("{}: {e}", $label);
                melodia_core::utils::toast::notify(
                    melodia_core::utils::toast::ToastKind::OperationFailed,
                    e.to_string(),
                );
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
                log::warn!("{}: {e}", $label);
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
        // `.clone()` first: `$expr` usually moves the state it borrows `runtime` from,
        // and a bare `$state.runtime.spawn_blocking(…)` holds that borrow across the move.
        $state.runtime.clone().spawn_blocking(move || {
            if let Err(e) = $expr {
                log::warn!("{}: {e}", $label);
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
                    log::warn!("{}: {e}", $label);
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

/// Wire an `on_toggle_row_favorite` / `on_set_row_rating`-shaped callback: collect ids,
/// bail on an empty set, hop onto the runtime, run the async setter, then run `after` for
/// the optimistic per-view patch. The Slint side passes an `[int]`, so a single-row click
/// and a multi-select batch share one path.
///
/// `captures:` lists the local bindings `after` needs, each cloned once into the callback
/// and once per invocation. Search stays hand-rolled, being deliberately non-optimistic —
/// a shape this doesn't model.
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

/// Reset one detail global's hero-Image properties and clear `has-blur`, so the backing
/// `SharedPixelBuffer` Arcs release and `FemtoVG` can reclaim the GPU textures. A macro
/// rather than a `fn` because the three detail globals are distinct generated types with
/// no trait between them.
///
/// Reach for [`release_detail_hero_images`] unless you are handing back several globals at
/// once and want the two shared resets run once — My Library's deferred hero teardown is
/// the only such caller.
macro_rules! release_hero_slots {
    ($g:expr) => {{
        let detail = &$g;
        detail.set_cover(::slint::Image::default());
        detail.set_blur_img_a(::slint::Image::default());
        detail.set_blur_img_b(::slint::Image::default());
        detail.set_has_blur(false);
    }};
}

/// The two shared resets every hero teardown owes, on their own.
///
/// Six heroes share one `HeroBackdrop` solve and one `HeroChips` row, so a teardown
/// leaving either behind paints the departing hero's colours and counts under the *next*
/// one. [`release_detail_hero_images`] is this plus the image slots; this bare pair is for
/// the heroes with none to hand back — Genre Detail's hashed gradient, and the two mosaic
/// pages whose tiles belong to their own tier.
///
/// **On My Library a section leave is not a teardown, and the two halves stop short of one
/// at different points.** The *colour set* belongs to the page rather than a tab, so
/// [`crate::ui::my_library::the_band_is_up`] holds it until the page itself is left — the
/// same question `apply_detail_artwork` asks on the way in, covering the two cases a
/// hand-off gate missed: the band *collapsing* over a hero whose colours it is still
/// painting, and a tab re-entered onto a detail whose banner comes back with it.
///
/// The *chip row* stops one step earlier, at [`crate::ui::hero_chips::clear_if_stale`],
/// and the asymmetry is the point: a colour held across a hand-off is the outgoing hero's
/// *tone*, where a count held across it is its *facts* under the incoming title.
macro_rules! release_shared_hero {
    ($ui:expr) => {{
        if !$crate::ui::my_library::the_band_is_up(&$ui) {
            $crate::ui::hero_backdrop::reset(&$ui);
        }
        $crate::ui::hero_chips::clear_if_stale(&$ui);
    }};
}

/// [`release_hero_slots`] for one detail global, plus [`release_shared_hero`].
///
/// **The slots ride the same page-level gate the colour set does**, for the reason the
/// detail id gives: nothing clears one on a tab leave, so `AlbumDetail.album-id >= 0`
/// still means "this banner is in the globals" and picking that tab again morphs it back
/// open ahead of the re-fetch — emptying `cover` here leaves that morph painting
/// `ArtworkImage`'s fallback glyph. Every path that genuinely closes a detail writes `-1`
/// first, and `release_collapsed_hero` hands the slots back once the band has shrunk.
macro_rules! release_detail_hero_images {
    ($ui:expr, $g:expr) => {{
        if !$crate::ui::my_library::the_band_is_up(&$ui) {
            $crate::ui::callbacks::macros::release_hero_slots!($g);
        }
        $crate::ui::callbacks::macros::release_shared_hero!($ui);
    }};
}

pub(in crate::ui) use {
    release_detail_hero_images, release_hero_slots, release_shared_hero, spawn_blocking_logged,
    spawn_logged, spawn_logged_sync, spawn_logged_toast, wire_pb, wire_row_flag, wire_sync,
    wire_sync_pb,
};
