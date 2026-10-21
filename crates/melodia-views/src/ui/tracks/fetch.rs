//! The Songs list as a [`PagedTrackList`]: the query its loads run under, and the library size its
//! header states beside the rows.

use std::sync::Arc;

use slint::Weak;

use super::TracksUi;
use crate::ui::callbacks::macros::spawn_logged;
use crate::ui::paged_rows::track_rows::{self, ListLoads, ListQuery, PagedTrackList};
use crate::ui::paged_rows::unfiltered_total;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_artwork::media::image::cover_thumbs::CoverThumbs;
use melodia_core::entities::track_list::{SortDir, TrackListScope, TrackOrder};
use melodia_core::error::AppResult;
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::Tracks;

/// The sort and the filter a load runs under, as the `Tracks` global spells them.
pub struct SongsQuery {
    order: TrackOrder,
    needle: Needle,
}

impl SongsQuery {
    pub fn new(sort_field: &str, sort_dir: &str, filter: &str) -> Self {
        Self {
            order: TrackOrder::new(sort_field, SortDir::from_token(sort_dir)),
            needle: fold_needle(filter),
        }
    }
}

impl PagedTrackList for TracksUi {
    type Global = Tracks<'static>;
    /// The library's size, which the header states whatever the filter leaves.
    type Facts = i32;

    const LABEL: &'static str = "Tracks.rows";

    fn loads(&self) -> &ListLoads {
        &self.loads
    }

    fn global(&self) -> &Weak<Tracks<'static>> {
        &self.global
    }

    fn row_covers(&self) -> &Arc<CoverThumbs> {
        &self.cover_thumbs
    }

    fn query(&self) -> Option<ListQuery> {
        let query = self.query.lock();
        Some(ListQuery {
            scope: TrackListScope::Library,
            order: query.order,
            needle: query.needle.clone(),
        })
    }

    async fn facts(state: &AppState, query: &ListQuery, ids: &[i64]) -> AppResult<i32> {
        unfiltered_total(ids, &query.needle, library::tracks::count(&state.db)).await
    }

    fn shown(&self, g: &Tracks<'static>, _: &ListQuery, total: Option<i32>) {
        if let Some(total) = total {
            g.set_total_count(total);
        }
    }
}

/// Reload the list under `query`: its ids, the rows it opens on with their covers warm, and every
/// row it still holds read again. What a launch, a re-enter and a library tick ask for.
pub fn refresh(state: &AppState, tracks_ui: &Arc<TracksUi>, query: SongsQuery) {
    *tracks_ui.query.lock() = query;
    let s = state.clone();
    let tu = Arc::clone(tracks_ui);
    spawn_logged!(s, "tracks::refresh", track_rows::reload(&s, &tu));
}

/// Load the ids under a new sort or filter, the count left as it is
/// ([`track_rows::requery`]).
pub fn requery(state: &AppState, tracks_ui: &Arc<TracksUi>, query: SongsQuery) {
    *tracks_ui.query.lock() = query;
    track_rows::requery(state, tracks_ui);
}
