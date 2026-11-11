//! A sheet track's row: what the sheet states, over what the image's own tags say for the whole
//! disc.
//!
//! A sheet value applies only where the sheet states one, and the image fills the rest, except for
//! what describes a single recording (BPM, key, mood, rating, the recording ids), which an image's
//! tags state for the whole disc and no track of it owns. An identifier or sort tag stays only
//! beside the field it came with, so a `PERFORMER` pairs with the disc's artist ids only where it
//! prints the disc's own credit.

use std::path::Path;

use lofty::tag::items::Timestamp;

use super::layout::{FileLayout, MIN_CLAIMED_ROWS};
use super::parse::{Sheet, SheetTrack, rem};
use crate::media::ingest::metadata::{
    parse_replaygain_gain, parse_replaygain_peak, split_genres, split_list,
};
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::credits::{CreditRole, RoleCredit, RoleCredits};
use melodia_core::entities::cue::{CueFrames, CueSpan, CueTrack, EmbeddedCue};
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::entities::tags::FileTags;

/// One row a sheet carves out of an image.
pub(crate) struct CueRow {
    pub(crate) track: CueTrack,
    pub(crate) span: CueSpan,
    pub(crate) meta: ExtractedMetadata,
}

/// The rows `layout` carves out of the image at `image`, whose own read is `disc`, or `None` where
/// fewer than [`MIN_CLAIMED_ROWS`] fit inside it. A track starting past the image's end is dropped
/// and logged, which only the image's length can tell, and the track before it plays to the end.
/// An image too short to split is another file the sheet's name led to, or a truncated one, and
/// keeps its own rows.
pub(crate) fn rows(
    sheet: &Sheet,
    layout: &FileLayout,
    image: &Path,
    disc: &ExtractedMetadata,
) -> Option<Vec<CueRow>> {
    let stem = image.file_stem().map(|stem| stem.to_string_lossy()).unwrap_or_default();
    let track_total = i32::try_from(layout.track_total()).unwrap_or(i32::MAX);
    let carved: Vec<CueRow> = layout
        .rows
        .iter()
        .filter(|row| {
            let starts_past_end = is_past_end(row.span.start(), disc);
            if starts_past_end {
                log::warn!(
                    "Dropped track {} of {}: it starts past the end of the file",
                    row.track.number(),
                    image.display()
                );
            }
            !starts_past_end
        })
        .map(|row| {
            let scopes = Scopes { track: row.source.map(|source| &sheet.tracks[source]), sheet };
            let mut meta = disc.clone();
            meta.embedded_cue = EmbeddedCue::Absent;
            meta.title = scopes
                .title()
                .map_or_else(|| format!("{stem} {:02}", row.track.number()), ToOwned::to_owned);
            meta.track_number = Some(i32::from(row.track.number()));
            meta.track_total = Some(track_total);
            let span = match row.span.end() {
                Some(end) if is_past_end(end, disc) => row.span.to_end_of_file(),
                _ => row.span,
            };
            meta.duration_ms = duration_ms(span, disc);
            apply_sheet(&mut meta, &scopes);
            forget_the_recording(&mut meta);
            CueRow { track: row.track, span, meta }
        })
        .collect();
    if carved.len() < MIN_CLAIMED_ROWS {
        log::warn!("{} is too short to split into its cue sheet's tracks", image.display());
        return None;
    }
    Some(carved)
}

/// Whether `at` lies at or past the end of the image `disc` reads, where it states its length.
fn is_past_end(at: CueFrames, disc: &ExtractedMetadata) -> bool {
    disc.duration_ms > 0 && at.millis() >= disc.duration_ms
}

/// The span's length, the last track's running to the end of the image.
fn duration_ms(span: CueSpan, disc: &ExtractedMetadata) -> i64 {
    let start = span.start().millis();
    match span.end() {
        Some(end) => end.millis() - start,
        None => (disc.duration_ms - start).max(0),
    }
}

/// Where a field is read from: the track's own lines first, then the sheet's.
struct Scopes<'a> {
    /// `None` for the hidden track.
    track: Option<&'a SheetTrack>,
    sheet: &'a Sheet,
}

impl Scopes<'_> {
    fn title(&self) -> Option<&str> {
        self.track?.fields.title.as_deref()
    }

    fn performer(&self) -> Option<&str> {
        let track = self.track.and_then(|track| track.fields.performer.as_deref());
        track.or(self.sheet.fields.performer.as_deref())
    }

    fn composer(&self) -> Option<&str> {
        let track = self.track.and_then(|track| {
            track.fields.songwriter.as_deref().or_else(|| track.fields.rem(rem::COMPOSER))
        });
        let sheet = &self.sheet.fields;
        track.or(sheet.songwriter.as_deref()).or_else(|| sheet.rem(rem::COMPOSER))
    }

    fn rem(&self, key: &str) -> Option<&str> {
        let track = self.track.and_then(|track| track.fields.rem(key));
        track.or_else(|| self.sheet.fields.rem(key))
    }

    fn track_rem(&self, key: &str) -> Option<&str> {
        self.track?.fields.rem(key)
    }
}

fn apply_sheet(meta: &mut ExtractedMetadata, scopes: &Scopes<'_>) {
    let sheet = &scopes.sheet.fields;
    if let Some(performer) = scopes.performer()
        && !restates(&meta.artist, performer)
    {
        meta.artist = ArtistCredit::from_name(performer);
        meta.artist_mbids.clear();
        meta.sort.artist = None;
    }
    if let Some(album) = &sheet.title {
        meta.album = Some(album.clone());
        meta.sort.album = None;
    }
    if let Some(album_artist) = &sheet.performer
        && !restates(&meta.album_artist, album_artist)
    {
        meta.album_artist = ArtistCredit::from_name(album_artist);
        meta.album_artist_mbids.clear();
        meta.sort.album_artist = None;
    }
    if let Some(composer) = scopes.composer() {
        meta.credits = with_composer(&meta.credits, composer);
    }
    if let Some(genre) = scopes.rem(rem::GENRE) {
        meta.genres = split_genres(genre);
    }
    if let Some(date) = scopes.rem(rem::DATE).and_then(|date| date.parse::<Timestamp>().ok()) {
        meta.release_date = Some(date.to_string());
        meta.year = Some(i32::from(date.year));
    }
    if let Some(disc) = sheet.rem(rem::DISCNUMBER).and_then(|n| n.parse().ok()) {
        meta.disc_number = Some(disc);
    }
    if let Some(discs) = sheet.rem(rem::TOTALDISCS).and_then(|n| n.parse().ok()) {
        meta.disc_total = Some(discs);
    }
    if let Some(catalog) = &scopes.sheet.catalog {
        meta.release.barcode = Some(catalog.clone());
    }
    meta.isrc = scopes.track.and_then(|track| track.isrc.clone());
    apply_replaygain(meta, scopes);
}

/// `image`'s credits, genres and release tags for track `track`, with what the sheet states over
/// them by [`apply_sheet`]'s rules. Lyrics describe a recording, so the image's are no track's.
pub(crate) fn track_tags(sheet: &Sheet, track: CueTrack, image: FileTags) -> FileTags {
    let scopes = Scopes { track: sheet.track(track), sheet };
    let mut tags = FileTags { lyrics: None, ..image };
    if let Some(performer) = scopes.performer()
        && !restates(&tags.credits.0, performer)
    {
        tags.credits.0 = ArtistCredit::from_name(performer);
    }
    if let Some(album_artist) = &sheet.fields.performer
        && !restates(&tags.credits.1, album_artist)
    {
        tags.credits.1 = ArtistCredit::from_name(album_artist);
    }
    if let Some(composer) = scopes.composer() {
        tags.roles = with_composer(&tags.roles, composer);
    }
    if let Some(genre) = scopes.rem(rem::GENRE) {
        tags.genres = split_genres(genre);
    }
    if let Some(catalog) = &sheet.catalog {
        tags.release.barcode.clone_from(catalog);
    }
    tags
}

/// Whether `stated` prints `credit` as it stands.
fn restates(credit: &ArtistCredit, stated: &str) -> bool {
    credit.line() == Some(stated)
}

/// The track pair is the track's own or none, since the image's describes the whole disc. The
/// album pair is the sheet's, else the image's, else the image's track pair, which over a whole
/// disc measures the album.
fn apply_replaygain(meta: &mut ExtractedMetadata, scopes: &Scopes<'_>) {
    let sheet = &scopes.sheet.fields;
    if let Some(gain) = sheet.rem("REPLAYGAIN_ALBUM_GAIN").and_then(parse_replaygain_gain) {
        meta.replaygain_album_gain = Some(gain);
        meta.replaygain_album_peak =
            sheet.rem("REPLAYGAIN_ALBUM_PEAK").and_then(parse_replaygain_peak);
    } else if meta.replaygain_album_gain.is_none() {
        meta.replaygain_album_gain = meta.replaygain_track_gain;
        meta.replaygain_album_peak = meta.replaygain_track_peak;
    }
    meta.replaygain_track_gain =
        scopes.track_rem("REPLAYGAIN_TRACK_GAIN").and_then(parse_replaygain_gain);
    meta.replaygain_track_peak =
        scopes.track_rem("REPLAYGAIN_TRACK_PEAK").and_then(parse_replaygain_peak);
}

/// `credits` with the sheet's composers in place of the image's, every other role kept.
fn with_composer(credits: &RoleCredits, composers: &str) -> RoleCredits {
    let composers = split_list(composers).map(|name| RoleCredit {
        role: CreditRole::Composer,
        name: name.to_owned(),
        detail: String::new(),
    });
    let others = credits.all().iter().filter(|credit| credit.role != CreditRole::Composer).cloned();
    RoleCredits::new(composers.chain(others).collect())
}

fn forget_the_recording(meta: &mut ExtractedMetadata) {
    meta.subtitle = None;
    meta.bpm = None;
    meta.initial_key = None;
    meta.mood = None;
    meta.work = None;
    meta.movement = None;
    meta.movement_number = None;
    meta.movement_total = None;
    meta.musicbrainz_track_id = None;
    meta.musicbrainz_release_track_id = None;
    meta.rating = None;
    meta.sort.title = None;
}
