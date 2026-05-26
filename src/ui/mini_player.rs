//! Miniplayer module — wires the `MiniPlayer` Slint global's
//! `active-changed` and `square-changed` callbacks to the Up Next
//! subscriber's visibility gate and the square-variant artwork cache
//! lifecycle.
//!
//! ## Resize-only trigger
//!
//! Tauri parity: the miniplayer is engaged purely by the OS window being
//! shrunk past the threshold derived inside `ui/app-window.slint`
//! (`mini-active: self.width < 550px || self.height < 250px`). There is
//! no entry or exit button anywhere in the UI — the user grows the
//! window past the threshold again to leave (or double-clicks the bare
//! drag region to maximise, which also exits mini).
//!
//! ## Why not a module under `window_chrome`
//!
//! `window_chrome` owns OS-frame concerns (the custom titlebar, drag
//! region, restart flow, minimise/maximise). The miniplayer is a
//! responsive *layout* concern; keeping it sibling-level makes the
//! dataflow greppable and avoids mixing orthogonal axes.

use std::rc::Rc;
use std::sync::Arc;

use slint::ComponentHandle;

use crate::error::AppError;
use crate::state::AppState;
use crate::ui::now_playing::NowPlayingState;
use crate::ui::now_playing_artwork::NowPlayingArtwork;
use crate::{AppWindow, MiniPlayer};

/// Hydrate `np_state.mini_square` from the live `MiniPlayer.square`
/// global. Necessary because `square-changed` only fires on actual
/// flips — a window already in a square aspect before entering mini
/// would leave the Rust mirror stale, and `kick_artwork()` would be
/// skipped on entry. Called from `on_active_changed` on entry.
fn sync_mini_square(weak: &slint::Weak<AppWindow>, np_state: &NowPlayingState) {
    let Some(ui) = weak.upgrade() else { return };
    np_state
        .mini_square
        .set(ui.global::<MiniPlayer>().get_square());
}

/// Off-thread release of the [`NowPlayingArtwork`] LRU + a glibc
/// `malloc_trim` to hand the freed pages back to the kernel. Mirrors the
/// release path in `wire_now_playing_open` (see
/// `src/ui/now_playing/up_next.rs`) — the heavy `(cover, blur)` buffers
/// are pinned only while a surface (the full Now Playing view *or* the
/// square miniplayer) renders them, and freeing on the transition keeps
/// RSS in line with the rest of the app's memory discipline.
fn release_artwork_off_thread(state: &AppState, np_artwork: &Arc<NowPlayingArtwork>) {
    let np = np_artwork.clone();
    state.runtime.spawn_blocking(move || {
        np.clear();
        crate::tasks::heap_trim::trim();
    });
}

/// Wire `MiniPlayer.{active-changed, square-changed}` to the Up Next
/// subscriber's `mini_visible` gate and the square-variant artwork
/// cache lifecycle. Runs on the Slint event-loop thread, between
/// `AppWindow::new()` and `app.run()` — same install window as the
/// `now_playing` / `window_chrome` modules.
pub fn install(
    app: &AppWindow,
    state: &AppState,
    np_artwork: &Arc<NowPlayingArtwork>,
    np_state: &Rc<NowPlayingState>,
) -> Result<(), AppError> {
    let mini = app.global::<MiniPlayer>();

    // active-changed: enter/leave mini state. On enter, flip the gates,
    // re-seed the Up Next list (so the square variant doesn't render an
    // empty list when the queue hasn't changed since the subscriber
    // started stashing), and — when entering directly into the square
    // variant — seed the high-res cover. On exit, release the artwork
    // LRU + trim glibc: neither the Now Playing view nor any miniplayer
    // surface needs the 384 px buffers in full-UI mode.
    {
        let np_state = np_state.clone();
        let state = state.clone();
        let np_artwork = np_artwork.clone();
        let weak = app.as_weak();
        mini.on_active_changed(move |is_active| {
            np_state.mini_visible.set(is_active);
            if is_active {
                // Hydrate from the Slint global — `square-changed` only
                // fires on actual flips, so a tall-aspect window already
                // in `MiniPlayer.square = true` before entry would leave
                // the Rust mirror at its `Cell::new(false)` init.
                sync_mini_square(&weak, &np_state);
                np_state.kick_up_next();
                if np_state.mini_square.get() {
                    np_state.kick_artwork();
                }
            } else {
                release_artwork_off_thread(&state, &np_artwork);
            }
        });
    }

    // square-changed: rectangle ↔ square flip while mini-active. The
    // square variant renders the high-res cover; the rectangle variant
    // uses the 48 px row-tier thumb. On rectangle→square seed the cover;
    // on square→rectangle release the cache so the 384 px buffers
    // don't linger while the only visible tile is 48 px.
    {
        let np_state = np_state.clone();
        let state = state.clone();
        let np_artwork = np_artwork.clone();
        mini.on_square_changed(move |is_square| {
            np_state.mini_square.set(is_square);
            if !np_state.mini_visible.get() {
                return;
            }
            if is_square {
                np_state.kick_artwork();
            } else {
                release_artwork_off_thread(&state, &np_artwork);
            }
        });
    }

    Ok(())
}
