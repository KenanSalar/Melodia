use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

use lofty::file::{FileType, TaggedFile, TaggedFileExt};
use lofty::prelude::*;
use lofty::properties::FileProperties;

use super::rating_tags;
use melodia_artwork::media::image::artwork;
use melodia_core::entities::scan::ExtractedMetadata;
use melodia_core::error::AppError;

/// Compute a full BLAKE3 hash of a file (64-char hex string).
/// Uses `update_reader` for optimized streaming I/O with SIMD-friendly buffering.
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

/// Format an already-fetched `Metadata`'s modification time as an RFC 3339
/// string. Returns `None` if the mtime is unavailable or out of range.
///
/// This is the single source of truth for the mtime string format. Callers
/// that compare against a stored `date_modified` (e.g. the incremental-scan
/// filter `scanner::track_is_current`) must derive their value through here
/// so the formats can't drift apart. Takes `&Metadata` so a caller that
/// already `stat`-ed the file doesn't pay for a second syscall.
pub fn date_modified_from_metadata(meta: &std::fs::Metadata) -> Option<String> {
    meta.modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .and_then(|dur| {
            chrono::DateTime::from_timestamp(i64::try_from(dur.as_secs()).ok()?, dur.subsec_nanos())
        })
        .map(|dt| dt.to_rfc3339())
}

/// Extract the file's modification time as an RFC 3339 string.
/// Returns `None` if the file metadata is unavailable or the timestamp is out of range.
pub fn extract_date_modified(path: &Path) -> Option<String> {
    std::fs::metadata(path).ok().as_ref().and_then(date_modified_from_metadata)
}

/// The container `path` holds, named by its header alone.
///
/// `FileType::from_buffer` is the strict half of lofty's sniffing, and the strictness is
/// the point. `Probe::guess_file_type` falls through to scanning the first kilobyte for
/// an MPEG frame sync, which arbitrary binary contains. A Matroska file comes back
/// confidently labelled AAC, carrying a sample rate and duration read out of the middle
/// of somebody's audio. A header matching nothing has to stay unidentified.
///
/// Reads what lofty's own sniffer reads: its longest check reaches byte 36.
fn sniff_file_type(path: &Path) -> Option<FileType> {
    const SNIFF_BYTES: usize = 36;

    let mut head = Vec::with_capacity(SNIFF_BYTES);
    std::fs::File::open(path).ok()?.take(SNIFF_BYTES as u64).read_to_end(&mut head).ok()?;
    FileType::from_buffer(&head)
}

/// How much of a file [`read_tags`] is being asked for.
///
/// Both halves lofty can be talked out of are expensive and neither is optional by default:
/// embedded pictures are the larger part of a parse, and `read_properties` costs a full frame
/// scan on a headerless VBR MP3, which is the shape a duration has to be counted out of.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TagScope {
    /// Tags, embedded pictures and the technical properties: what the scanner ingests.
    Full,
    /// Tags and properties. For a rescan, which already has the pictures.
    NoArtwork,
    /// Tags alone: no pictures, and no frame scan for duration or bitrate.
    TagsOnly,
}

/// Probe `path` for tags.
///
/// lofty keys `Probe::open` on the extension, and its map of those is narrower than what
/// its parsers cover: `.oga` resolves to nothing there, so an Ogg Vorbis file named that
/// way reads as an unknown format. Asking the header covers it. Only asked when the
/// extension resolved to nothing, so every file that parses today still parses the same
/// way, and the extra open stays off the scan's hot path.
///
/// Every lofty open in the tree comes through here, `media::ingest::tag_writer` included: a file the
/// scan identifies by its header and the tag editor refuses by its extension is a track whose
/// tags are visible and unsavable.
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

    probe
        .read()
        .map_err(|e| AppError::metadata(format!("Failed to read tags from {}", path.display()), e))
}

/// Parse a `ReplayGain` gain string like "-6.50 dB" to f64. Rejects non-finite
/// values (`"nan"`, `"inf"` parse successfully as floats in Rust) so a malformed
/// tag can't poison the playback DSP — the value is baked into the audio source
/// and a `NaN`/`inf` gain would render the track as silence.
fn parse_replaygain_gain(s: &str) -> Option<f64> {
    s.trim().trim_end_matches("dB").trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Parse a `ReplayGain` peak string (linear scale, e.g. "0.988553") to f64.
/// Rejects non-finite values for the same reason as the gain parser.
fn parse_replaygain_peak(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// What [`extract`] does with a file it can hash but whose tags won't parse.
#[derive(Clone, Copy)]
enum OnUnreadableTags {
    Fail,
    FilenameRow,
}

/// Read a file's tags, properties and artwork into a row.
///
/// Fails if the tags won't parse. Callers that write a file and re-read it to refresh
/// its row want exactly that: a row built from a parse that didn't happen would blank
/// the track instead of reporting the failure.
pub fn extract_metadata(
    path: &Path,
    artwork_dir: &Path,
    cover_cache: &artwork::CoverCache,
    skip_artwork: bool,
) -> Result<ExtractedMetadata, AppError> {
    extract(path, artwork_dir, cover_cache, skip_artwork, OnUnreadableTags::Fail)
}

/// As [`extract_metadata`], but a file whose tags won't parse still yields a row, titled
/// from its filename, with external artwork and a decoder-probed duration if either is
/// there to be had.
///
/// For the scan paths, where the alternative is the file disappearing: a container with
/// no tag reader (Matroska, CAF) and one with tags too broken to parse both arrive here,
/// and dropping either leaves a file sitting in a watched folder that the library never
/// mentions. The hash above the parse is what makes this safe to do blind. It reads the
/// whole file, so anything that gets past it is readable and the parse failure is the
/// format's, not the disk's.
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
    // Only allocate the fallback name if a tag title is actually missing — for
    // a tagged music library this avoids ~1 String allocation per scanned file
    // on the hot scan path.
    let file_name = || path.file_stem().and_then(|s| s.to_str()).unwrap_or("Unknown").to_owned();

    let fs_meta = std::fs::metadata(path);
    let file_size = fs_meta.as_ref().map_or(0, |m| i64::try_from(m.len()).unwrap_or(i64::MAX));

    // Derived from the `Metadata` already in hand — `extract_date_modified` would
    // `stat` the file a second time. This is exactly what
    // `date_modified_from_metadata` exists for; `scanner::track_is_current` is the
    // other caller that already holds one.
    let date_modified = fs_meta.as_ref().ok().and_then(date_modified_from_metadata);

    let file_hash = compute_file_hash(path)?;

    let scope = if skip_artwork {
        TagScope::NoArtwork
    } else {
        TagScope::Full
    };
    let tagged_file = match read_tags(path, scope) {
        Ok(tagged) => Some(tagged),
        Err(e) => match on_unreadable {
            OnUnreadableTags::Fail => return Err(e),
            OnUnreadableTags::FilenameRow => {
                log::debug!(
                    "{}; keeping a filename-derived row",
                    melodia_core::error::describe(&e)
                );
                None
            }
        },
    };

    let properties = tagged_file.as_ref().map(TaggedFile::properties);

    let duration_ms = match properties {
        Some(props) => i64::try_from(props.duration().as_millis()).unwrap_or(i64::MAX),
        // Lofty reports duration off the parse that just failed, so the decoder is the
        // only thing left that knows. Still `0` where it can't say either.
        None => melodia_audio::player::source::file_decode::probe_duration(path)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
    };

    let bitrate = properties
        .and_then(|props| props.overall_bitrate().or(props.audio_bitrate()))
        .map(|br| i32::try_from(br).unwrap_or(i32::MAX));
    let channels = properties.and_then(FileProperties::channels).map(i32::from);
    let sample_rate = properties
        .and_then(FileProperties::sample_rate)
        .map(|rate| i32::try_from(rate).unwrap_or(i32::MAX));
    let bit_depth = properties.and_then(FileProperties::bit_depth).map(i32::from);

    // Determine codec from file type
    let codec = tagged_file.as_ref().map(|tagged| format!("{:?}", tagged.file_type()));

    // Try to read tags - check all tag types and pick the first one with data
    let tag =
        tagged_file.as_ref().and_then(|tagged| tagged.primary_tag().or_else(|| tagged.first_tag()));

    // Extract artwork: check external cover files first, then embedded tag
    let artwork_path = if skip_artwork {
        None
    } else {
        artwork::find_and_cache_artwork(path, tag, artwork_dir, cover_cache)
    };

    // Trim + drop whitespace-only tags. Some ripped/transcoded files carry
    // an `Artist`/`Album`/`Genre` field that's nothing but spaces; left as-is
    // they bypass the `is_empty()` guard in `upsert_artist`/`upsert_album`/
    // `upsert_genre` and create ghost entity rows that pollute the Artists
    // view and trigger Deezer image-fetch warnings on startup.
    let (
        title,
        artist,
        album_artist,
        album,
        genre,
        track_number,
        disc_number,
        year,
        composer,
        comment,
    ) = if let Some(tag) = tag {
        (
            // tag.title()/artist()/album()/genre()/comment() return Cow<str>; use to_string()
            // (Cow::to_owned returns Cow). get_string() returns &str → to_owned().
            tag.title()
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(file_name),
            tag.artist().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
            tag.get_string(ItemKey::AlbumArtist)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            tag.album().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
            tag.genre().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
            tag.track().map(|t| i32::try_from(t).unwrap_or(i32::MAX)),
            tag.disk().map(|d| i32::try_from(d).unwrap_or(i32::MAX)),
            tag.date().map(|ts| i32::from(ts.year)),
            tag.get_string(ItemKey::Composer)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            tag.comment().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
        )
    } else {
        (file_name(), None, None, None, None, None, None, None, None, None)
    };

    // Extract extended metadata from tags
    let (
        bpm,
        musicbrainz_track_id,
        musicbrainz_release_id,
        label,
        original_year,
        replaygain_track_gain,
        replaygain_track_peak,
        replaygain_album_gain,
        replaygain_album_peak,
    ) = if let Some(tag) = tag {
        (
            // `ItemKey::Bpm` has NO ID3v2 mapping — MP3 / WAV / AIFF keep BPM in `TBPM`,
            // which lofty exposes as `IntegerBpm`. Reading only `Bpm` therefore misses it
            // on every ID3v2 file, including the ones `tag_writer::apply_bpm` writes.
            // Prefer the decimal key (Vorbis `BPM`, MP4 freeform); fall back to the integer.
            tag.get_string(ItemKey::Bpm)
                .or_else(|| tag.get_string(ItemKey::IntegerBpm))
                .and_then(|s| s.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite()),
            tag.get_string(ItemKey::MusicBrainzRecordingId)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            tag.get_string(ItemKey::MusicBrainzReleaseId)
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty()),
            tag.get_string(ItemKey::Label).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
            tag.get_string(ItemKey::OriginalReleaseDate)
                .and_then(|s| s.get(..4).and_then(|y| y.parse::<i32>().ok())),
            tag.get_string(ItemKey::ReplayGainTrackGain).and_then(parse_replaygain_gain),
            tag.get_string(ItemKey::ReplayGainTrackPeak).and_then(parse_replaygain_peak),
            tag.get_string(ItemKey::ReplayGainAlbumGain).and_then(parse_replaygain_gain),
            tag.get_string(ItemKey::ReplayGainAlbumPeak).and_then(parse_replaygain_peak),
        )
    } else {
        (None, None, None, None, None, None, None, None, None)
    };

    // Its own line rather than a tenth slot in the tuple above: the whole read is one call, and
    // the conversion it fronts is argued in `rating_tags` where every format's shape is in view.
    let rating = tag.and_then(rating_tags::stars_from_tag);

    Ok(ExtractedMetadata {
        title,
        artist,
        album_artist,
        album,
        genre,
        track_number,
        disc_number,
        year,
        composer,
        comment,
        bpm,
        musicbrainz_track_id,
        musicbrainz_release_id,
        label,
        original_year,
        replaygain_track_gain,
        replaygain_track_peak,
        replaygain_album_gain,
        replaygain_album_peak,
        rating,
        duration_ms,
        codec,
        bitrate,
        channels,
        sample_rate,
        bit_depth,
        file_size,
        file_hash,
        date_modified,
        artwork_path,
    })
}

#[cfg(test)]
#[path = "tests/metadata_tests.rs"]
mod tests;
