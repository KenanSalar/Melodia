//! The LRC format, pure and IO-free, hand-rolled for `playlist_files::m3u`'s reason: the format is
//! small and settled.
//!
//! [`parse`] is total and tolerant. It takes a sidecar, a lyrics tag or an API field without being
//! told which, so no caller asks whether text "is LRC" first; whether it came back timed is
//! [`Lyrics::is_synced`]'s question. Enhanced LRC's word stamps are read with their lines, and a
//! line whose word times don't add up lights whole rather than failing its sheet.

use std::borrow::Cow;

use melodia_core::entities::lyrics::{
    LyricLine, Lyrics, LyricsSource, TRANSLATION_MARK, WordStamp, sheet_lines, split_gloss,
};

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

    for line in sheet_lines(text) {
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
                    words: Vec::new(),
                });
            }
            continue;
        }
        let mut written = Written::read(rest, &stamps);
        // Moved into the last stamp: nearly every line carries exactly one, where a clone would be
        // the whole cost.
        let last = stamps.len() - 1;
        for (index, at) in stamps.into_iter().enumerate() {
            let copy = if index == last { std::mem::take(&mut written) } else { written.clone() };
            timed.push(copy.sung_at(at, offset));
        }
    }

    if timed.is_empty() {
        return Lyrics::new(plain, source);
    }
    // Stable, so a line written twice against one stamp keeps the order its author chose.
    timed.sort_by_key(|line| line.at_ms);
    drop_overrunning_words(&mut timed);
    Lyrics::new(close_at_blanks(timed), source)
}

/// A stamped line as written, which each stamp it is sung under takes a copy of.
#[derive(Clone, Default)]
struct Written {
    text: String,
    translation: Option<String>,
    pace: Option<Pace>,
}

impl Written {
    fn read(rest: &str, line_stamps: &[i64]) -> Self {
        let (spoken, stamps) = split_word_stamps(rest);
        let (text, translation) = split_translation(&spoken);
        let caret = translation.as_ref().and(spoken.find(TRANSLATION_MARK));
        let (words, end_ms) = place(stamps, text.len(), caret);
        let pace = Pace::new(words, end_ms, line_stamps);
        Self { text, translation, pace }
    }

    fn sung_at(self, at: i64, offset: i64) -> LyricLine {
        let (words, end_ms) = self.pace.map_or_default(|pace| pace.sung_at(at, offset));
        LyricLine {
            at_ms: Some(on_track(at, offset)),
            end_ms,
            text: self.text,
            romanization: None,
            translation: self.translation,
            words,
        }
    }
}

/// A line's word times as written, and the line stamp they were timed against: a copy under
/// another stamp moves them by its distance from that one, so a chorus lights on every repeat.
#[derive(Clone)]
struct Pace {
    anchor: i64,
    words: Vec<WordStamp>,
    end_ms: Option<i64>,
}

impl Pace {
    /// `None`, the line lit whole and its end dropped, where it has no word stamps, they run out of
    /// order, or no line stamp comes at or before the first to time them against.
    fn new(words: Vec<WordStamp>, end_ms: Option<i64>, line_stamps: &[i64]) -> Option<Self> {
        let first = words.first()?.at_ms;
        let anchor = line_stamps.iter().copied().filter(|&at| at <= first).max()?;
        let in_order = words.iter().map(|word| word.at_ms).chain(end_ms).is_sorted();
        in_order.then_some(Self { anchor, words, end_ms })
    }

    fn sung_at(mut self, at: i64, offset: i64) -> (Vec<WordStamp>, Option<i64>) {
        let shift = at - self.anchor;
        for word in &mut self.words {
            word.at_ms = on_track(word.at_ms.saturating_add(shift), offset);
        }
        (self.words, self.end_ms.map(|end| on_track(end.saturating_add(shift), offset)))
    }
}

/// A stamp with `[offset:]` applied, held at the track's start.
fn on_track(stamp: i64, offset: i64) -> i64 {
    stamp.saturating_sub(offset).max(0)
}

/// Lights whole any line whose words or end run past the sheet's next later stamp, a blank one
/// included. Ahead of [`close_at_blanks`], so an `end_ms` here is the line's own end stamp.
fn drop_overrunning_words(lines: &mut [LyricLine]) {
    let mut next_later = None;
    let mut run_at = None;
    for line in lines.iter_mut().rev() {
        if line.at_ms != run_at {
            next_later = run_at;
            run_at = line.at_ms;
        }
        let sung_until = line.end_ms.or(line.words.last().map(|word| word.at_ms));
        if let (Some(until), Some(bound)) = (sung_until, next_later)
            && until > bound
        {
            line.words = Vec::new();
            line.end_ms = None;
        }
    }
}

/// Turns each blank stamp, the format's "stop showing the last line", into the end of the line
/// above it rather than an empty row the panel centres on through the solo, unless an end stamp
/// already ended it. The first blank of a run wins, and one before any words is dropped.
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
    sheet_lines(text).any(|line| !split_stamps(line).0.is_empty())
}

fn split_translation(line: &str) -> (String, Option<String>) {
    let (sung, gloss) = split_gloss(line);
    (sung.to_owned(), gloss.map(str::to_owned))
}

/// The `[offset:±ms]` tag, or zero. A **positive** offset means the sheet runs early, so it comes
/// off the stamps rather than onto them.
fn offset_ms(text: &str) -> i64 {
    sheet_lines(text)
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
    let whole_seconds = digits(minutes)?.checked_mul(60)?.checked_add(digits(seconds)?)?;
    whole_seconds.checked_mul(1000)?.checked_add(fraction_ms(fraction)?)
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

/// Takes the enhanced-LRC word stamps (`<mm:ss.xx>`) out of a line, each kept with the byte its
/// word starts at in the trimmed text left. A seam keeps one of its own whitespace characters, a
/// full-width space included, and none where it had none, so a syllable stamp joins its word. A `<`
/// that is not a stamp stays put, so `<3` survives.
fn split_word_stamps(line: &str) -> (Cow<'_, str>, Vec<WordStamp>) {
    let line = line.trim();
    if !line.contains('<') {
        return (Cow::Borrowed(line), Vec::new());
    }
    let mut out = String::with_capacity(line.len());
    let mut stamps = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else { break };
        let Some(at_ms) = stamp_ms(&after[..close]) else {
            out.push_str(&rest[..=open]);
            rest = after;
            continue;
        };
        out.push_str(&rest[..open]);
        rest = &after[close + 1..];
        let before = out.chars().next_back().filter(|c| c.is_whitespace());
        let seam = before.or_else(|| rest.chars().next().filter(|c| c.is_whitespace()));
        out.truncate(out.trim_end().len());
        rest = rest.trim_start();
        if let Some(seam) = seam
            && !out.is_empty()
        {
            out.push(seam);
        }
        stamps.push(WordStamp { start: out.len(), at_ms });
    }
    out.push_str(rest);
    out.truncate(out.trim_end().len());
    (Cow::Owned(out), stamps)
}

/// Splits a line's word stamps between its words and its end: the last stamp with no word after
/// it ends the line, and one past the gloss's `caret` is dropped, the gloss lighting whole.
fn place(
    mut stamps: Vec<WordStamp>,
    sung_len: usize,
    caret: Option<usize>,
) -> (Vec<WordStamp>, Option<i64>) {
    // `split_word_stamps` hands the starts in order, which both searches need.
    let sung_half = stamps.partition_point(|stamp| caret.is_none_or(|caret| stamp.start <= caret));
    let words = stamps.partition_point(|stamp| stamp.start < sung_len);
    let end_ms = stamps[words..sung_half].last().map(|stamp| stamp.at_ms);
    stamps.truncate(words);
    (stamps, end_ms)
}

#[cfg(test)]
#[path = "tests/lrc_tests.rs"]
mod tests;
