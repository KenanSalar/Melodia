use std::path::Path;
use std::time::UNIX_EPOCH;

use lofty::file::TaggedFileExt;
use lofty::prelude::*;

use crate::error::AppError;
use super::artwork;

#[derive(Debug, Clone)]
pub struct ExtractedMetadata {
    pub title: String,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub year: Option<i32>,
    pub composer: Option<String>,
    pub comment: Option<String>,
    pub bpm: Option<f64>,
    pub musicbrainz_track_id: Option<String>,
    pub musicbrainz_release_id: Option<String>,
    pub label: Option<String>,
    pub original_year: Option<i32>,
    pub replaygain_track_gain: Option<f64>,
    pub replaygain_track_peak: Option<f64>,
    pub replaygain_album_gain: Option<f64>,
    pub replaygain_album_peak: Option<f64>,
    pub duration_ms: i64,
    pub codec: Option<String>,
    pub bitrate: Option<i32>,
    pub channels: Option<i32>,
    pub sample_rate: Option<i32>,
    pub bit_depth: Option<i32>,
    pub file_size: i64,
    pub file_hash: String,
    pub date_modified: Option<String>,
    pub artwork_path: Option<String>,
}

/// Compute a full BLAKE3 hash of a file (64-char hex string).
/// Uses `update_reader` for optimized streaming I/O with SIMD-friendly buffering.
pub fn compute_file_hash(path: &Path) -> Result<String, AppError> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| AppError::Metadata(format!("Failed to open {} for hashing: {}", path.display(), e)))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(&mut file)
        .map_err(|e| AppError::Metadata(format!("Failed to hash {}: {}", path.display(), e)))?;
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
            chrono::DateTime::from_timestamp(
                i64::try_from(dur.as_secs()).ok()?,
                dur.subsec_nanos(),
            )
        })
        .map(|dt| dt.to_rfc3339())
}

/// Extract the file's modification time as an RFC 3339 string.
/// Returns `None` if the file metadata is unavailable or the timestamp is out of range.
pub fn extract_date_modified(path: &Path) -> Option<String> {
    std::fs::metadata(path)
        .ok()
        .as_ref()
        .and_then(date_modified_from_metadata)
}

/// Parse a `ReplayGain` gain string like "-6.50 dB" to f64
fn parse_replaygain_gain(s: &str) -> Option<f64> {
    s.trim().trim_end_matches("dB").trim().parse::<f64>().ok()
}

/// Parse a `ReplayGain` peak string (linear scale, e.g. "0.988553") to f64
fn parse_replaygain_peak(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

pub fn extract_metadata(
    path: &Path,
    artwork_dir: &Path,
    cover_cache: &artwork::CoverCache,
    skip_artwork: bool,
) -> Result<ExtractedMetadata, AppError> {
    // Only allocate the fallback name if a tag title is actually missing — for
    // a tagged music library this avoids ~1 String allocation per scanned file
    // on the hot scan path.
    let file_name = || {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_owned()
    };

    let fs_meta = std::fs::metadata(path);
    let file_size = fs_meta
        .as_ref()
        .map(|m| i64::try_from(m.len()).unwrap_or(i64::MAX))
        .unwrap_or(0);

    let date_modified = extract_date_modified(path);

    let file_hash = compute_file_hash(path)?;

    let tagged_file = if skip_artwork {
        // Skip reading embedded pictures — significant speedup for rescans
        let parse_opts = lofty::config::ParseOptions::new().read_cover_art(false);
        lofty::probe::Probe::open(path)
            .map_err(|e| AppError::Metadata(format!("Failed to open {}: {}", path.display(), e)))?
            .options(parse_opts)
            .read()
            .map_err(|e| AppError::Metadata(format!("Failed to read tags from {}: {}", path.display(), e)))?
    } else {
        lofty::probe::read_from_path(path)
            .map_err(|e| AppError::Metadata(format!("Failed to read tags from {}: {}", path.display(), e)))?
    };

    let properties = tagged_file.properties();
    let duration_ms =
        i64::try_from(properties.duration().as_millis()).unwrap_or(i64::MAX);

    let bitrate = {
        let br = properties.overall_bitrate().or(properties.audio_bitrate());
        br.map(|b| i32::try_from(b).unwrap_or(i32::MAX))
    };
    let channels = properties.channels().map(i32::from);
    let sample_rate = properties
        .sample_rate()
        .map(|s| i32::try_from(s).unwrap_or(i32::MAX));
    let bit_depth = properties.bit_depth().map(i32::from);

    // Determine codec from file type
    let codec = Some(format!("{:?}", tagged_file.file_type()));

    // Try to read tags - check all tag types and pick the first one with data
    let tag = tagged_file.primary_tag().or_else(|| tagged_file.first_tag());

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
    let (title, artist, album_artist, album, genre, track_number, disc_number, year, composer, comment) =
        if let Some(tag) = tag {
            (
                // tag.title()/artist()/album()/genre()/comment() return Cow<str>; use to_string()
                // (Cow::to_owned returns Cow). get_string() returns &str → to_owned().
                tag.title().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).unwrap_or_else(file_name),
                tag.artist().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
                tag.get_string(ItemKey::AlbumArtist).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
                tag.album().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
                tag.genre().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
                tag.track().map(|t| i32::try_from(t).unwrap_or(i32::MAX)),
                tag.disk().map(|d| i32::try_from(d).unwrap_or(i32::MAX)),
                tag.date().map(|ts| i32::from(ts.year)),
                tag.get_string(ItemKey::Composer).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
                tag.comment().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
            )
        } else {
            (file_name(), None, None, None, None, None, None, None, None, None)
        };

    // Extract extended metadata from tags
    let (bpm, musicbrainz_track_id, musicbrainz_release_id, label, original_year,
         replaygain_track_gain, replaygain_track_peak, replaygain_album_gain, replaygain_album_peak) =
        if let Some(tag) = tag {
            (
                tag.get_string(ItemKey::Bpm).and_then(|s| s.parse::<f64>().ok()),
                tag.get_string(ItemKey::MusicBrainzRecordingId).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
                tag.get_string(ItemKey::MusicBrainzReleaseId).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
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
