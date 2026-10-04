//! The text a Copy entry puts on the clipboard, for every surface that offers one.
//!
//! A menu holds ids rather than text, so the text is resolved here: one line per item, in the order
//! the ids were handed over, which is the display order every selection keeps. An item without the
//! field asked for contributes no line rather than an empty one, so a pasted list never carries
//! blanks the user has to clean out.
//!
//! **Line breaks between items, never a space.** A title or a station name holds spaces of its
//! own, so a space-joined list cannot say where one item ends, and a path can hold commas. Pasted
//! into a single-line field, the breaks become whatever that field makes of them: Melodia's own
//! search box turns them into spaces, plenty of others drop them.
//!
//! **Nothing here logs what it resolves.** A copied line is the user's, a stream URL can carry a
//! session token, and the rolling log ships inside bug reports.

use std::collections::HashMap;

use crate::library::entity_tracks::EntityKind;
use crate::state::AppState;
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::AppError;
use melodia_store::database::queries;

/// What sits between a credit and a name on one line: what a stream announces, and what a search
/// box takes.
const CREDIT_SEPARATOR: &str = " - ";

/// What a track's Copy flyout offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackField {
    Title,
    Artist,
    Album,
    ArtistTitle,
    Path,
}

impl TrackField {
    /// Parses the token a `.slint` menu spells, `None` for anything else.
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "title" => Some(Self::Title),
            "artist" => Some(Self::Artist),
            "album" => Some(Self::Album),
            "artist-title" => Some(Self::ArtistTitle),
            "path" => Some(Self::Path),
            _ => None,
        }
    }
}

/// What an entity card's Copy entry offers. Every kind has a name; only an album credits an
/// artist, and the other kinds answer the two artist fields with nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityField {
    Name,
    Artist,
    ArtistName,
}

impl EntityField {
    /// Parses the token a `.slint` menu spells, `None` for anything else.
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "name" => Some(Self::Name),
            "artist" => Some(Self::Artist),
            "artist-name" => Some(Self::ArtistName),
            _ => None,
        }
    }
}

/// What a station card's Copy flyout offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StationField {
    Name,
    StreamUrl,
    Website,
}

impl StationField {
    /// Parses the token a `.slint` menu spells, `None` for anything else.
    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "name" => Some(Self::Name),
            "stream-url" => Some(Self::StreamUrl),
            "website" => Some(Self::Website),
            _ => None,
        }
    }
}

/// The parts of a station a Copy entry can reach. Borrowed, since the stations live in the radio
/// slice's caches and a directory station and a kept one are different types there.
#[derive(Clone, Copy, Debug)]
pub struct StationText<'a> {
    pub name: &'a str,
    pub stream_url: &'a str,
    pub website: Option<&'a str>,
}

/// One line per track, in the order `ids` gives.
///
/// # Errors
///
/// Propagates the lookup's database error.
pub async fn track_lines(
    state: &AppState,
    ids: &[i64],
    field: TrackField,
) -> Result<String, AppError> {
    let tracks = queries::track::get_track_summaries_by_ids(&state.db, ids).await?;
    Ok(join_lines(tracks.iter().filter_map(|track| track_line(track, field))))
}

/// One line per entity, in the order `ids` gives. An id deleted since the grid painted
/// contributes nothing.
///
/// # Errors
///
/// Propagates the lookup's database error.
pub async fn entity_lines(
    state: &AppState,
    kind: EntityKind,
    ids: &[i64],
    field: EntityField,
) -> Result<String, AppError> {
    let mut labels = entity_labels(state, kind, ids).await?;
    Ok(join_lines(
        ids.iter()
            .filter_map(|id| labels.remove(id))
            .filter_map(|label| entity_line(&label, field)),
    ))
}

/// One line per station, in the order handed over.
pub fn station_lines<'a>(
    stations: impl IntoIterator<Item = StationText<'a>>,
    field: StationField,
) -> String {
    join_lines(stations.into_iter().filter_map(|station| station_line(station, field)))
}

/// What a card names, and the artist it credits where it has one.
struct EntityLabel {
    name: String,
    artist: Option<String>,
}

async fn entity_labels(
    state: &AppState,
    kind: EntityKind,
    ids: &[i64],
) -> Result<HashMap<i64, EntityLabel>, AppError> {
    let db = &state.db;
    let labels = match kind {
        EntityKind::Album => queries::entity_labels::album_labels(db, ids)
            .await?
            .into_iter()
            .map(|(id, name, artist)| (id, EntityLabel { name, artist: Some(artist) }))
            .collect(),
        EntityKind::Artist => named(queries::entity_labels::artist_names(db, ids).await?),
        EntityKind::Genre => named(queries::entity_labels::genre_names(db, ids).await?),
        EntityKind::Playlist => named(queries::entity_labels::playlist_names(db, ids).await?),
        EntityKind::Track => queries::track::get_track_summaries_by_ids(db, ids)
            .await?
            .into_iter()
            .map(|track| (track.id, EntityLabel { name: track.title, artist: track.artist }))
            .collect(),
    };
    Ok(labels)
}

fn named(rows: Vec<(i64, String)>) -> HashMap<i64, EntityLabel> {
    rows.into_iter().map(|(id, name)| (id, EntityLabel { name, artist: None })).collect()
}

fn track_line(track: &TrackSummary, field: TrackField) -> Option<String> {
    match field {
        TrackField::Title => present(&track.title).map(str::to_owned),
        TrackField::Artist => track.artist.as_deref().and_then(present).map(str::to_owned),
        TrackField::Album => track.album.as_deref().and_then(present).map(str::to_owned),
        TrackField::ArtistTitle => credited(track.artist.as_deref(), &track.title),
        TrackField::Path => present(&track.file_path).map(str::to_owned),
    }
}

fn entity_line(label: &EntityLabel, field: EntityField) -> Option<String> {
    match field {
        EntityField::Name => present(&label.name).map(str::to_owned),
        EntityField::Artist => label.artist.as_deref().and_then(present).map(str::to_owned),
        EntityField::ArtistName => credited(label.artist.as_deref(), &label.name),
    }
}

fn station_line(station: StationText<'_>, field: StationField) -> Option<String> {
    let text = match field {
        StationField::Name => station.name,
        StationField::StreamUrl => station.stream_url,
        StationField::Website => station.website?,
    };
    present(text).map(str::to_owned)
}

/// `credit - name`, or the name alone where nothing is credited. A line with no name says
/// nothing, credit or not.
fn credited(credit: Option<&str>, name: &str) -> Option<String> {
    let name = present(name)?;
    Some(match credit.and_then(present) {
        Some(credit) => format!("{credit}{CREDIT_SEPARATOR}{name}"),
        None => name.to_owned(),
    })
}

/// A field that holds only whitespace is as missing as one that is `None`.
fn present(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn join_lines(lines: impl Iterator<Item = String>) -> String {
    lines.collect::<Vec<_>>().join("\n")
}
