//! Wires the `MiniPlayer` global's `active-changed` and `square-changed` to the Up Next
//! subscriber's visibility gate and the square variant's artwork-cache lifecycle.
//!
//! **Resize-only trigger.** The miniplayer engages purely on the window being shrunk
//! past the threshold `app-window.slint` derives; there is no entry or exit button
//! anywhere, and the user grows the window again to leave.
//!
//! Sibling to `window_chrome` rather than under it: that module owns OS-frame concerns —
//! the titlebar, the drag region, the restart flow — where this is a responsive *layout*
//! concern, and mixing the two axes makes neither greppable.

use std::rc::Rc;
use std::sync::Arc;

use slint::ComponentHandle;

use crate::error::AppError;
use crate::state::AppState;
use crate::ui::now_playing::NowPlayingState;
use crate::ui::now_playing_artwork::NowPlayingArtwork;
use crate::{AppWindow, MiniPlayer};

/// Hydrate `np_state.mini_square` from the live global, because `square-changed` only
/// fires on actual flips — a window already square before entering mini would leave the
/// mirror stale and `kick_artwork()` skipped on entry.
fn sync_mini_square(weak: &slint::Weak<AppWindow>, np_state: &NowPlayingState) {
    let Some(ui) = weak.upgrade() else { return };
    np_state.mini_square.set(ui.global::<MiniPlayer>().get_square());
}

/// Off-thread release of the [`NowPlayingArtwork`] LRU plus a `malloc_trim` to hand the
/// pages back, mirroring `wire_now_playing_open`'s: the heavy `(cover, blur)` buffers
/// are pinned only while a surface renders them.
fn release_artwork_off_thread(state: &AppState, np_artwork: &Arc<NowPlayingArtwork>) {
    let np = np_artwork.clone();
    state.runtime.spawn_blocking(move || {
        np.clear();
        crate::services::platform::allocator::trim();
    });
}

/// Wire both callbacks to the Up Next subscriber's `mini_visible` gate and the square
/// variant's artwork-cache lifecycle. Runs on the event-loop thread between
/// `AppWindow::new()` and `app.run()`, the same window as `now_playing` and
/// `window_chrome`.
pub fn install(
    app: &AppWindow,
    state: &AppState,
    np_artwork: &Arc<NowPlayingArtwork>,
    np_state: &Rc<NowPlayingState>,
) -> Result<(), AppError> {
    let mini = app.global::<MiniPlayer>();

    // active-changed: on enter, flip the gates, re-seed Up Next so the square variant
    // doesn't render an empty list, and seed the high-res cover if the entry is
    // directly into square. On exit, release the artwork LRU — nothing in full-UI mode
    // needs those buffers.
    {
        let np_state = np_state.clone();
        let state = state.clone();
        let np_artwork = np_artwork.clone();
        let weak = app.as_weak();
        mini.on_active_changed(move |is_active| {
            np_state.mini_visible.set(is_active);
            if is_active {
                // `square-changed` only fires on actual flips, so a window already
                // square before entry would leave the mirror at its `false` init.
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

    // square-changed: only the square variant renders the high-res cover, the rectangle
    // one taking the row tier's thumb — so seed on the way in, release on the way out
    // rather than leaving the large buffers behind a small tile.
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
