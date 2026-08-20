//! Stations projected onto the Slint boundary structs.
//!
//! The tree's sixteen other `to_slint_*` converters live beside the view that fills the model,
//! and these are no different — what is different is the third input on the directory's side. A
//! browsed card's star and its logo are not facts about the station, they are facts about *this
//! install*, so both arrive as arguments rather than being looked up here: a converter that
//! reached for either would need the handle, and the caller already holds both while it walks the
//! page. A kept station carries them as columns and so needs neither.

use slint::SharedString;

use crate::entities::radio::{DirectoryStation, Facet, RadioStation};
use crate::{RadioFacetRow, RadioStationRow};

/// How many of a station's tags a card shows.
///
/// The directory's tag field is free-form and user-entered, so a popular station routinely
/// carries a dozen, most of them restatements of the first two. Three is what fits the card's
/// meta line at the narrowest column count without eliding.
const TAG_DISPLAY_LIMIT: usize = 3;

/// Separator between tags on the card's meta line. A middot rather than a comma, so the line
/// reads as a set of labels and not as prose.
const TAG_SEPARATOR: &str = " · ";

/// One browsed station, with this install's answers about it folded in.
///
/// `id` stays `0`: a directory station has no row until the user keeps or plays it, and that zero
/// is what every call site taking a whole row branches on.
pub fn to_slint_radio_station_row(
    station: &DirectoryStation,
    is_favorite: bool,
    logo: Option<&str>,
) -> RadioStationRow {
    RadioStationRow {
        id: 0,
        uuid: SharedString::from(&station.station_uuid),
        name: SharedString::from(&station.name),
        homepage: station.homepage.as_deref().map(SharedString::from).unwrap_or_default(),
        artwork_path: logo.map(SharedString::from).unwrap_or_default(),
        tags: SharedString::from(display_tags(&station.tags)),
        country: SharedString::from(&station.country),
        codec: SharedString::from(&station.codec),
        bitrate: station.bitrate,
        hls: station.hls,
        is_favorite,
    }
}

/// One kept station, as the two local tabs draw it.
///
/// Reads nothing but the row: the star and the logo are columns here, where a browsed station has
/// to be told both. `uuid` stays empty for a station the user typed in, which is what the card
/// splits its star and its Edit control on.
pub fn to_slint_kept_station_row(station: &RadioStation) -> RadioStationRow {
    RadioStationRow {
        id: crate::ui::util::clamp_i64_to_i32(station.id),
        uuid: station.station_uuid.as_deref().map(SharedString::from).unwrap_or_default(),
        name: SharedString::from(&station.name),
        homepage: station.homepage.as_deref().map(SharedString::from).unwrap_or_default(),
        artwork_path: station.artwork_path.as_deref().map(SharedString::from).unwrap_or_default(),
        tags: SharedString::from(display_tags(&station.tags)),
        country: SharedString::from(&station.country),
        codec: SharedString::from(&station.codec),
        bitrate: station.bitrate,
        hls: station.hls,
        is_favorite: station.is_favorite,
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
