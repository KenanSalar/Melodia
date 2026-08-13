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
/// `async` **and do no file I/O** — the transport calls, whose bodies acquire a
/// `parking_lot::Mutex` and reach Rodio. Spawns onto the runtime so the UI
/// thread isn't blocked.
///
/// A `views.json` / `settings.json` write wants [`spawn_blocking_logged!`]
/// instead; the two are not interchangeable, and this one used to carry both.
macro_rules! spawn_logged_sync {
    ($state:ident, $label:literal, $expr:expr) => {{
        $state.runtime.clone().spawn(async move {
            if let Err(e) = $expr {
                log::warn!("{}: {e}", $label);
            }
        });
    }};
}

/// The persist-and-forget shape: a synchronous `library::settings::*` write on
/// the **blocking** pool, warning on failure.
///
/// Separate from `spawn_logged_sync!` because the pool is the whole difference.
/// These bodies open, rewrite and fsync a JSON file, which is exactly what
/// `spawn_blocking` exists for and exactly what an async worker must not be
/// parked on (`.claude/rules/tokio.md`). Twelve call sites spelled this out for
/// themselves, split by nothing more than which file they were written in — the
/// nine column toggles alone were five on the runtime through
/// `spawn_logged_sync!` and four hand-rolled here on the blocking pool, for one
/// write with one label.
///
/// **The label is a literal, which is the one reason a site legitimately stays
/// hand-rolled**: `Nav.persist-selected-index` interpolates the index it failed
/// to store, and a warning that doesn't name it says almost nothing. The two
/// tab persists are hand-rolled for a second reason — see below.
///
/// Every site writes `views.json`, which is what the `debug` line can call it:
/// column toggles and widths, sort, browse path and view mode leave no other
/// trace, so this is the whole record that a view's own state moved.
/// `AppState::persist_blocking` is the `settings.json` half of the same idea.
macro_rules! spawn_blocking_logged {
    ($state:ident, $label:literal, $expr:expr) => {{
        // Before the spawn, so a write that hangs still says what it was — the
        // `persist_blocking` argument.
        log::debug!("view state: {}", $label);
        // `.clone()` on the runtime handle first, the `spawn_logged_sync!` shape:
        // `$expr` usually moves the state it borrows `runtime` from, and a bare
        // `$state.runtime.spawn_blocking(…)` holds that borrow across the move.
        $state.runtime.clone().spawn_blocking(move || {
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

/// The two shared resets every hero teardown owes, on their own. `$ui` is the
/// `AppWindow`.
///
/// Six heroes share one `HeroBackdrop` solve and one `HeroChips` row, so a
/// teardown that leaves either behind paints the departing hero's colours and
/// counts under the *next* one, for the frames before its own decode and fetch
/// land. [`release_detail_hero_images`] is this plus the image slots, and is
/// what a detail with a cover wants; this bare pair is for the heroes with no
/// images to hand back — Genre Detail, whose tile is a hashed gradient, and the
/// two mosaic pages, whose tiles belong to their own tier. Four sites spelled
/// the pair out, each with its own paragraph saying it was the macro minus the
/// slots.
///
/// **On My Library a section leave is not a teardown, and the two halves stop
/// short of one at different points.**
///
/// The *colour set* belongs to the page rather than to a tab, so it is handed
/// back only once the page itself is left. That is the gate the publish side has
/// always had, read at the level it actually holds at: `apply_detail_artwork`
/// writes the set only while its own section is active, and
/// [`crate::ui::my_library::the_band_is_up`] is the same question on the way out.
/// It subsumes the narrower hand-off case it replaced — switch from Genre Detail
/// to a Playlists tab that already has a detail open and `detail-open` never goes
/// false, so the band deliberately doesn't morph while the departing tab hands
/// the colours back and the entering tab republishes them a query and a decode
/// later — and covers the two the hand-off gate missed: the band *collapsing*
/// over a hero whose colours it is still painting, and a tab re-entered onto a
/// detail whose banner has to come back with it.
///
/// The *chip row* stops one step earlier, at
/// [`crate::ui::hero_chips::clear_if_stale`], and that asymmetry is the point: a
/// colour held across a **hand-off** is the outgoing hero's *tone*, where a count
/// held across it is the outgoing hero's *facts* under the incoming one's title.
/// An empty strip states nothing, which is what a hero with no answer yet should
/// say — but a strip the *incoming* hero has already filled states the right
/// thing, and every cross-tab drill fills it in the tick that moves the tab. So the
/// question is the record's rather than the departing view's, and the leave hands
/// over no tab at all: the two cases where the counts stay put — the band
/// collapsing the departing banner, and Now Playing merely covering a page whose
/// hero comes back underneath — are both readable off the ids.
macro_rules! release_shared_hero {
    ($ui:expr) => {{
        if !$crate::ui::my_library::the_band_is_up(&$ui) {
            $crate::ui::hero_backdrop::reset(&$ui);
        }
        $crate::ui::hero_chips::clear_if_stale(&$ui);
    }};
}

/// [`release_hero_slots`] for one detail global, plus [`release_shared_hero`].
/// `$ui` is the `AppWindow`.
///
/// **The slots ride the same page-level gate the colour set does**, for the
/// reason the detail id gives: nothing on this page clears one on a tab leave, so
/// `AlbumDetail.album-id >= 0` still means "this banner is in the globals" and
/// picking that tab again morphs it straight back open — ahead of the re-fetch
/// the pick kicks. Emptying `cover` here is what left that morph painting
/// `ArtworkImage`'s fallback glyph, and left the 400 ms collapse before it doing
/// the same. Every path that genuinely closes a detail writes `-1` first, and
/// `release_collapsed_hero` hands the slots back on that id once the band has
/// finished shrinking.
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
