use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

use lofty::file::{FileType, TaggedFile, TaggedFileExt};
use lofty::prelude::*;
use lofty::properties::FileProperties;
use lofty::tag::Tag;
use lofty::tag::items::Timestamp;

use super::{rating_tags, role_tags};
use melodia_artwork::media::image::artwork;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::scan::{ExtractedMetadata, ReleaseTags, SortTags};
use melodia_core::error::AppError;

/// Returns the BLAKE3 hash of the whole file, as 64 hex characters.
pub fn compute_file_hash(path: &Path) -> Result<String, AppError> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        AppError::metadata(format!("Failed to open {} for hashing", path.display()), e)
    })?;
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(&mut file)
        .map_err(|e| AppError::metadata(format!("Failed to hash {}", path.display()), e))?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Formats `meta`'s mtime as the RFC 3339 string `date_modified` stores, or `None` where it is
/// unavailable or out of range.
///
/// The one spelling of that format, since `scanner::track_is_current` compares against the stored
/// string byte for byte. Takes the `Metadata` so a caller that already `stat`ed doesn't again.
pub fn date_modified_from_metadata(meta: &std::fs::Metadata) -> Option<String> {
    meta.modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .and_then(|dur| {
            chrono::DateTime::from_timestamp(i64::try_from(dur.as_secs()).ok()?, dur.subsec_nanos())
        })
        .map(|dt| dt.to_rfc3339())
}

/// [`date_modified_from_metadata`] for a caller holding no `Metadata`, at the cost of a `stat`.
pub fn extract_date_modified(path: &Path) -> Option<String> {
    std::fs::metadata(path).ok().as_ref().and_then(date_modified_from_metadata)
}

/// The container `path` holds, named by its header alone.
///
/// `FileType::from_buffer` and never `Probe::guess_file_type`, whose fallback scans the first
/// kilobyte for an MPEG frame sync that arbitrary binary contains: a Matroska file comes back as
/// AAC, with a duration read out of the middle of somebody's audio. 36 bytes is lofty's longest
/// check.
///
/// **A fallback, never an identifier.** `from_buffer` doesn't search past a leading `ID3v2` tag,
/// so it answers `None` for most MP3s. Harmless where [`read_tags`] asks it, once the extension
/// resolved to nothing; as a primary gate it would refuse nearly every MP3 there is.
fn sniff_file_type(path: &Path) -> Option<FileType> {
    const SNIFF_BYTES: usize = 36;

    let mut head = Vec::with_capacity(SNIFF_BYTES);
    std::fs::File::open(path).ok()?.take(SNIFF_BYTES as u64).read_to_end(&mut head).ok()?;
    FileType::from_buffer(&head)
}

/// The channel count `path`'s Opus identification packet states, or `None` where the file is not
/// laid out the way an encoder writes one.
///
/// Located rather than read at a fixed offset, since an Ogg page header runs 27 bytes plus one per
/// segment. Anything this can't parse goes to lofty with every other malformed header.
fn opus_channel_count(path: &Path) -> Option<u8> {
    const PAGE_HEADER: usize = 27;
    const SEGMENT_COUNT_AT: usize = 26;
    const MAGIC: &[u8] = b"OpusHead";
    /// The longest segment table, then the magic, the version byte and the count.
    const HEAD_BYTES: usize = PAGE_HEADER + 255 + 10;

    let mut head = Vec::with_capacity(HEAD_BYTES);
    std::fs::File::open(path).ok()?.take(HEAD_BYTES as u64).read_to_end(&mut head).ok()?;

    let packet = head.get(PAGE_HEADER + usize::from(*head.get(SEGMENT_COUNT_AT)?)..)?;
    let rest = packet.strip_prefix(MAGIC)?;
    // Version, then the count.
    rest.get(1).copied()
}

/// How much of a file [`read_tags`] parses.
///
/// Pictures are most of a parse, and properties cost a full frame scan on a headerless VBR MP3.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TagScope {
    /// Tags, pictures and properties: what the scanner ingests.
    Full,
    /// Tags and properties, for a rescan that already has the pictures.
    NoArtwork,
    /// Tags alone, with no frame scan for duration or bitrate.
    TagsOnly,
}

/// Probes `path` for tags. Every lofty open in the tree comes through here, `tag_writer`'s
/// included, since a file the scan identifies by its header and the editor refuses by its
/// extension has tags that are visible and unsavable.
///
/// lofty keys `Probe::open` on an extension map narrower than its parsers (`.oga` maps to
/// nothing), so a file whose extension resolves to nothing is asked by its header. Only then,
/// which keeps the extra open off the scan's hot path.
pub fn read_tags(path: &Path, scope: TagScope) -> Result<TaggedFile, AppError> {
    let parse_opts = lofty::config::ParseOptions::new()
        .read_cover_art(scope == TagScope::Full)
        .read_properties(scope != TagScope::TagsOnly);

    let mut probe = lofty::probe::Probe::open(path)
        .map_err(|e| AppError::metadata(format!("Failed to open {}", path.display()), e))?
        .options(parse_opts);

    if probe.file_type().is_none()
        && let Some(sniffed) = sniff_file_type(path)
    {
        probe = probe.set_file_type(sniffed);
    }

    // lofty's Opus reader guards a count too large for its mapping family but not a zero, which
    // reaches `ChannelMask::from_opus_channels`' `expect`. Under `panic = "abort"` one crafted file
    // in a watched folder ends the process mid-scan. Not in `sniff_file_type`, which never sees a
    // file named `.opus`.
    if matches!(probe.file_type(), Some(FileType::Opus)) && opus_channel_count(path) == Some(0) {
        return Err(AppError::metadata_msg(format!("{} states no audio channels", path.display())));
    }

    probe
        .read()
        .map_err(|e| AppError::metadata(format!("Failed to read tags from {}", path.display()), e))
}

/// Parses a `ReplayGain` gain like `"-6.50 dB"`. Rejects `nan` and `inf`, which `f64` parses and
/// which would play the track as silence.
fn parse_replaygain_gain(s: &str) -> Option<f64> {
    s.trim().trim_end_matches("dB").trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Parses a linear `ReplayGain` peak like `"0.988553"`, finite only.
fn parse_replaygain_peak(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// What an `R128_*_GAIN` value moves by to mean what a `REPLAYGAIN_*` one does: R128 normalises to
/// −23 LUFS and `ReplayGain` 2.0 to −18, and RFC 7845 §5.2 fixes the reference the tag is written
/// against.
const R128_TO_REPLAYGAIN_DB: f64 = 5.0;

/// Parses an `R128_*_GAIN` value, Q7.8 fixed-point dB like `"-1280"`, restated against
/// `ReplayGain`'s reference. RFC 7845 states it as an `i16`, which caps it at ±128 dB and leaves no
/// non-finite value to guard.
fn parse_r128_gain(s: &str) -> Option<f64> {
    let q7_8 = s.trim().parse::<i16>().ok()?;
    Some(f64::from(q7_8) / 256.0 + R128_TO_REPLAYGAIN_DB)
}

/// What [`extract`] does with a file it can hash but whose tags won't parse.
#[derive(Clone, Copy)]
enum OnUnreadableTags {
    Fail,
    FilenameRow,
}

/// Reads a file's tags, properties and artwork into a row, failing if the tags won't parse.
///
/// Strict for the tag write's re-read: a row built from a parse that didn't happen would blank the
/// track instead of reporting the failure.
pub fn extract_metadata(
    path: &Path,
    artwork_dir: &Path,
    cover_cache: &artwork::CoverCache,
    skip_artwork: bool,
) -> Result<ExtractedMetadata, AppError> {
    extract(path, artwork_dir, cover_cache, skip_artwork, OnUnreadableTags::Fail)
}

/// As [`extract_metadata`], but a file whose tags won't parse still yields a row titled from its
/// filename, with external artwork and a decoder-probed duration where there are any.
///
/// For the scan paths, where the alternative is a file in a watched folder the library never
/// mentions: Matroska and CAF have no tag reader at all. Safe to do blind because the hash reads
/// the whole file first, so a parse failure past it is the format's, not the disk's.
pub fn extract_or_filename_row(
    path: &Path,
    artwork_dir: &Path,
    cover_cache: &artwork::CoverCache,
    skip_artwork: bool,
) -> Result<ExtractedMetadata, AppError> {
    extract(path, artwork_dir, cover_cache, skip_artwork, OnUnreadableTags::FilenameRow)
}

fn extract(
    path: &Path,
    artwork_dir: &Path,
    cover_cache: &artwork::CoverCache,
    skip_artwork: bool,
    on_unreadable: OnUnreadableTags,
) -> Result<ExtractedMetadata, AppError> {
    let file_name = || path.file_stem().and_then(|s| s.to_str()).unwrap_or("Unknown").to_owned();

    let (file_size, date_modified) = file_stat(path);
    let file_hash = compute_file_hash(path)?;
    let scope = if skip_artwork { TagScope::NoArtwork } else { TagScope::Full };
    let tagged_file = open_tags(path, scope, on_unreadable)?;
    let audio = AudioProperties::read(path, tagged_file.as_ref());

    let tag =
        tagged_file.as_ref().and_then(|tagged| tagged.primary_tag().or_else(|| tagged.first_tag()));

    let artwork_path = if skip_artwork {
        None
    } else {
        artwork::find_and_cache_artwork(path, tag, artwork_dir, cover_cache)
    };

    let title = text(tag, ItemKey::TrackTitle).unwrap_or_else(file_name);

    let artist = read_credit(tag, ItemKey::TrackArtist, ItemKey::TrackArtists);
    let album_artist = read_credit(tag, ItemKey::AlbumArtist, ItemKey::AlbumArtists);

    let released = release_timestamp(tag);
    let originally_released = read_timestamp(tag, ItemKey::OriginalReleaseDate);

    let rating = tag.and_then(rating_tags::stars_from_tag);
    let credits = tag.map(role_tags::read_roles).unwrap_or_default();

    Ok(ExtractedMetadata {
        artist_mbids: read_credit_mbids(tag, ItemKey::MusicBrainzArtistId, artist.artists().len()),
        album_artist_mbids: read_credit_mbids(
            tag,
            ItemKey::MusicBrainzReleaseArtistId,
            album_artist.artists().len(),
        ),
        title,
        artist,
        album_artist,
        album: text(tag, ItemKey::AlbumTitle),
        genres: read_genres(tag),
        credits,
        sort: read_sort_tags(tag),
        release: read_release_tags(tag),
        track_number: count(tag.and_then(Tag::track)),
        track_total: count(tag.and_then(Tag::track_total)),
        disc_number: count(tag.and_then(Tag::disk)),
        disc_total: count(tag.and_then(Tag::disk_total)),
        disc_subtitle: text(tag, ItemKey::SetSubtitle),
        subtitle: text(tag, ItemKey::TrackSubtitle),
        release_date: released.map(|ts| ts.to_string()),
        year: released.map(|ts| i32::from(ts.year)),
        original_date: originally_released.map(|ts| ts.to_string()),
        original_year: originally_released.map(|ts| i32::from(ts.year)),
        comment: text(tag, ItemKey::Comment),
        bpm: read_bpm(tag),
        initial_key: text(tag, ItemKey::InitialKey),
        mood: text(tag, ItemKey::Mood),
        grouping: text(tag, ItemKey::ContentGroup),
        work: text(tag, ItemKey::Work),
        movement: text(tag, ItemKey::Movement),
        movement_number: number(tag, ItemKey::MovementNumber),
        movement_total: number(tag, ItemKey::MovementTotal),
        language: text(tag, ItemKey::Language),
        copyright: text(tag, ItemKey::CopyrightMessage),
        isrc: text(tag, ItemKey::Isrc),
        musicbrainz_track_id: text(tag, ItemKey::MusicBrainzRecordingId),
        musicbrainz_release_id: text(tag, ItemKey::MusicBrainzReleaseId),
        musicbrainz_release_track_id: text(tag, ItemKey::MusicBrainzTrackId),
        replaygain_track_gain: replaygain_gain(
            tag,
            ItemKey::ReplayGainTrackGain,
            ItemKey::R128TrackGain,
        ),
        replaygain_track_peak: replaygain_peak(tag, ItemKey::ReplayGainTrackPeak),
        replaygain_album_gain: replaygain_gain(
            tag,
            ItemKey::ReplayGainAlbumGain,
            ItemKey::R128AlbumGain,
        ),
        replaygain_album_peak: replaygain_peak(tag, ItemKey::ReplayGainAlbumPeak),
        rating,
        duration_ms: audio.duration_ms,
        codec: audio.codec,
        bitrate: audio.bitrate,
        channels: audio.channels,
        sample_rate: audio.sample_rate,
        bit_depth: audio.bit_depth,
        file_size,
        file_hash,
        date_modified,
        artwork_path,
    })
}

/// The file's size and its stored mtime string, off one `stat`.
fn file_stat(path: &Path) -> (i64, Option<String>) {
    let fs_meta = std::fs::metadata(path);
    let file_size = fs_meta.as_ref().map_or(0, |m| i64::try_from(m.len()).unwrap_or(i64::MAX));
    let date_modified = fs_meta.as_ref().ok().and_then(date_modified_from_metadata);
    (file_size, date_modified)
}

/// The parsed file, or `None` where the tags won't parse and the caller keeps a filename row.
fn open_tags(
    path: &Path,
    scope: TagScope,
    on_unreadable: OnUnreadableTags,
) -> Result<Option<TaggedFile>, AppError> {
    match read_tags(path, scope) {
        Ok(tagged) => Ok(Some(tagged)),
        Err(e) => match on_unreadable {
            OnUnreadableTags::Fail => Err(e),
            OnUnreadableTags::FilenameRow => {
                log::debug!(
                    "{}; keeping a filename-derived row",
                    melodia_core::error::describe(&e)
                );
                Ok(None)
            }
        },
    }
}

/// What the stream is rather than what its tags say.
struct AudioProperties {
    duration_ms: i64,
    codec: Option<String>,
    bitrate: Option<i32>,
    channels: Option<i32>,
    sample_rate: Option<i32>,
    bit_depth: Option<i32>,
}

impl AudioProperties {
    fn read(path: &Path, tagged_file: Option<&TaggedFile>) -> Self {
        let properties = tagged_file.map(TaggedFile::properties);
        let duration_ms = match properties {
            Some(props) => i64::try_from(props.duration().as_millis()).unwrap_or(i64::MAX),
            // The parse failed, so only the decoder can count it; `0` where it can't either.
            None => melodia_audio::player::source::file_decode::probe_duration(path)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
        };
        Self {
            duration_ms,
            codec: tagged_file.map(|tagged| format!("{:?}", tagged.file_type())),
            bitrate: properties
                .and_then(|props| props.overall_bitrate().or(props.audio_bitrate()))
                .map(|br| i32::try_from(br).unwrap_or(i32::MAX)),
            channels: properties.and_then(FileProperties::channels).map(i32::from),
            sample_rate: properties
                .and_then(FileProperties::sample_rate)
                .map(|rate| i32::try_from(rate).unwrap_or(i32::MAX)),
            bit_depth: properties.and_then(FileProperties::bit_depth).map(i32::from),
        }
    }
}

/// `ItemKey::Bpm` has no `ID3v2` mapping: MP3, WAV and AIFF keep BPM in `TBPM`, which lofty reads
/// as `IntegerBpm`, and so does `tag_writer`'s write. The decimal key wins where both exist.
fn read_bpm(tag: Option<&Tag>) -> Option<f64> {
    text(tag, ItemKey::Bpm)
        .or_else(|| text(tag, ItemKey::IntegerBpm))
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// A `ReplayGain` gain, or its `R128` counterpart restated against the same reference.
///
/// Opus carries `R128_*_GAIN`, and read as untagged would play at unity in a normalised library.
/// `REPLAYGAIN_*` wins where both exist, already meaning what the columns mean.
fn replaygain_gain(tag: Option<&Tag>, replaygain: ItemKey, r128: ItemKey) -> Option<f64> {
    text(tag, replaygain)
        .as_deref()
        .and_then(parse_replaygain_gain)
        .or_else(|| text(tag, r128).as_deref().and_then(parse_r128_gain))
}

/// Neither peak has an R128 counterpart — R128 defines none — and the prevent-clipping path
/// already treats a peak it doesn't know as no ceiling.
fn replaygain_peak(tag: Option<&Tag>, key: ItemKey) -> Option<f64> {
    text(tag, key).as_deref().and_then(parse_replaygain_peak)
}

/// One tag value, trimmed, `None` when absent or blank. Takes `Option<&Tag>` so a file whose tags
/// wouldn't parse reads as one with none.
///
/// Blank is absent: a whitespace-only `Album` or `Genre` would get past `upsert_album`'s and
/// `upsert_genre`'s `is_empty()` guards and make a ghost row.
fn text(tag: Option<&Tag>, key: ItemKey) -> Option<String> {
    tag.and_then(|tag| tag.get_string(key)).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// [`text`] parsed as a whole number, for the counts that have no [`Accessor`] getter.
fn number(tag: Option<&Tag>, key: ItemKey) -> Option<i32> {
    text(tag, key).and_then(|s| s.parse().ok())
}

/// Whether a flag tag is set: anything but `0` or `false`, `TCMP` being written as `1` and
/// `COMPILATION` as either.
///
/// **The `false` has to be case-folded.** `upsert_album`'s `is_compilation` is an `OR`, so one file
/// spelling it `FALSE` would mark the whole release and no rescan could clear it. An unknown
/// spelling fails toward set, the direction a present flag argues for.
fn flag(tag: Option<&Tag>, key: ItemKey) -> bool {
    text(tag, key).is_some_and(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
}

/// A count lofty already parsed, saturated rather than wrapped.
fn count(value: Option<u32>) -> Option<i32> {
    value.map(|n| i32::try_from(n).unwrap_or(i32::MAX))
}

/// A whole date under one key. `BestAttempt` parsing, so a year-only tag is a year-only
/// [`Timestamp`] and renders back as the four digits it came in as.
fn read_timestamp(tag: Option<&Tag>, key: ItemKey) -> Option<Timestamp> {
    text(tag, key).and_then(|value| value.parse().ok())
}

/// Where a release date can sit, in the order it is taken: `TDRL`/`RELEASEDATE` is the explicit
/// answer, `TDRC`/`DATE` the one Picard writes, and `YEAR` a Vorbis spelling some rippers emit
/// alone. [`super::tag_writer`] clears all three before writing, so an edit can't land behind a
/// key read first.
pub(super) const RELEASE_DATE_KEYS: [ItemKey; 3] =
    [ItemKey::ReleaseDate, ItemKey::RecordingDate, ItemKey::Year];

/// The release date a file carries, under whichever of [`RELEASE_DATE_KEYS`] it used.
pub(super) fn release_timestamp(tag: Option<&Tag>) -> Option<Timestamp> {
    RELEASE_DATE_KEYS.into_iter().find_map(|key| read_timestamp(tag, key))
}

/// Every genre the file names.
///
/// A repeated `GENRE` is already a list. A single value splits on `;` alone, the separator Picard
/// flattens a list to in ID3v2.3: `ID3v1`'s own list holds `Pop/Funk`, and `,` and `&` live inside
/// names like `Drum & Bass`.
fn read_genres(tag: Option<&Tag>) -> GenreList {
    let Some(tag) = tag else {
        return GenreList::default();
    };
    let values = trimmed_values(tag, ItemKey::Genre);
    let names = match values.as_slice() {
        [single] => {
            single.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
        }
        _ => values,
    };
    GenreList::new(names)
}

fn read_sort_tags(tag: Option<&Tag>) -> SortTags {
    SortTags {
        artist: text(tag, ItemKey::TrackArtistSortOrder),
        album_artist: text(tag, ItemKey::AlbumArtistSortOrder),
        album: text(tag, ItemKey::AlbumTitleSortOrder),
        title: text(tag, ItemKey::TrackTitleSortOrder),
    }
}

fn read_release_tags(tag: Option<&Tag>) -> ReleaseTags {
    ReleaseTags {
        label: text(tag, ItemKey::Label),
        catalog_number: text(tag, ItemKey::CatalogNumber),
        barcode: text(tag, ItemKey::Barcode),
        media: text(tag, ItemKey::OriginalMediaType),
        release_type: text(tag, ItemKey::MusicBrainzReleaseType),
        release_country: text(tag, ItemKey::ReleaseCountry),
        musicbrainz_release_group_id: text(tag, ItemKey::MusicBrainzReleaseGroupId),
        is_compilation: flag(tag, ItemKey::FlagCompilation),
    }
}

/// The `MusicBrainz` ids listed in parallel with a credit's names, **all of them or none**.
///
/// The pairing is positional, so counts that disagree leave no alignment to trust, and a misaligned
/// id stamps the wrong `MBID` onto a real artist where nothing downstream can catch it.
fn read_credit_mbids(tag: Option<&Tag>, key: ItemKey, credited: usize) -> Vec<String> {
    let Some(tag) = tag else {
        return Vec::new();
    };
    let ids = trimmed_values(tag, key);
    if ids.len() == credited { ids } else { Vec::new() }
}

/// A file's two artist credits, off the tag the Edit-Tags dialog already opened for its lyrics.
/// The file is the authority, which a database seeded from `artist_id` alone cannot be.
pub fn credits_from_tag(tag: Option<&Tag>) -> (ArtistCredit, ArtistCredit) {
    (
        read_credit(tag, ItemKey::TrackArtist, ItemKey::TrackArtists),
        read_credit(tag, ItemKey::AlbumArtist, ItemKey::AlbumArtists),
    )
}

/// What one artist field reads as: the credit, and the string that renders it.
///
/// `ARTISTS` wins, then a multi-value `ARTIST` (a NUL-separated `TPE1`, a repeated Vorbis field).
/// A *single* value is one artist whatever it contains: `AC/DC` and `Earth, Wind & Fire` are one
/// name each, and no exceptions list for splitting on punctuation could be finished. The string is
/// rendered from the credit, so the column and its join rows can't disagree.
fn read_credit(tag: Option<&Tag>, printed_key: ItemKey, list_key: ItemKey) -> ArtistCredit {
    let Some(tag) = tag else {
        return ArtistCredit::default();
    };
    let listed = unflattened(trimmed_values(tag, list_key));
    let printed = trimmed_values(tag, printed_key);
    let credit_line = printed.first().cloned().unwrap_or_default();
    let names = if listed.is_empty() { printed } else { listed };

    ArtistCredit::from_tags(&credit_line, &names)
}

/// A list tag that arrived as one `"; "`-joined string, split back into its names.
///
/// ID3v2.3 has no multi-value frame, so Picard writes `TXXX:Artists` as `"Alice; Bob"`. Only the
/// **list** key comes here, where a single value can only be a flattened list, and the space is
/// required so a bare `;` inside a name stays there.
fn unflattened(values: Vec<String>) -> Vec<String> {
    let [single] = values.as_slice() else {
        return values;
    };
    if !single.contains("; ") {
        return values;
    }
    single.split("; ").map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// Every value under `key`, trimmed, blanks dropped; [`super::role_tags`] reads through it too.
/// Ripped libraries carry whitespace-only artist fields, and a NUL-terminated UTF-8 frame leaves an
/// empty tail value.
pub(super) fn trimmed_values(tag: &Tag, key: ItemKey) -> Vec<String> {
    tag.get_strings(key).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

#[cfg(test)]
#[path = "tests/metadata_tests.rs"]
mod tests;
