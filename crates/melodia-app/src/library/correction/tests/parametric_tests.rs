//! The parametric format as `AutoEq` and Equalizer APO write it. A filter read wrong plays a curve
//! nobody asked for, so every line is either a filter, passed over, or listed with its reason.

use super::parse;
use melodia_core::entities::correction::{
    CorrectionFilter, FilterError, FilterKind, FilterSetError, ProfileRefusal, SkipReason,
    SkippedLine,
};

/// A filter every case below can stand beside, so a file isn't refused for holding none.
const FILTER_LINE: &str = "Filter 1: ON PK Fc 105 Hz Gain 6.5 dB Q 0.70";

const FILTER: CorrectionFilter =
    CorrectionFilter { kind: FilterKind::Peaking, freq_hz: 105.0, gain_db: 6.5, q: 0.7 };

fn filters_read(text: &str) -> Result<Vec<CorrectionFilter>, ProfileRefusal> {
    parse(text).map(|profile| profile.filters.filters().to_vec())
}

/// Each listed line's reason, in a file whose first line is [`FILTER_LINE`]. The numbering has a
/// test of its own.
fn listed_after_a_filter(line: &str) -> Result<Vec<SkipReason>, ProfileRefusal> {
    let profile = parse(&format!("{FILTER_LINE}\n{line}"))?;
    Ok(profile.skipped.iter().map(|listed| listed.reason).collect())
}

fn refusal(text: &str) -> Option<ProfileRefusal> {
    parse(text).err()
}

/// `PK`, `PEQ` and `Modal` are one peaking filter, and a hand-written file needn't copy `AutoEq`'s
/// case or number its filters.
#[test]
fn each_filter_spelling_reads_into_its_kind() {
    let cases = [
        ("Filter 1: ON PK Fc 105 Hz Gain 6.5 dB Q 0.70", FILTER),
        (
            "Filter: ON PEQ Fc 1000 Hz Gain -3.0 dB Q 1.41",
            CorrectionFilter {
                kind: FilterKind::Peaking,
                freq_hz: 1_000.0,
                gain_db: -3.0,
                q: 1.41,
            },
        ),
        (
            "Filter 10: ON Modal Fc 60 Hz Gain 2 dB Q 4",
            CorrectionFilter { kind: FilterKind::Peaking, freq_hz: 60.0, gain_db: 2.0, q: 4.0 },
        ),
        (
            "Filter 2: ON LSC Fc 105 Hz Gain 6.0 dB Q 0.70",
            CorrectionFilter { kind: FilterKind::LowShelf, freq_hz: 105.0, gain_db: 6.0, q: 0.7 },
        ),
        (
            "Filter 3: ON HSC Fc 10000 Hz Gain -2.5 dB Q 0.70",
            CorrectionFilter {
                kind: FilterKind::HighShelf,
                freq_hz: 10_000.0,
                gain_db: -2.5,
                q: 0.7,
            },
        ),
        (
            "filter 4: on pk fc 200 hz gain 1 db q 2",
            CorrectionFilter { kind: FilterKind::Peaking, freq_hz: 200.0, gain_db: 1.0, q: 2.0 },
        ),
    ];
    for (line, expected) in cases {
        assert_eq!(filters_read(line), Ok(vec![expected]), "{line}");
    }
}

/// The review shows every line the correction won't run before anything applies, so a reason read
/// wrong tells the user to fix the wrong thing.
#[test]
fn each_line_it_cannot_use_is_listed_with_its_reason() {
    let cases = [
        ("Filter 2: OFF PK Fc 200 Hz Gain 3 dB Q 1", SkipReason::Off),
        ("Filter 2: ON LP Fc 18000 Hz", SkipReason::Unsupported),
        ("Filter 2: ON HP Fc 20 Hz", SkipReason::Unsupported),
        ("Filter 2: ON LS 6dB Fc 100 Hz Gain 5 dB", SkipReason::Unsupported),
        ("Filter 2: ON LSC 12.0 dB Fc 100 Hz Gain 5.0 dB", SkipReason::Unsupported),
        ("Filter 2: ON PK Fc 1000 Hz Gain 3 dB BW Oct 1.0", SkipReason::Unsupported),
        (
            "Filter 2: ON PK Fc 1000 Hz Gain 45 dB Q 1",
            SkipReason::OutOfRange(FilterError::Gain(45.0)),
        ),
        (
            "Filter 2: ON PK Fc 1000 Hz Gain 3 dB Q inf",
            SkipReason::OutOfRange(FilterError::Q(f64::INFINITY)),
        ),
        ("Filter 2: ON PK Fc 1000 Hz Gain x dB Q 1", SkipReason::Unreadable),
        ("Filter 2: ON PK Fc 1000 Hz Q 1 Gain 3 dB", SkipReason::Unreadable),
        ("Filter 2: PK Fc 1000 Hz Gain 3 dB Q 1", SkipReason::Unreadable),
        ("Filters 2: ON PK Fc 1000 Hz Gain 3 dB Q 1", SkipReason::Unreadable),
        ("Filter 2 ON PK Fc 1000 Hz Gain 3 dB Q 1", SkipReason::Unreadable),
        ("GraphicEQ: 20 -3.1; 25 -2.9", SkipReason::Unreadable),
    ];
    for (line, reason) in cases {
        assert_eq!(listed_after_a_filter(line), Ok(vec![reason]), "{line}");
    }
}

#[test]
fn a_line_that_says_nothing_to_the_correction_is_passed_over() {
    let lines = ["", "   ", "# a comment", "Preamp: -6.2 dB", "Channel: all", "channel: ALL"];
    for line in lines {
        assert_eq!(listed_after_a_filter(line), Ok(Vec::new()), "{line:?}");
    }
}

#[test]
fn a_file_setting_one_ear_apart_is_refused_whole() {
    for channel in ["Channel: L", "Channel: R", "Channel: 1"] {
        let text = format!("{FILTER_LINE}\n{channel}\n{FILTER_LINE}");

        assert_eq!(refusal(&text), Some(ProfileRefusal::OneEar), "{channel}");
    }
}

fn filter_lines(count: usize) -> String {
    (1..=count)
        .map(|n| format!("Filter {n}: ON PK Fc 1000 Hz Gain 1 dB Q 1"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Cut to fit, it would play a curve its author never drew.
#[test]
fn a_file_holding_more_filters_than_a_correction_is_refused_whole() {
    let cases = [(20, None), (21, Some(ProfileRefusal::Filters(FilterSetError::TooMany(21))))];
    for (count, expected) in cases {
        assert_eq!(refusal(&filter_lines(count)), expected, "{count} filters");
    }
}

/// The filter out of range is a listed line by the time the set is built, so the import dialog
/// only ever sees an empty set here, never a filter's index.
#[test]
fn a_file_with_no_filter_the_correction_can_run_is_refused() {
    let cases = [
        ("nothing", ""),
        ("comments alone", "# AutoEq\nPreamp: -6 dB"),
        ("one filter out of range", "Filter 1: ON PK Fc 5 Hz Gain 3 dB Q 1"),
    ];
    for (what, text) in cases {
        assert_eq!(refusal(text), Some(ProfileRefusal::Filters(FilterSetError::Empty)), "{what}");
    }
}

/// Left in, the mark makes the first filter an unreadable line.
#[test]
fn a_byte_order_mark_does_not_hide_the_first_filter() {
    let text = format!("\u{feff}{FILTER_LINE}");

    assert_eq!(filters_read(&text), Ok(vec![FILTER]));
}

/// The review points the user at a line by number, so a blank or a comment above it still counts.
#[test]
fn a_listed_line_is_numbered_as_an_editor_counts_it() {
    let text = concat!(
        "# AutoEq\n",
        "\n",
        "Filter 1: ON PK Fc 105 Hz Gain 6.5 dB Q 0.70\n",
        "  Filter 2: OFF PK Fc 200 Hz Gain 3 dB Q 1\n",
    );

    let listed = parse(text).map(|profile| profile.skipped.to_vec());

    let expected = SkippedLine {
        number: 4,
        text: "Filter 2: OFF PK Fc 200 Hz Gain 3 dB Q 1".into(),
        reason: SkipReason::Off,
    };
    assert_eq!(listed.ok(), Some(vec![expected]));
}
