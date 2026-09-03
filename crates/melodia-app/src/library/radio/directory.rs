//! What the directory says, and the one thing this install does to the answer.
//!
//! Nothing here writes: a directory answer has a shelf life and becomes a row only when the user
//! keeps or plays it. The thinning below is a second pass over a page the client already filtered
//! — deliberately, because the client's own pass may not bake a *setting* into a `OnceCell` the
//! session reuses. Both halves read `has_more` before they thin, and both say so.

use std::sync::Arc;

use crate::state::AppState;
use melodia_core::entities::radio;
use melodia_core::error::AppError;
use melodia_net::services::net::radio_browser;

use super::directory_client;

/// Search the directory. Results are a network answer with a shelf life and are
/// never written to the table; one becomes a row when the user keeps or plays it.
pub async fn search(
    state: &AppState,
    search: &radio::StationSearch,
) -> Result<radio::StationPage, AppError> {
    let mut page = radio_browser::search(directory_client(state)?, search).await?;
    hide_segmented(&mut page, state.radio_hide_segmented.get());
    Ok(page)
}

/// Drop the segmented stations from a page, if the user has them hidden.
///
/// Here rather than in the request because the endpoint has no `hls` parameter to send. It thins
/// the page without touching [`radio::StationPage::has_more`], which the client already read off
/// the raw response: these rows were served and counted, and paging has to step over them rather
/// than stop at them.
pub(super) fn hide_segmented(page: &mut radio::StationPage, hide: bool) {
    if hide {
        page.stations.retain(|station| !station.hls);
    }
}

/// What the directory currently says about one station, for the station page.
///
/// Deliberately **additive**: the caller keeps whatever the row it opened from
/// already said and takes only the facts the table has no column for — the
/// state, the popularity figures and the directory's own last check. Letting it
/// overwrite the rest would undo a user's `local_*` override from a background
/// fetch, which is the one thing the split columns exist to prevent.
///
/// `Ok(None)` is a uuid the directory no longer lists.
pub async fn station_details(
    state: &AppState,
    station_uuid: &str,
) -> Result<Option<radio::DirectoryStation>, AppError> {
    radio_browser::station_by_uuid(directory_client(state)?, station_uuid).await
}

/// Vote for a station, which is how its popularity ordering stays meaningful.
///
/// No opt-out of its own, unlike the play click: a vote happens only because
/// somebody pressed a button that says so, where the click rides every play and
/// is therefore the one that needs a setting. The master switch still covers it.
pub async fn vote(state: &AppState, station_uuid: &str) -> Result<(), AppError> {
    radio_browser::cast_vote(directory_client(state)?, station_uuid).await
}

/// One of the directory's facet lists, for the filter chips. Large and
/// near-static, so it is fetched once per session and shared thereafter.
pub async fn facets(
    state: &AppState,
    kind: radio::FacetKind,
) -> Result<Arc<[radio::Facet]>, AppError> {
    let facets = radio_browser::facets(directory_client(state)?, kind).await?;
    Ok(hide_segmented_codecs(facets, kind, state.radio_hide_segmented.get()))
}

/// Drop the codecs that only ever name a segmented stream, if the user has those hidden.
///
/// [`hide_segmented`]'s counterpart on the chip. The directory counts every station its checker
/// saw, so a Format list built from those counts otherwise offers filters whose entire result the
/// page thins away: `UNKNOWN` is what the checker writes when it could not read a playlist at all,
/// and a comma means it found a picture track beside the audio.
///
/// Filtered here rather than in `radio_browser`, whose cell holds one list per session and must
/// not bake a setting into it, and the input is handed back untouched for every other kind: the
/// tag list runs to tens of thousands of entries and this is called on every chip open.
pub(super) fn hide_segmented_codecs(
    facets: Arc<[radio::Facet]>,
    kind: radio::FacetKind,
    hide: bool,
) -> Arc<[radio::Facet]> {
    if !hide || kind != radio::FacetKind::Codecs {
        return facets;
    }
    facets.iter().filter(|facet| !names_segmented(&facet.name)).cloned().collect()
}

/// Codec names the directory only ever writes for a stream nothing can play as one continuous
/// mount. `MP4` is spelled out because it is a container rather than a codec and every station
/// under it is flagged segmented.
pub(super) fn names_segmented(codec: &str) -> bool {
    codec.contains(',')
        || codec.eq_ignore_ascii_case(radio::UNKNOWN_CODEC)
        || codec.eq_ignore_ascii_case("MP4")
}
