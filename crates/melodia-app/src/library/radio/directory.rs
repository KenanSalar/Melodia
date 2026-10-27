//! What the directory says, and the one thing this install does to the answer.
//!
//! Nothing here writes: a directory answer becomes a row only when the user keeps or plays it.
//! The segmented-station thinning runs here rather than in the client, whose session cache may not
//! bake in a setting.

use std::sync::Arc;

use crate::state::AppState;
use melodia_core::entities::radio;
use melodia_core::error::AppError;
use melodia_net::services::net::radio_browser;

use super::directory_client;

pub async fn search(
    state: &AppState,
    search: &radio::StationSearch,
) -> Result<radio::StationPage, AppError> {
    let mut page = radio_browser::search(directory_client(state)?, search).await?;
    hide_segmented(&mut page, state.radio_hide_segmented.get());
    Ok(page)
}

/// Drop the segmented stations from a page, if the user has them hidden, the endpoint taking no
/// `hls` parameter. [`radio::StationPage::has_more`] stays as the client read it: these rows were
/// served, and paging steps over them rather than stopping at them.
pub(super) fn hide_segmented(page: &mut radio::StationPage, hide: bool) {
    if hide {
        page.stations.retain(|station| !station.hls);
    }
}

/// What the directory says about one station now, or `Ok(None)` for a uuid it no longer lists.
/// **Additive**: the caller takes only facts the table has no column for, or a background fetch
/// would undo a user's `local_*` override.
pub async fn station_details(
    state: &AppState,
    station_uuid: &str,
) -> Result<Option<radio::DirectoryStation>, AppError> {
    radio_browser::station_by_uuid(directory_client(state)?, station_uuid).await
}

/// Vote for a station. No opt-out of its own, unlike the play click: a vote only ever follows a
/// button that says so, where the click rides every play.
pub async fn vote(state: &AppState, station_uuid: &str) -> Result<(), AppError> {
    radio_browser::cast_vote(directory_client(state)?, station_uuid).await
}

/// One of the directory's facet lists, for the filter chips, fetched once per session.
pub async fn facets(
    state: &AppState,
    kind: radio::FacetKind,
) -> Result<Arc<[radio::Facet]>, AppError> {
    let facets = radio_browser::facets(directory_client(state)?, kind).await?;
    Ok(hide_segmented_codecs(facets, kind, state.radio_hide_segmented.get()))
}

/// [`hide_segmented`]'s counterpart on the Format chip, which would otherwise offer filters whose
/// whole result the page thins away. Every other kind is handed back untouched, the tag list
/// running to tens of thousands of entries on every chip open.
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

/// Codec names the directory only writes for a segmented stream: `UNKNOWN` where its checker could
/// not read a playlist, a comma where it found a picture track beside the audio, and `MP4`, a
/// container whose every station is flagged segmented.
pub(super) fn names_segmented(codec: &str) -> bool {
    codec.contains(',')
        || codec.eq_ignore_ascii_case(radio::UNKNOWN_CODEC)
        || codec.eq_ignore_ascii_case("MP4")
}
