//! Stations projected onto the Slint boundary structs.
//!
//! The tree's sixteen other `to_slint_*` converters live beside the view that fills the model,
//! and these are no different — what is different is the third input on the directory's side. A
//! browsed card's star and its logo are not facts about the station, they are facts about *this
//! install*, so both arrive as arguments rather than being looked up here: a converter that
//! reached for either would need the handle, and the caller already holds both while it walks the
//! page. A kept station carries them as columns and so needs neither.

use slint::{Model, SharedString, VecModel};

use crate::entities::radio::{DirectoryStation, Facet, RadioStation};
use crate::{RadioFacetRow, RadioStationGridRow, RadioStationRow};

use super::identity;

/// How many of a station's tags a card shows.
///
/// The directory's tag field is free-form and user-entered, so a popular station routinely
/// carries a dozen, most of them restatements of the first two. Three is what fits the card's
/// meta line at the narrowest column count without eliding.
const TAG_DISPLAY_LIMIT: usize = 3;

/// Separator between tags on the card's meta line. A middot rather than a comma, so the line
/// reads as a set of labels and not as prose.
const TAG_SEPARATOR: &str = " · ";

/// Whether a station id names a database row.
///
/// **The one place `id == 0` is given its meaning**, and it is asked from four directions: which
/// cache a page resolves from, whether a removal has anything to remove, whether `views.json` can
/// name the open page for the next launch, and whether a history walk can reopen it. A browsed
/// station is a directory answer with a shelf life and no row until the user keeps or plays it.
pub fn station_has_row(id: i64) -> bool {
    id != 0
}

/// Write `cards` onto the delegates a grid already has mounted, or report that it cannot.
///
/// **A model reset is what this exists to avoid**, and it costs more than a repaint. `write_grid`
/// is a `set_vec`: every delegate is torn down and rebuilt, and a rebuilt card carries no pointer
/// state until the next mouse *event*. So a click that repaints the grid it was aimed at leaves
/// the card still under the cursor drawn as though it had never been hovered — the star, the play
/// control and the fill all drop out with nothing having moved — and a delegate destroyed
/// mid-press takes the grab with it, so the click never lands at all. Both are reachable from an
/// ordinary card click: playing or starring a station refetches the kept lists, and the logo
/// sweep repaints on a timer for as long as a page takes to fill.
///
/// Returns `false` when the grid is not the same stations in the same places, which is a caller
/// owing the full write instead — there is nothing to reuse, so the reset is the honest answer.
/// It may have written some cards before deciding that; the full write behind it covers them.
///
/// UI thread only.
pub fn patch_grid(
    grid: &slint::ModelRc<RadioStationGridRow>,
    cards: &[RadioStationRow],
    columns: i32,
) -> bool {
    // The chunking the caller would have built, so a column change is a shape change rather than
    // a rechunk this silently skips: the same stations in the same order fill different rows at
    // different column counts, and every position would still agree.
    let cols = usize::try_from(columns.max(1)).unwrap_or(1);
    let Some(chunks) = grid.as_any().downcast_ref::<VecModel<RadioStationGridRow>>() else {
        return false;
    };
    if chunks.row_count() != cards.len().div_ceil(cols) {
        return false;
    }

    for (chunk, wanted) in cards.chunks(cols).enumerate() {
        let Some(row) = chunks.row_data(chunk) else {
            return false;
        };
        let Some(mounted) = row.stations.as_any().downcast_ref::<VecModel<RadioStationRow>>()
        else {
            return false;
        };
        if mounted.row_count() != wanted.len() {
            return false;
        }
        for (slot, card) in wanted.iter().enumerate() {
            let Some(current) = mounted.row_data(slot) else {
                return false;
            };
            if !same_station(&current, card) {
                return false;
            }
            // Value-compared, so an unchanged card notifies nothing: the common repaint is a
            // refetch that moved one row, and the rest of the page owes no work for it.
            if current != *card {
                mounted.set_row_data(slot, card.clone());
            }
        }
    }
    true
}

/// Whether two rows name the same station.
///
/// Both halves, for [`super::detail::StationRef`]'s reason: a browsed row's `id` is always `0`,
/// and a hand-typed one carries no `uuid`.
fn same_station(a: &RadioStationRow, b: &RadioStationRow) -> bool {
    a.id == b.id && a.uuid == b.uuid
}

/// One browsed station, with this install's answers about it folded in.
///
/// `id` stays `0`: a directory station has no row until the user keeps or plays it, and that zero
/// is what every call site taking a whole row branches on.
pub fn to_slint_radio_station_row(
    station: &DirectoryStation,
    is_favorite: bool,
    logo: Option<&str>,
) -> RadioStationRow {
    let tile = identity::station_tile(&station.name);
    RadioStationRow {
        id: 0,
        uuid: SharedString::from(&station.station_uuid),
        name: SharedString::from(&station.name),
        homepage: station.homepage.as_deref().map(SharedString::from).unwrap_or_default(),
        // Browse mounts the grid with `manageable: false`, so no card here draws a pencil and the
        // question never arises; the kept tabs are where a station is edited.
        editable: false,
        artwork_path: logo.map(SharedString::from).unwrap_or_default(),
        tags: SharedString::from(display_tags(&station.tags)),
        country: SharedString::from(&station.country),
        codec: SharedString::from(&station.codec),
        bitrate: station.bitrate,
        hls: station.hls,
        is_favorite,
        // Nothing has been played from the directory: a play writes the row first.
        play_count: 0,
        tile_color_1: tile.color_1,
        tile_color_2: tile.color_2,
        monogram: tile.monogram,
    }
}

/// One kept station, as the two local tabs draw it.
///
/// Reads nothing but the row: the star and the logo are columns here, where a browsed station has
/// to be told both. `uuid` stays empty for a station the user typed in, which is what the card
/// splits its star and its Edit control on.
pub fn to_slint_kept_station_row(station: &RadioStation) -> RadioStationRow {
    let tile = identity::station_tile(&station.name);
    RadioStationRow {
        id: crate::ui::util::clamp_i64_to_i32(station.id),
        uuid: station.station_uuid.as_deref().map(SharedString::from).unwrap_or_default(),
        name: SharedString::from(&station.name),
        homepage: station.website().map(SharedString::from).unwrap_or_default(),
        editable: station.is_editable(),
        artwork_path: station.artwork_path.as_deref().map(SharedString::from).unwrap_or_default(),
        tags: SharedString::from(display_tags(station.genre().unwrap_or_default())),
        country: station.country_name().map(SharedString::from).unwrap_or_default(),
        codec: SharedString::from(&station.codec),
        bitrate: station.bitrate,
        hls: station.hls,
        is_favorite: station.is_favorite,
        play_count: station.play_count,
        tile_color_1: tile.color_1,
        tile_color_2: tile.color_2,
        monogram: tile.monogram,
    }
}

/// One facet-list entry behind a filter chip.
pub fn to_slint_facet_row(facet: &Facet) -> RadioFacetRow {
    RadioFacetRow {
        name: SharedString::from(&facet.name),
        code: facet.code.as_deref().map(SharedString::from).unwrap_or_default(),
        station_count: i32::try_from(facet.station_count).unwrap_or(i32::MAX),
    }
}

/// The directory's tag field as the separate labels a chip strip wants, under the same cap the
/// card's meta line takes. The hero is the one surface that can draw them as individual chips —
/// it is mounted once, where a card is a delegate inside a virtualized list.
pub fn split_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .take(TAG_DISPLAY_LIMIT)
        .map(str::to_owned)
        .collect()
}

/// The directory's comma-separated tag field as one display line.
///
/// Joined here rather than handed over as a list, because a card cannot split a string and a
/// per-card chip strip inside a virtualized `ListView` would put a `changed` tracker in a
/// delegate the virtualization destroys.
fn display_tags(raw: &str) -> String {
    let mut out = String::new();
    for tag in raw.split(',').map(str::trim).filter(|tag| !tag.is_empty()).take(TAG_DISPLAY_LIMIT) {
        if !out.is_empty() {
            out.push_str(TAG_SEPARATOR);
        }
        out.push_str(tag);
    }
    out
}

#[cfg(test)]
#[path = "tests/rows_tests.rs"]
mod tests;
