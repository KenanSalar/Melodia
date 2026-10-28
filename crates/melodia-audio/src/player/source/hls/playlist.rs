//! The two playlist shapes HLS uses, told apart from the pointer `.m3u` that names one Icecast
//! mount. Pure text in, URLs resolved against the playlist they came from out.

use std::time::Duration;

use reqwest::Url;

use melodia_core::error::AppError;

/// Mandatory in a media playlist, so its presence is what separates one from a plain Extended M3U.
const TARGET_DURATION_TAG: &str = "#EXT-X-TARGETDURATION:";
/// Mandatory in a master playlist. The variant's URI is the next line that is not a tag.
const STREAM_INF_TAG: &str = "#EXT-X-STREAM-INF:";
/// A rendition group. The audio one carries its playlist in an attribute rather than on the next
/// line, which is what makes it reachable without opening a rung of the video ladder beside it.
const MEDIA_TAG: &str = "#EXT-X-MEDIA:";
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

/// A rung of the variant ladder, or the playlist an audio rendition group points at.
pub struct Variant {
    pub url: Url,
    /// Peak bits per second, `0` where none was named, which a rendition group never names.
    pub bandwidth: u64,
    /// A simulcast whose audio would cost a video stream to reach.
    pub has_video: bool,
}

/// A segment playlist as it stood when fetched.
pub struct MediaPlaylist {
    /// How long to wait before asking for it again.
    pub target_duration: Duration,
    /// The sequence number of `segments[0]`. Everything after it counts up by one.
    pub media_sequence: u64,
    pub segments: Vec<Url>,
    /// The header a fragmented-MP4 stream's segments are meaningless without, sent once ahead of
    /// the first and never counted toward [`Self::media_sequence`].
    pub init_segment: Option<Url>,
    /// A playlist that will not grow again, which for a live station never happens.
    pub ended: bool,
}

pub enum Playlist {
    Master(Vec<Variant>),
    Media(MediaPlaylist),
}

/// Keyed on the two tags the spec makes mandatory, one per playlist type, so a plain `#EXTM3U`
/// naming one mount stays on the pointer path.
pub fn is_hls(body: &str) -> bool {
    lines(body)
        .any(|line| line.starts_with(TARGET_DURATION_TAG) || line.starts_with(STREAM_INF_TAG))
}

/// Parse `body`, resolving every URI it names against `base`.
pub fn parse(body: &str, base: &Url) -> Result<Playlist, AppError> {
    let variants = parse_renditions(body, base);
    if variants.is_empty() {
        parse_media(body, base).map(Playlist::Media)
    } else {
        Ok(Playlist::Master(variants))
    }
}

/// The richest audio-only rendition, an audio group counting as one and being the only one on a
/// television simulcast. **Where every rendition carries a picture, the pick inverts to the
/// poorest**: the audio is the same on every rung, so the richest would spend megabits on a picture
/// nothing here draws.
pub fn pick_variant(mut variants: Vec<Variant>) -> Option<Variant> {
    if variants.iter().any(|variant| !variant.has_video) {
        variants.retain(|variant| !variant.has_video);
        return variants.into_iter().max_by_key(|variant| variant.bandwidth);
    }
    // A rung stating no `BANDWIDTH` reads as zero, which a poorest-wins pick would always take;
    // the attribute is mandatory, so those are malformed.
    if variants.iter().any(|variant| variant.bandwidth > 0) {
        variants.retain(|variant| variant.bandwidth > 0);
    }
    variants.into_iter().min_by_key(|variant| variant.bandwidth)
}

fn parse_renditions(body: &str, base: &Url) -> Vec<Variant> {
    let mut variants = Vec::new();
    let mut pending: Option<(u64, bool)> = None;

    for line in lines(body) {
        if let Some(list) = line.strip_prefix(STREAM_INF_TAG) {
            pending = Some(stream_inf(list));
        } else if let Some(list) = line.strip_prefix(MEDIA_TAG) {
            if let Some(url) = audio_rendition(list, base) {
                variants.push(Variant { url, bandwidth: 0, has_video: false });
            }
        } else if !line.starts_with('#')
            && let Some((bandwidth, has_video)) = pending.take()
            && let Some(url) = joined(base, line)
        {
            variants.push(Variant { url, bandwidth, has_video });
        }
    }
    variants
}

/// Three attributes answer whether a rung has video, `CODECS` being only a *should*: a master
/// omitting it while sizing its rungs in pixels would otherwise read as a row of audio streams.
fn stream_inf(list: &str) -> (u64, bool) {
    let mut bandwidth = 0;
    let mut has_video = false;
    for (name, value) in attributes(list) {
        match name {
            "BANDWIDTH" => bandwidth = value.parse().unwrap_or(0),
            "CODECS" => has_video |= names_video(value),
            "RESOLUTION" | "VIDEO" => has_video |= !value.is_empty(),
            _ => {}
        }
    }
    (bandwidth, has_video)
}

/// `None` for every other kind of group, and for audio muxed into the rungs.
fn audio_rendition(list: &str, base: &Url) -> Option<Url> {
    let mut is_audio = false;
    let mut uri = None;
    for (name, value) in attributes(list) {
        match name {
            "TYPE" => is_audio = value.eq_ignore_ascii_case("AUDIO"),
            "URI" => uri = Some(value),
            _ => {}
        }
    }
    if !is_audio {
        return None;
    }
    joined(base, uri?)
}

/// Refuses a blank `URI`, which joined would resolve to the playlist's own address, pointing a
/// rendition back at its master or handing the demuxer the playlist text as an init segment.
///
/// **Every URI in a playlist is resolved here**, and `Url::join` hands an absolute `file://` or
/// `data:` back unchanged, so [`services::net::is_http`] is asked of the parsed value.
///
/// [`services::net::is_http`]: melodia_net::services::net::is_http
fn joined(base: &Url, uri: &str) -> Option<Url> {
    if uri.is_empty() {
        return None;
    }
    base.join(uri).ok().filter(melodia_net::services::net::is_http)
}

/// Split on the commas outside a quoted value, which `CODECS="mp4a.40.2,avc1.4d401f"` needs.
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
    const VIDEO_PREFIXES: [&str; 8] =
        ["avc1", "avc3", "hvc1", "hev1", "dvh1", "dvhe", "av01", "vp09"];
    codecs
        .split(',')
        .map(str::trim)
        .any(|codec| VIDEO_PREFIXES.iter().any(|prefix| codec.starts_with(prefix)))
}

fn parse_media(body: &str, base: &Url) -> Result<MediaPlaylist, AppError> {
    let mut target_duration = None;
    let mut media_sequence = 0;
    let mut segments = Vec::new();
    let mut init_segment = None;
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
        } else if let Some(attrs) = line.strip_prefix(MAP_TAG) {
            // A `BYTERANGE` on the init is not honoured: the two would have to be requested
            // together to mean anything, and no station in the directory sends one.
            init_segment = attributes(attrs)
                .find(|(name, _)| *name == "URI")
                .and_then(|(_, uri)| joined(base, uri));
        } else if line == ENDLIST_TAG {
            ended = true;
        } else if !line.starts_with('#')
            && let Some(url) = joined(base, line)
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
        init_segment,
        ended,
    })
}

fn is_encrypted(attrs: &str) -> bool {
    attributes(attrs)
        .find(|(name, _)| *name == "METHOD")
        .is_some_and(|(_, method)| !method.eq_ignore_ascii_case(NO_ENCRYPTION))
}

/// Trimmed, blanks dropped, and the byte-order mark some servers lead with taken off.
fn lines(body: &str) -> impl Iterator<Item = &str> {
    body.strip_prefix('\u{FEFF}')
        .unwrap_or(body)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
}

#[cfg(test)]
#[path = "tests/playlist_tests.rs"]
mod tests;
