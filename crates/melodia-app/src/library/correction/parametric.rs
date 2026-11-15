//! The parametric format `AutoEq` writes, which is Equalizer APO's: a `Preamp:` line, then a line
//! per filter. Keywords match in any case, so a hand-written file needn't copy `AutoEq`'s.

use melodia_core::entities::correction::{
    CorrectionFilter, CorrectionFilters, FilterKind, ParsedProfile, ProfileRefusal, SkipReason,
    SkippedLine,
};

/// Some Windows editors write one at the head of a UTF-8 file.
const BYTE_ORDER_MARK: char = '\u{feff}';

/// The peaking filter's three spellings.
const PEAKING: [&str; 3] = ["PK", "PEQ", "Modal"];

enum Line {
    Ignored,
    Filter(CorrectionFilter),
    Skipped(SkipReason),
    OneEar,
}

/// Every filter the correction can run, and every other line that says something with its reason.
pub(super) fn parse(text: &str) -> Result<ParsedProfile, ProfileRefusal> {
    let text = text.strip_prefix(BYTE_ORDER_MARK).unwrap_or(text);
    let mut filters = Vec::new();
    let mut skipped = Vec::new();
    for (index, line) in text.lines().map(str::trim).enumerate() {
        match read_line(line) {
            Line::Ignored => {}
            Line::Filter(filter) => filters.push(filter),
            Line::Skipped(reason) => {
                skipped.push(SkippedLine { number: index + 1, text: line.into(), reason });
            }
            Line::OneEar => return Err(ProfileRefusal::OneEar),
        }
    }
    Ok(ParsedProfile {
        filters: CorrectionFilters::try_from(filters)?,
        skipped: skipped.into_boxed_slice(),
    })
}

/// The file's `Preamp:` is ignored: the headroom comes off the curve the correction plays, the
/// sliders included.
fn read_line(line: &str) -> Line {
    if line.is_empty() || line.starts_with('#') {
        return Line::Ignored;
    }
    let Some((command, args)) = line.split_once(':') else {
        return Line::Skipped(SkipReason::Unreadable);
    };
    let command = command.trim();
    if is(command, "Preamp") {
        return Line::Ignored;
    }
    if is(command, "Channel") {
        return if is(args.trim(), "all") { Line::Ignored } else { Line::OneEar };
    }
    if !is_filter_command(command) {
        return Line::Skipped(SkipReason::Unreadable);
    }
    match read_filter(args) {
        Ok(filter) => Line::Filter(filter),
        Err(reason) => Line::Skipped(reason),
    }
}

/// `Filter`, bare or numbered.
fn is_filter_command(command: &str) -> bool {
    const FILTER: &str = "Filter";
    let Some(head) = command.get(..FILTER.len()) else {
        return false;
    };
    let number = command[FILTER.len()..].trim_start();
    is(head, FILTER) && number.bytes().all(|byte| byte.is_ascii_digit())
}

/// `ON <type> Fc <hz> Hz Gain <db> dB Q <q>`.
fn read_filter(args: &str) -> Result<CorrectionFilter, SkipReason> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let [state, rest @ ..] = tokens.as_slice() else {
        return Err(SkipReason::Unreadable);
    };
    if is(state, "OFF") {
        return Err(SkipReason::Off);
    }
    if !is(state, "ON") {
        return Err(SkipReason::Unreadable);
    }
    let [kind, shape @ ..] = rest else {
        return Err(SkipReason::Unreadable);
    };
    let kind = filter_kind(kind).ok_or(SkipReason::Unsupported)?;
    let filter = match shape {
        [fc_word, fc, hz, gain_word, gain, db, q_word, q]
            if is(fc_word, "Fc")
                && is(hz, "Hz")
                && is(gain_word, "Gain")
                && is(db, "dB")
                && is(q_word, "Q") =>
        {
            CorrectionFilter { kind, freq_hz: number(fc)?, gain_db: number(gain)?, q: number(q)? }
        }
        _ => return Err(unread_shape(kind, shape)),
    };
    filter.check().map_err(SkipReason::OutOfRange)?;
    Ok(filter)
}

fn filter_kind(token: &str) -> Option<FilterKind> {
    if PEAKING.iter().any(|spelling| is(token, spelling)) {
        return Some(FilterKind::Peaking);
    }
    if is(token, "LSC") {
        return Some(FilterKind::LowShelf);
    }
    is(token, "HSC").then_some(FilterKind::HighShelf)
}

/// A shelf's slope where `Fc` is due, or a width in octaves, names a shape the correction has no
/// stage for, where anything else is a line it can't read.
fn unread_shape(kind: FilterKind, shape: &[&str]) -> SkipReason {
    let slope = kind != FilterKind::Peaking && shape.first().is_some_and(|token| !is(token, "Fc"));
    let octaves = shape.iter().any(|token| is(token, "BW"));
    if slope || octaves { SkipReason::Unsupported } else { SkipReason::Unreadable }
}

/// Parses `inf` and `NaN` too, which the filter's own check refuses with the value named.
fn number(token: &str) -> Result<f64, SkipReason> {
    token.parse().map_err(|_| SkipReason::Unreadable)
}

fn is(token: &str, keyword: &str) -> bool {
    token.eq_ignore_ascii_case(keyword)
}
