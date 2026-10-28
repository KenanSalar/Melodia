//! Extended-M3U8 text, IO-free. The parser ignores unknown `#` lines and a leading BOM, so other
//! players' files import too.
//!
//! Our exports add a `#MELODIA-HASH:<blake3-hex>` line per track, which other players skip as a
//! comment, so a restored playlist re-matches a track whose file has moved.

use melodia_core::entities::track::PlaylistExportRow;

const HEADER: &str = "#EXTM3U";
const PLAYLIST_TAG: &str = "#PLAYLIST:";
const EXTINF_TAG: &str = "#EXTINF:";
const HASH_TAG: &str = "#MELODIA-HASH:";

/// A path line plus the tags above it. Every field but `path` is best-effort, foreign files
/// omitting any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEntry {
    /// Verbatim: absolute or relative, native or foreign.
    pub path: String,
    /// Lowercased.
    pub hash: Option<String>,
    /// Informational; `None` where missing, unparseable or negative.
    pub duration_secs: Option<i64>,
    /// "Artist - Title", for diagnostics only.
    pub display: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedM3u {
    pub playlist_name: Option<String>,
    pub entries: Vec<ParsedEntry>,
}

/// Replace CR/LF with a space, so a stray newline can't break the single-line tag format.
fn one_line(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains(['\r', '\n']) {
        std::borrow::Cow::Owned(s.replace(['\r', '\n'], " "))
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// `\n` endings, a trailing newline, UTF-8 without a BOM.
pub fn serialize(playlist_name: &str, tracks: &[PlaylistExportRow]) -> String {
    // About three lines of 80 bytes per track.
    let mut out = String::with_capacity(64 + playlist_name.len() + tracks.len() * 240);
    out.push_str(HEADER);
    out.push('\n');
    out.push_str(PLAYLIST_TAG);
    out.push_str(&one_line(playlist_name));
    out.push('\n');

    for t in tracks {
        let secs = if t.duration_ms > 0 { (t.duration_ms + 500) / 1000 } else { -1 };
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

/// `(duration_secs, display)` from the part after `#EXTINF:`. The duration's leading integer
/// survives a float or trailing attributes (`213 tvg-id="x",Title`).
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

/// Never errors: an unrecognised file yields no entries, and the caller decides whether that is
/// fatal.
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
