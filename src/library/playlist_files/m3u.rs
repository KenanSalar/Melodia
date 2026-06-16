//! Extended-M3U8 serialization and parsing.
//!
//! Pure, IO-free: [`serialize`] turns a playlist name + ordered tracks into
//! Extended-M3U8 text, [`parse`] turns text back into a [`ParsedM3u`]. The
//! parser is deliberately tolerant — it ignores unknown `#` comment lines and
//! a leading UTF-8 BOM so files exported by other players (VLC, foobar2000)
//! import too.
//!
//! Melodia's own exports add one non-standard line per track,
//! `#MELODIA-HASH:<blake3-hex>`, which other players ignore as a comment.
//! That hash is what lets a restored playlist re-match a track even after its
//! file was moved.

use crate::entities::track::PlaylistExportRow;

const HEADER: &str = "#EXTM3U";
const PLAYLIST_TAG: &str = "#PLAYLIST:";
const EXTINF_TAG: &str = "#EXTINF:";
const HASH_TAG: &str = "#MELODIA-HASH:";

/// One parsed playlist entry: a path line plus the comment tags that
/// preceded it. Every field except `path` is best-effort — foreign files
/// may omit any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEntry {
    /// The path line, verbatim (absolute/relative, native or foreign).
    pub path: String,
    /// BLAKE3 hex from `#MELODIA-HASH` (lowercased), if present.
    pub hash: Option<String>,
    /// Whole-second duration from `#EXTINF` (informational; `None` if
    /// missing, unparseable, or negative/unknown).
    pub duration_secs: Option<i64>,
    /// "Artist - Title" display string from `#EXTINF` (diagnostics only).
    pub display: Option<String>,
}

/// A parsed Extended-M3U8 file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedM3u {
    /// Playlist name from `#PLAYLIST:`, if the file carried one.
    pub playlist_name: Option<String>,
    pub entries: Vec<ParsedEntry>,
}

/// Replace CR/LF with a space so a stray newline in a title/name can't
/// break the single-line tag format.
fn one_line(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains(['\r', '\n']) {
        std::borrow::Cow::Owned(s.replace(['\r', '\n'], " "))
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// Serialize a playlist (name + ordered tracks) to Extended-M3U8 text.
///
/// Emits `\n` line endings and a trailing newline, UTF-8 without a BOM.
/// `#EXTINF` duration is `duration_ms` rounded to whole seconds (`-1` when
/// unknown); the title field is `"<artist> - <title>"`, or just `"<title>"`
/// when the track has no artist. The `#MELODIA-HASH` line is omitted for
/// tracks with no `file_hash`.
pub fn serialize(playlist_name: &str, tracks: &[PlaylistExportRow]) -> String {
    // Rough capacity: header + name + ~3 lines * ~80 bytes per track.
    let mut out = String::with_capacity(64 + playlist_name.len() + tracks.len() * 240);
    out.push_str(HEADER);
    out.push('\n');
    out.push_str(PLAYLIST_TAG);
    out.push_str(&one_line(playlist_name));
    out.push('\n');

    for t in tracks {
        let secs = if t.duration_ms > 0 {
            (t.duration_ms + 500) / 1000
        } else {
            -1
        };
        out.push_str(EXTINF_TAG);
        out.push_str(&secs.to_string());
        out.push(',');
        match &t.artist {
            Some(a) if !a.is_empty() => {
                out.push_str(&one_line(a));
                out.push_str(" - ");
                out.push_str(&one_line(&t.title));
            }
            _ => out.push_str(&one_line(&t.title)),
        }
        out.push('\n');

        if let Some(h) = &t.file_hash
            && !h.is_empty()
        {
            out.push_str(HASH_TAG);
            out.push_str(&one_line(h));
            out.push('\n');
        }

        out.push_str(&one_line(&t.file_path));
        out.push('\n');
    }

    out
}

/// Parse `#EXTINF:<duration>,<display>` content (the part after the tag).
/// Returns `(duration_secs, display)`. Duration tolerates floats and
/// trailing attributes (`#EXTINF:213 tvg-id="x",Title`) by taking the
/// leading integer part; negative/unknown (`-1`) maps to `None`.
fn parse_extinf(rest: &str) -> (Option<i64>, Option<String>) {
    let (dur_str, disp) = match rest.split_once(',') {
        Some((d, t)) => (d, Some(t.trim())),
        None => (rest, None),
    };
    let duration_secs = dur_str
        .split_whitespace()
        .next()
        .and_then(|tok| tok.split('.').next())
        .and_then(|int_part| int_part.parse::<i64>().ok())
        .filter(|&d| d >= 0);
    let display = disp.filter(|d| !d.is_empty()).map(ToOwned::to_owned);
    (duration_secs, display)
}

/// Parse Extended-M3U8 text tolerantly. Strips a leading UTF-8 BOM, ignores
/// blank lines and unknown `#` comments; a non-comment line is a path and
/// flushes the pending `#EXTINF`/`#MELODIA-HASH` tags into one
/// [`ParsedEntry`]. Never errors — an unrecognised file just yields zero
/// entries and the caller decides whether that is fatal.
pub fn parse(content: &str) -> ParsedM3u {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);

    let mut result = ParsedM3u::default();
    let mut pending_hash: Option<String> = None;
    let mut pending_dur: Option<i64> = None;
    let mut pending_display: Option<String> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('#') {
            if let Some(name) = line.strip_prefix(PLAYLIST_TAG) {
                let name = name.trim();
                if !name.is_empty() {
                    result.playlist_name = Some(name.to_owned());
                }
            } else if let Some(rest) = line.strip_prefix(EXTINF_TAG) {
                let (dur, disp) = parse_extinf(rest);
                pending_dur = dur;
                pending_display = disp;
            } else if let Some(rest) = line.strip_prefix(HASH_TAG) {
                let h = rest.trim();
                if !h.is_empty() {
                    pending_hash = Some(h.to_ascii_lowercase());
                }
            }
            // Any other `#...` line (incl. `#EXTM3U`) is ignored.
            continue;
        }

        result.entries.push(ParsedEntry {
            path: line.to_owned(),
            hash: pending_hash.take(),
            duration_secs: pending_dur.take(),
            display: pending_display.take(),
        });
    }

    result
}

#[cfg(test)]
#[path = "tests/m3u_tests.rs"]
mod tests;
