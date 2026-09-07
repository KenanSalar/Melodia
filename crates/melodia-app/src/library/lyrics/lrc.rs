//! The LRC format, pure and IO-free.
//!
//! Hand-rolled rather than a crate, for `playlist_files::m3u`'s reason: the format is small and
//! settled, so a dependency would be carried forever against a parser that never needs to change.
//!
//! [`parse`] is total and tolerant. It takes a sidecar file, a lyrics tag or an API field without
//! being told which, and answers with a timed sheet, a plain one, or nothing. So no caller has to
//! ask whether some text "is LRC" before handing it over, and whether the answer came back timed
//! is [`Lyrics::is_synced`]'s question rather than a second one asked up front.

use std::borrow::Cow;

use melodia_core::entities::lyrics::{LyricLine, Lyrics, LyricsSource};

/// The identification tags the format defines. A line that is nothing but one of these is
/// metadata, and is dropped rather than sung.
///
/// Matched only against a whole bracketed line, which is what lets a plain sheet keep its
/// `[Chorus]` and `[Verse 2]` markers: those carry no colon and no known key, and a reader wants
/// them on the page.
const ID_TAG_KEYS: [&str; 9] = ["al", "ar", "au", "by", "length", "offset", "re", "ti", "ve"];

/// Parses a sheet, or `None` where there is nothing to show.
///
/// Tolerant the way the M3U reader is: anything that does not parse as a timestamp is either
/// metadata to drop or text to keep, and no input is an error.
///
/// A sheet carrying any timed line at all keeps **only** its timed lines. That is what makes
/// "a sheet is timed or it is not" true for everything downstream, rather than a hope.
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
                plain.push(LyricLine {
                    at_ms: None,
                    text: line.trim_end().to_owned(),
                });
            }
            continue;
        }
        let sung = strip_word_stamps(rest);
        let sung = sung.trim();
        timed.extend(stamps.into_iter().map(|at| LyricLine {
            at_ms: Some(at.saturating_sub(offset).max(0)),
            text: sung.to_owned(),
        }));
    }

    if timed.is_empty() {
        return Lyrics::new(plain, source);
    }
    // Stable, so a line written twice against one stamp keeps the order its author chose.
    timed.sort_by_key(|line| line.at_ms);
    Lyrics::new(timed, source)
}

/// The `[offset:±ms]` tag, or zero.
///
/// A **positive** offset means the sheet runs early, so it comes off the stamps rather than onto
/// them. It is the only correction a hand-timed sheet carries, so ignoring it would leave exactly
/// the sheets that needed a nudge running wrong.
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

/// Peels the leading `[mm:ss.xx]` stamps off a line, returning them and whatever follows.
///
/// A list rather than one stamp because that is how the format spells a repeated chorus, so a
/// parser taking only the first silently loses every repeat.
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

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx` in milliseconds, taking `,` as a decimal point too.
///
/// `None` for anything else, which is what drops every identification tag without a second rule
/// to keep in step with this one: a key that is not a number is not a time.
fn stamp_ms(inner: &str) -> Option<i64> {
    let (minutes, rest) = inner.split_once(':')?;
    let (seconds, fraction) = rest.split_once(['.', ',']).unwrap_or((rest, ""));
    Some((digits(minutes)? * 60 + digits(seconds)?) * 1000 + fraction_ms(fraction)?)
}

/// A field that is nothing but ASCII digits. Rejects the sign `i64::from_str` would take, a
/// negative minute being a stamp nothing could seek to.
fn digits(field: &str) -> Option<i64> {
    let field = field.trim();
    if field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    field.parse().ok()
}

/// The fractional field in milliseconds, so `.5`, `.50` and `.500` agree.
fn fraction_ms(fraction: &str) -> Option<i64> {
    /// What the first digit after the point is worth.
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

/// Drops enhanced-LRC word stamps (`<mm:ss.xx>`) from a line's text.
///
/// The timings are real and a karaoke sweep would want them, but nothing draws one yet and left
/// in the string they would be drawn as words. A `<` that is not a stamp stays put, so a line
/// with `<3` in it survives.
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
