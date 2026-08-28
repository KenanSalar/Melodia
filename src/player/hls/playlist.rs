//! The two playlist shapes HLS uses, and telling both apart from the pointer `.m3u` a station
//! serves to name a single Icecast mount.
//!
//! Pure text in, resolved URLs out. Every URI is joined against the playlist it came from, which
//! is the half `stream_source::first_stream_url` cannot do and the reason relative segment names
//! reach nothing today.

use std::time::Duration;

use reqwest::Url;

use crate::error::AppError;

/// Mandatory in a media playlist, so its presence is what separates one from a plain Extended M3U.
const TARGET_DURATION_TAG: &str = "#EXT-X-TARGETDURATION:";
/// Mandatory in a master playlist. The variant's URI is the next line that is not a tag.
const STREAM_INF_TAG: &str = "#EXT-X-STREAM-INF:";
const MEDIA_SEQUENCE_TAG: &str = "#EXT-X-MEDIA-SEQUENCE:";
const KEY_TAG: &str = "#EXT-X-KEY:";
const MAP_TAG: &str = "#EXT-X-MAP:";
const ENDLIST_TAG: &str = "#EXT-X-ENDLIST";

/// The `METHOD` an unencrypted playlist still sometimes spells out rather than omitting the tag.
const NO_ENCRYPTION: &str = "NONE";

/// Bounds on the reload period a playlist may ask for. The floor stops a malformed tiny value
/// turning the refresh into a request loop, the ceiling stops a large one parking the station.
const MIN_TARGET_SECS: f32 = 1.0;
const MAX_TARGET_SECS: f32 = 30.0;

/// A rendition named by a master playlist.
pub struct Variant {
    pub url: Url,
    /// Peak bits per second the master claims, `0` where it named none.
    pub bandwidth: u64,
    /// Whether `CODECS` names a picture track, which for a radio station means a simulcast whose
    /// audio would cost a video stream to reach.
    pub has_video: bool,
}

/// A segment playlist, as it stood at the moment it was fetched.
pub struct MediaPlaylist {
    /// How long to wait before asking for it again.
    pub target_duration: Duration,
    /// The sequence number of `segments[0]`. Everything after it counts up by one.
    pub media_sequence: u64,
    pub segments: Vec<Url>,
    /// A playlist that will not grow again, which for a live station never happens.
    pub ended: bool,
}

pub enum Playlist {
    Master(Vec<Variant>),
    Media(MediaPlaylist),
}

/// Whether this body is HLS rather than the pointer playlist most stations serve.
///
/// Keyed on the two tags the spec makes mandatory, one per playlist type, so a plain `#EXTM3U`
/// naming one Icecast mount stays on the path that already handles it.
pub fn is_hls(body: &str) -> bool {
    lines(body)
        .any(|line| line.starts_with(TARGET_DURATION_TAG) || line.starts_with(STREAM_INF_TAG))
}

/// Parse `body`, resolving every URI it names against `base`.
pub fn parse(body: &str, base: &Url) -> Result<Playlist, AppError> {
    let variants = parse_variants(body, base);
    if variants.is_empty() {
        parse_media(body, base).map(Playlist::Media)
    } else {
        Ok(Playlist::Master(variants))
    }
}

/// The rendition to play: an audio-only one where the master offers any, and the richest of
/// whatever is left.
pub fn pick_variant(mut variants: Vec<Variant>) -> Option<Variant> {
    if variants.iter().any(|variant| !variant.has_video) {
        variants.retain(|variant| !variant.has_video);
    }
    variants.into_iter().max_by_key(|variant| variant.bandwidth)
}

fn parse_variants(body: &str, base: &Url) -> Vec<Variant> {
    let mut variants = Vec::new();
    let mut pending: Option<(u64, bool)> = None;

    for line in lines(body) {
        if let Some(list) = line.strip_prefix(STREAM_INF_TAG) {
            pending = Some(stream_inf(list));
        } else if !line.starts_with('#')
            && let Some((bandwidth, has_video)) = pending.take()
            && let Ok(url) = base.join(line)
        {
            variants.push(Variant {
                url,
                bandwidth,
                has_video,
            });
        }
    }
    variants
}

/// The two attributes the variant pick reads: everything else a master carries describes a
/// picture we are not going to draw.
fn stream_inf(list: &str) -> (u64, bool) {
    let mut bandwidth = 0;
    let mut has_video = false;
    for (name, value) in attributes(list) {
        match name {
            "BANDWIDTH" => bandwidth = value.parse().unwrap_or(0),
            "CODECS" => has_video = names_video(value),
            _ => {}
        }
    }
    (bandwidth, has_video)
}

/// Split an attribute list on the commas that are not inside a quoted value, which is the one
/// thing `CODECS="mp4a.40.2,avc1.4d401f"` needs and a plain `split(',')` gets wrong.
fn attributes(list: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut quoted = false;
    list.split(move |c| {
        if c == '"' {
            quoted = !quoted;
        }
        c == ',' && !quoted
    })
    .filter_map(|pair| pair.split_once('='))
    .map(|(name, value)| (name.trim(), value.trim().trim_matches('"')))
}

fn names_video(codecs: &str) -> bool {
    const VIDEO_PREFIXES: [&str; 4] = ["avc1", "avc3", "hvc1", "hev1"];
    codecs
        .split(',')
        .map(str::trim)
        .any(|codec| VIDEO_PREFIXES.iter().any(|prefix| codec.starts_with(prefix)))
}

fn parse_media(body: &str, base: &Url) -> Result<MediaPlaylist, AppError> {
    let mut target_duration = None;
    let mut media_sequence = 0;
    let mut segments = Vec::new();
    let mut ended = false;

    for line in lines(body) {
        if let Some(value) = line.strip_prefix(TARGET_DURATION_TAG) {
            // `is_finite` is not belt-and-braces: `"inf"` parses, and `Duration::from_secs_f32`
            // panics on it.
            target_duration = value
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|secs| *secs > 0.0 && secs.is_finite())
                .map(|secs| secs.clamp(MIN_TARGET_SECS, MAX_TARGET_SECS));
        } else if let Some(value) = line.strip_prefix(MEDIA_SEQUENCE_TAG) {
            media_sequence = value.trim().parse().unwrap_or(0);
        } else if let Some(attrs) = line.strip_prefix(KEY_TAG) {
            if is_encrypted(attrs) {
                return Err(AppError::network_msg(
                    "This station's stream is encrypted and Melodia cannot play it",
                ));
            }
        } else if line.starts_with(MAP_TAG) {
            // Fragmented MP4. The demuxer for it is not in the stream path's feature set, and a
            // bare `.m4s` could not be opened on its own even if it were.
            return Err(AppError::network_msg(
                "This station's stream uses a segment format Melodia cannot read yet",
            ));
        } else if line == ENDLIST_TAG {
            ended = true;
        } else if !line.starts_with('#')
            && let Ok(url) = base.join(line)
        {
            segments.push(url);
        }
    }

    let target_duration = target_duration
        .ok_or_else(|| AppError::network_msg("Station playlist named no segment duration"))?;

    Ok(MediaPlaylist {
        target_duration: Duration::from_secs_f32(target_duration),
        media_sequence,
        segments,
        ended,
    })
}

fn is_encrypted(attrs: &str) -> bool {
    attributes(attrs)
        .find(|(name, _)| *name == "METHOD")
        .is_some_and(|(_, method)| !method.eq_ignore_ascii_case(NO_ENCRYPTION))
}

/// The playlist's meaningful lines: trimmed, blanks dropped, and the byte-order mark some servers
/// put in front of the `#EXTM3U` taken off the first one.
fn lines(body: &str) -> impl Iterator<Item = &str> {
    body.strip_prefix('\u{FEFF}')
        .unwrap_or(body)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
}
