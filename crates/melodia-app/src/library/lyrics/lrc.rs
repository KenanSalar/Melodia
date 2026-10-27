//! The LRC format, pure and IO-free, hand-rolled for `playlist_files::m3u`'s reason: the format is
//! small and settled.
//!
//! [`parse`] is total and tolerant. It takes a sidecar, a lyrics tag or an API field without being
//! told which, so no caller asks whether text "is LRC" first; whether it came back timed is
//! [`Lyrics::is_synced`]'s question.

use std::borrow::Cow;

use melodia_core::entities::lyrics::{LyricLine, Lyrics, LyricsSource, split_gloss};

/// The identification tags the format defines, dropped as metadata. Matched only against a whole
/// bracketed line with a known key, so a plain sheet keeps its `[Chorus]` markers.
const ID_TAG_KEYS: [&str; 9] = ["al", "ar", "au", "by", "length", "offset", "re", "ti", "ve"];

/// Parses a sheet, or `None` where there is nothing to show; no input is an error. A sheet with
/// any timed line keeps **only** its timed lines, so downstream a sheet is timed or it isn't.
#[must_use]
pub fn parse(text: &str, source: LyricsSource) -> Option<Lyrics> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let offset = offset_ms(text);

    let mut timed: Vec<LyricLine> = Vec::new();
    let mut plain: Vec<LyricLine> = Vec::new();

    for line in text.lines() {
        let (stamps, rest) = split_stamps(line);
        if stamps.is_empty() {
            if !is_id_tag(line) {
                let (sung, gloss) = split_translation(line.trim_end());
                plain.push(LyricLine {
                    at_ms: None,
                    end_ms: None,
                    text: sung,
                    romanization: None,
                    translation: gloss,
                });
            }
            continue;
        }
        let stripped = strip_word_stamps(rest);
        let (mut sung, mut gloss) = split_translation(stripped.trim());
        // Moved into the last stamp: nearly every line carries exactly one, where a clone would be
        // the whole cost.
        let last = stamps.len() - 1;
        for (index, at) in stamps.into_iter().enumerate() {
            let (text, translation) = if index == last {
                (std::mem::take(&mut sung), gloss.take())
            } else {
                (sung.clone(), gloss.clone())
            };
            timed.push(LyricLine {
                at_ms: Some(at.saturating_sub(offset).max(0)),
                end_ms: None,
                text,
                romanization: None,
                translation,
            });
        }
    }

    if timed.is_empty() {
        return Lyrics::new(plain, source);
    }
    // Stable, so a line written twice against one stamp keeps the order its author chose.
    timed.sort_by_key(|line| line.at_ms);
    Lyrics::new(close_at_blanks(timed), source)
}

/// Turns each blank stamp, the format's "stop showing the last line", into the end of the line
/// above it rather than an empty row the panel centres on through the solo. The first blank of a
/// run wins, and one before any words is dropped.
fn close_at_blanks(lines: Vec<LyricLine>) -> Vec<LyricLine> {
    let mut sung: Vec<LyricLine> = Vec::with_capacity(lines.len());
    for line in lines {
        if !line.text.trim().is_empty() {
            sung.push(line);
            continue;
        }
        if let Some(previous) = sung.last_mut()
            && previous.end_ms.is_none()
        {
            previous.end_ms = line.at_ms;
        }
    }
    sung
}

/// Whether `text` carries timings, read as [`parse`] reads it but stopping at the first stamped
/// line rather than building the sheet.
#[must_use]
pub(super) fn is_timed(text: &str) -> bool {
    text.lines().any(|line| !split_stamps(line).0.is_empty())
}

fn split_translation(line: &str) -> (String, Option<String>) {
    let (sung, gloss) = split_gloss(line);
    (sung.to_owned(), gloss.map(str::to_owned))
}

/// The `[offset:±ms]` tag, or zero. A **positive** offset means the sheet runs early, so it comes
/// off the stamps rather than onto them.
fn offset_ms(text: &str) -> i64 {
    text.lines()
        .filter_map(bracketed)
        .filter_map(|inner| inner.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("offset"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or(0)
}

/// The inside of a line that is nothing but one bracketed tag.
fn bracketed(line: &str) -> Option<&str> {
    line.trim().strip_prefix('[')?.strip_suffix(']')
}

fn is_id_tag(line: &str) -> bool {
    let Some(inner) = bracketed(line) else {
        return false;
    };
    let Some((key, _)) = inner.split_once(':') else {
        return false;
    };
    ID_TAG_KEYS.iter().any(|known| known.eq_ignore_ascii_case(key.trim()))
}

/// Peels the leading `[mm:ss.xx]` stamps off a line, returning them and whatever follows. Several
/// is how the format spells a repeated chorus.
fn split_stamps(line: &str) -> (Vec<i64>, &str) {
    let mut rest = line.trim_start();
    let mut stamps = Vec::new();
    while let Some(body) = rest.strip_prefix('[')
        && let Some((inner, tail)) = body.split_once(']')
        && let Some(at) = stamp_ms(inner)
    {
        stamps.push(at);
        rest = tail.trim_start();
    }
    (stamps, rest)
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` in milliseconds, taking `,` as a decimal point too. `None`
/// for anything else, which is what keeps an identification tag from reading as a stamp.
fn stamp_ms(inner: &str) -> Option<i64> {
    let (minutes, rest) = inner.split_once(':')?;
    let (seconds, fraction) = rest.split_once(['.', ',']).unwrap_or((rest, ""));
    Some((digits(minutes)? * 60 + digits(seconds)?) * 1000 + fraction_ms(fraction)?)
}

/// A field that is nothing but ASCII digits, refusing the sign `i64::from_str` would take.
fn digits(field: &str) -> Option<i64> {
    let field = field.trim();
    if field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}

/// The fractional field in milliseconds, so `.5`, `.50` and `.500` agree.
fn fraction_ms(fraction: &str) -> Option<i64> {
    const FIRST_PLACE: i64 = 100;

    let mut ms = 0;
    let mut place = FIRST_PLACE;
    for byte in fraction.trim().bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        // Past milliseconds `place` is zero, so a fourth digit is dropped rather than refused.
        ms += i64::from(byte - b'0') * place;
        place /= 10;
    }
    Some(ms)
}

/// Drops enhanced-LRC word stamps (`<mm:ss.xx>`), which nothing draws yet. A `<` that is not a
/// stamp stays put, so a line with `<3` in it survives.
fn strip_word_stamps(line: &str) -> Cow<'_, str> {
    if !line.contains('<') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else { break };
        if stamp_ms(&after[..close]).is_none() {
            out.push_str(&rest[..=open]);
            rest = after;
            continue;
        }
        out.push_str(&rest[..open]);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
#[path = "tests/lrc_tests.rs"]
mod tests;
