//! Most Played strip fetch + apply. A single horizontal carousel of the
//! library-wide top tracks by `play_count`. Honours the list's search filter
//! (title + artist) so the strip narrows in lockstep with the tracklist.

use std::path::PathBuf;
use std::sync::Arc;

use slint::{ComponentHandle, Model, VecModel, Weak};

use super::{RecentlyPlayedUi, to_slint_most_played_row};
use crate::library;
use crate::state::AppState;
use crate::ui::detail_filter::most_played_matches;
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
    match library::recently_played::get_most_played(state, MOST_PLAYED_LIMIT).await {
        Ok(most_played) => {
            *rp_ui.state().most_played.lock() = most_played;
        }
        Err(e) => log::warn!("recently_played::refresh_strips most_played: {e}"),
    }

    // Prewarm the strip tier off-thread before the rows land in the model —
    // the cards' `request-most-played-cover` lookups decode on miss on the UI
    // thread. `prewarm` dedupes its input.
    let covers: Vec<PathBuf> = rp_ui
        .state()
        .most_played
        .lock()
        .iter()
        .filter_map(|t| t.artwork_path.as_deref().map(PathBuf::from))
        .collect();
    if !covers.is_empty() {
        let thumbs = rp_ui.most_played_thumbs.clone();
        let _ = tokio::task::spawn_blocking(move || thumbs.prewarm(&covers)).await;
    }

    apply_filtered_strips(rp_ui, weak);
}

/// Re-walk the cached `most_played` Vec through the current filter and push the
/// strip rows. Cheap — runs entirely in memory. Empty filter ⇒ all rows;
/// non-empty ⇒ case-insensitive substring match on title + artist.
pub fn apply_filtered_strips(rp_ui: &Arc<RecentlyPlayedUi>, weak: &Weak<AppWindow>) {
    let needle = rp_ui.state().filter.lock().to_lowercase();

    let rows: Vec<UiEntityStripRow> = {
        let cache = rp_ui.state().most_played.lock();
        cache
            .iter()
            .filter(|t| most_played_matches(t, &needle))
            .map(to_slint_most_played_row)
            .collect()
    };

    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(ui) = weak.upgrade() else { return };
        let g = ui.global::<RecentlyPlayed>();
        let model = g.get_most_played_rows();
        let Some(vec) = model.as_any().downcast_ref::<VecModel<UiEntityStripRow>>() else {
            log::warn!("RecentlyPlayed.most-played-rows: VecModel<EntityStripRow> downcast failed");
            return;
        };
        vec.set_vec(rows);
    });
}
