//! Most Played strip fetch + apply. A single horizontal carousel of the
//! library-wide top tracks by `play_count`. Honours the list's search filter
//! (title + artist) so the strip narrows in lockstep with the tracklist.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::{RecentlyPlayedUi, to_slint_most_played_row};
use crate::library;
use crate::state::AppState;
use crate::ui::row_match::most_played_matches;
use crate::{AppWindow, EntityStripRow as UiEntityStripRow, RecentlyPlayed};

/// Cap for the Most Played strip — enough to fill a horizontal scroll
/// comfortably without inflating the SQL projection. A strip walks its rows
/// in a plain `for`, so the cap is what keeps it affordable; the Favorites
/// tab that used to share this number is a virtualized grid now and fetches
/// uncapped.
const MOST_PLAYED_LIMIT: i64 = 10;

/// Fetch the Most Played strip and apply it (filtered). Returns `()` — the
/// caller (`kick_full_refresh`) has no use for a propagated error; failures
/// are logged here with context.
pub async fn refresh_strips(
    state: &AppState,
    rp_ui: &Arc<RecentlyPlayedUi>,
    weak: &Weak<AppWindow>,
) {
    let most_played = library::recently_played::get_most_played(state, MOST_PLAYED_LIMIT)
        .await
        .inspect_err(|e| log::warn!("recently_played::refresh_strips most_played: {e}"))
        .ok();

    // A leave that landed while the query was in flight has already cleared
    // this cache and emptied the strip's model, so everything below — the
    // store, the cover prewarm and the apply — would undo that teardown behind
    // a view nobody can see. Nothing is lost by dropping the result: the leave
    // set `mark_dirty`, so the next enter re-fetches. The gate below does *not*
    // cover this: it serializes the two writes, it does not order them, so a
    // store landing wholly after the wipe passes it cleanly.
    if !rp_ui.section_active() {
        return;
    }

    if let Some(rows) = most_played {
        // Under the section gate so the store can't interleave with
        // `release_section_state`'s wipe and leave half of each on screen, the
        // way `favorites::grids::fetch::refresh_grids` does it. Held across the
        // synchronous store only — never across an `.await`.
        let _gate = rp_ui.gate();
        *rp_ui.state().most_played.lock() = rows;
    }

    // Prewarm the strip tier off-thread before the rows land in the model —
    // the cards' `request-most-played-cover` lookups decode on miss on the UI
    // thread. `capacity()` locks the tier, so read it before taking the
    // cache's lock rather than inside — nothing today can deadlock the pair,
    // but there's no reason to hold both.
    let cap = rp_ui.most_played_thumbs.capacity();
    let covers: Vec<PathBuf> = {
        let most_played = rp_ui.state().most_played.lock();
        crate::ui::grid_prewarm::unique_artwork_paths(
            most_played.iter().map(|t| t.artwork_path.as_deref()),
            cap,
        )
    };
    if !covers.is_empty() {
        let thumbs = rp_ui.most_played_thumbs.clone();
        let ru = rp_ui.clone();
        let _ = tokio::task::spawn_blocking(move || {
            thumbs.prewarm(&covers);
            // Hand the buffers straight back when the section was left while
            // the decode ran — `release_section_state` is spawned on the same
            // pool by that leave and often wins, so without this the tier it
            // emptied comes back populated behind a view nobody can see. The
            // check has to sit *after* the decode, which is the whole point;
            // `FavoritesUi::prewarm_tab_covers` carries the same one. The
            // shared row tier `refresh_tracks` warms is deliberately exempt:
            // every other list draws from it, and the leave doesn't clear it.
            if !ru.section_active() {
                thumbs.clear();
            }
        })
        .await;
    }

    apply_filtered_strips(rp_ui, weak);
}

/// Re-walk the cached `most_played` Vec through the current filter and push the
/// strip rows. Cheap — runs entirely in memory. Empty filter ⇒ all rows;
/// non-empty ⇒ the shared [`most_played_matches`] walk, the same one the
/// recency list beside it runs.
///
/// A hidden section is never written to, the way
/// `favorites::grids::apply::write_filtered_grids` refuses to: the leave
/// teardown empties this model deliberately, and the check has to sit *inside*
/// the closure because the leave can land while the post is in flight.
pub fn apply_filtered_strips(rp_ui: &Arc<RecentlyPlayedUi>, weak: &Weak<AppWindow>) {
    let needle = rp_ui.state().filter.lock().clone();

    let rows: Vec<UiEntityStripRow> = {
        let cache = rp_ui.state().most_played.lock();
        cache
            .iter()
            .filter(|t| most_played_matches(t, &needle))
            .map(to_slint_most_played_row)
            .collect()
    };

    let rp_ui = rp_ui.clone();
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        if !rp_ui.section_active() {
            return;
        }
        let g = ui.global::<RecentlyPlayed>();
        let model = g.get_most_played_rows();
        let Some(vec) = model.as_any().downcast_ref::<VecModel<UiEntityStripRow>>() else {
            log::warn!("RecentlyPlayed.most-played-rows: VecModel<EntityStripRow> downcast failed");
            return;
        };
        vec.set_vec(rows);
    });
}
