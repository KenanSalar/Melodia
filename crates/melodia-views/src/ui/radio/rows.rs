//! Stations projected onto the Slint boundary structs.
//!
//! A browsed card's star, logo and measured format are facts about this install rather than the
//! station, so they arrive as [`LocalAnswers`] from a caller already holding the handle; a kept
//! station carries all three as columns.

use slint::{Model, SharedString, VecModel};

use melodia_core::entities::radio::{DirectoryStation, Facet, RadioStation, UNKNOWN_CODEC};
use melodia_ui::{RadioFacetRow, RadioStationGridRow, RadioStationRow};

use super::{facets, identity};
use crate::ui::util::opt_shared;

/// How many of a station's free-form tags a card shows: what fits the meta line at the narrowest
/// column count, the tags past it mostly restating the first two.
const TAG_DISPLAY_LIMIT: usize = 3;

/// A middot rather than a comma, so the meta line reads as labels rather than prose.
const TAG_SEPARATOR: &str = " · ";

/// Whether a station id names a database row: the one place `id == 0` is given its meaning, a
/// browsed station having no row until the user keeps or plays it.
pub fn station_has_row(id: i64) -> bool {
    id != 0
}

/// Writes `cards` onto the delegates a grid has mounted, or returns `false` where the grid isn't
/// the same stations in the same places and the caller owes the full write.
///
/// A reset rebuilds every delegate, and a rebuilt card has no pointer state until the next mouse
/// event: the card under the cursor drops its hover, and one destroyed mid-press loses the click.
/// Playing or starring a station and the logo sweep's repaints all reach it. Cards written before
/// a `false` are covered by the full write.
pub fn patch_grid(
    grid: &slint::ModelRc<RadioStationGridRow>,
    cards: &[RadioStationRow],
    columns: i32,
) -> bool {
    // The caller's chunking, so a column change is a shape change rather than a skipped rechunk.
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
            // Value-compared, so an unchanged card notifies nothing.
            if current != *card {
                mounted.set_row_data(slot, card.clone());
            }
        }
    }
    true
}

/// Whether two rows name the same station: both halves, as in [`super::detail::StationRef`].
fn same_station(a: &RadioStationRow, b: &RadioStationRow) -> bool {
    a.id == b.id && a.uuid == b.uuid
}

/// What this install knows about a browsed station that the directory's answer doesn't carry,
/// unset for one it has never met. A struct because two of the fields are optional strings that
/// only their order would tell apart.
#[derive(Default)]
pub struct LocalAnswers {
    pub is_favorite: bool,
    pub logo: Option<String>,
    pub format: Option<String>,
}

/// One browsed station with those answers folded in. `id` stays `0`, and `select_key` too, the
/// Browse grid being the one that knows the station's place.
pub fn to_slint_radio_station_row(
    station: &DirectoryStation,
    local: &LocalAnswers,
) -> RadioStationRow {
    let (logo, format) = (local.logo.as_deref(), local.format.as_deref());
    let tile = identity::station_tile(&station.name);
    RadioStationRow {
        id: 0,
        select_key: 0,
        uuid: SharedString::from(&station.station_uuid),
        name: SharedString::from(&station.name),
        homepage: opt_shared(station.homepage.as_deref()),
        // Browse mounts its grid with `manageable: false`; the kept tabs are where one is edited.
        editable: false,
        artwork_path: opt_shared(logo),
        tags: SharedString::from(display_tags(&station.tags)),
        country: SharedString::from(&station.country),
        codec: SharedString::from(display_codec(format, &station.codec, station.hls)),
        bitrate: station.bitrate,
        is_favorite: local.is_favorite,
        // Nothing has been played from the directory: a play writes the row first.
        play_count: 0,
        tile_color_1: tile.color_1,
        tile_color_2: tile.color_2,
        monogram: tile.monogram,
    }
}

/// One kept station as the local tabs draw it, off the row alone. `uuid` stays empty for a
/// hand-typed station, which is what the card splits its star and Edit control on.
pub fn to_slint_kept_station_row(station: &RadioStation) -> RadioStationRow {
    let tile = identity::station_tile(&station.name);
    let format = station.format();
    let id = crate::ui::util::clamp_i64_to_i32(station.id);
    RadioStationRow {
        id,
        select_key: id,
        uuid: opt_shared(station.station_uuid.as_deref()),
        name: SharedString::from(&station.name),
        homepage: opt_shared(station.website()),
        editable: station.is_editable(),
        artwork_path: opt_shared(station.artwork_path.as_deref()),
        tags: SharedString::from(display_tags(station.genre().unwrap_or_default())),
        country: opt_shared(station.country_name()),
        codec: SharedString::from(display_codec(format.as_deref(), &station.codec, station.hls)),
        bitrate: station.bitrate,
        is_favorite: station.is_favorite,
        play_count: station.play_count,
        tile_color_1: tile.color_1,
        tile_color_2: tile.color_2,
        monogram: tile.monogram,
    }
}

/// One facet-list entry behind a filter chip, drawn through [`facets::drawn_as`].
pub fn to_slint_facet_row(chip: Option<facets::ChipFilter>, facet: &Facet) -> RadioFacetRow {
    let (label, code) = facets::drawn_as(chip, facet);
    RadioFacetRow {
        name: SharedString::from(label.as_ref()),
        code: SharedString::from(code),
        station_count: i32::try_from(facet.station_count).unwrap_or(i32::MAX),
    }
}

/// The tag field as separate labels for the hero's chip strip, under the card's cap; the hero is
/// mounted once, where a card is a virtualized delegate.
pub fn split_tags(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .take(TAG_DISPLAY_LIMIT)
        .map(str::to_owned)
        .collect()
}

/// The format a station's row states. A segmented station whose directory codec is `UNKNOWN` or
/// blank reads [`facets::SEGMENTED_CODEC_LABEL`], the row having the `hls` flag where the chip has
/// to generalise; one that isn't segmented keeps the directory's word.
///
/// `format` is `RadioStation::format`'s answer, `None` for a browsed station. The segmented test
/// reads the directory column rather than `format`, which a probed codec fills once the station
/// has played: what is worth saying is how the segments arrive, not what is inside them.
fn display_codec<'a>(format: Option<&'a str>, directory: &'a str, hls: bool) -> &'a str {
    let audio_unidentified = directory
        .split(',')
        .next()
        .is_some_and(|audio| audio.is_empty() || audio.eq_ignore_ascii_case(UNKNOWN_CODEC));
    if hls && audio_unidentified {
        return facets::SEGMENTED_CODEC_LABEL;
    }
    format.unwrap_or(directory)
}

/// The tag field as one display line: a card can't split a string, and a chip strip per card would
/// put a `changed` tracker in a delegate the virtualization destroys.
pub fn display_tags(raw: &str) -> String {
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
