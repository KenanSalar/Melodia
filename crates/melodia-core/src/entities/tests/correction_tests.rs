//! The checks a correction passes on its way to the audio thread, and the per-device map it lives
//! in. A hand-edited `eq.json` and an imported file both reach the DSP through these alone.

use super::super::correction::{
    Correction, CorrectionFilter, CorrectionFilters, CorrectionOrigin, DeviceCorrections,
    FilterError, FilterKind, FilterSetError, MAX_CORRECTION_FILTERS,
};

const IN_RANGE: CorrectionFilter =
    CorrectionFilter { kind: FilterKind::Peaking, freq_hz: 1_000.0, gain_db: 3.0, q: 1.0 };

fn with_freq(freq_hz: f64) -> CorrectionFilter {
    CorrectionFilter { freq_hz, ..IN_RANGE }
}

fn with_gain(gain_db: f64) -> CorrectionFilter {
    CorrectionFilter { gain_db, ..IN_RANGE }
}

fn with_q(q: f64) -> CorrectionFilter {
    CorrectionFilter { q, ..IN_RANGE }
}

/// Which range refused the filter, `None` for none. A token rather than the error, whose `f64`
/// can be a NaN that equals nothing.
fn refused_by(filter: CorrectionFilter) -> Option<&'static str> {
    match filter.check() {
        Ok(()) => None,
        Err(FilterError::Frequency(_)) => Some("frequency"),
        Err(FilterError::Gain(_)) => Some("gain"),
        Err(FilterError::Q(_)) => Some("q"),
    }
}

#[test]
fn each_range_takes_its_edges_and_refuses_the_step_past_them() {
    let cases = [
        ("lowest frequency", with_freq(10.0), None),
        ("under the lowest frequency", with_freq(10.0_f64.next_down()), Some("frequency")),
        ("highest frequency", with_freq(24_000.0), None),
        ("over the highest frequency", with_freq(24_000.0_f64.next_up()), Some("frequency")),
        ("deepest cut", with_gain(-30.0), None),
        ("past the deepest cut", with_gain((-30.0_f64).next_down()), Some("gain")),
        ("largest boost", with_gain(30.0), None),
        ("past the largest boost", with_gain(30.0_f64.next_up()), Some("gain")),
        ("narrowest Q", with_q(20.0), None),
        ("past the narrowest Q", with_q(20.0_f64.next_up()), Some("q")),
        ("widest Q", with_q(0.05), None),
        ("past the widest Q", with_q(0.05_f64.next_down()), Some("q")),
    ];
    for (what, filter, expected) in cases {
        assert_eq!(refused_by(filter), expected, "{what}");
    }
}

/// A file can spell `inf` or `nan` where a number goes, and Rust parses both.
#[test]
fn a_value_that_is_not_finite_is_refused_by_its_own_range() {
    let cases = [
        ("NaN frequency", with_freq(f64::NAN), Some("frequency")),
        ("infinite frequency", with_freq(f64::INFINITY), Some("frequency")),
        ("NaN gain", with_gain(f64::NAN), Some("gain")),
        ("infinite cut", with_gain(f64::NEG_INFINITY), Some("gain")),
        ("NaN Q", with_q(f64::NAN), Some("q")),
    ];
    for (what, filter, expected) in cases {
        assert_eq!(refused_by(filter), expected, "{what}");
    }
}

#[test]
fn a_set_holds_one_to_twenty_filters() {
    let cases = [
        (0, Err(FilterSetError::Empty)),
        (1, Ok(1)),
        (MAX_CORRECTION_FILTERS, Ok(20)),
        (MAX_CORRECTION_FILTERS + 1, Err(FilterSetError::TooMany(21))),
    ];
    for (count, expected) in cases {
        let built = CorrectionFilters::try_from(vec![IN_RANGE; count]);

        assert_eq!(built.map(|set| set.filters().len()), expected, "{count} filters");
    }
}

/// An error naming the wrong filter sends the user to fix one that reads fine.
#[test]
fn a_set_names_the_filter_that_is_out_of_range() {
    let filters = vec![IN_RANGE, IN_RANGE, with_q(0.0), IN_RANGE];

    let built = CorrectionFilters::try_from(filters);

    assert_eq!(built, Err(FilterSetError::Filter { index: 2, source: FilterError::Q(0.0) }));
}

fn correction_of(gain_db: f64) -> Result<Correction, FilterSetError> {
    Ok(Correction {
        origin: CorrectionOrigin::Paste,
        device_name: None,
        filters: CorrectionFilters::try_from(vec![with_gain(gain_db)])?,
    })
}

/// System default is a field of its own, so clearing a device's correction can't take it, and the
/// reverse.
#[test]
fn system_default_and_a_device_keep_their_own_correction() -> Result<(), FilterSetError> {
    let mut corrections = DeviceCorrections::default();
    corrections.insert(None, correction_of(-2.0)?);
    corrections.insert(Some("hw:CARD=UMC22".to_owned()), correction_of(4.0)?);

    let removed = corrections.remove(Some("hw:CARD=UMC22"));

    let gain = |correction: Option<&Correction>| correction.map(|c| c.filters.filters()[0].gain_db);
    assert_eq!(gain(removed.as_ref()), Some(4.0), "the device's own correction comes back");
    assert_eq!(gain(corrections.for_device(Some("hw:CARD=UMC22"))), None);
    assert_eq!(gain(corrections.for_device(None)), Some(-2.0), "System default's stays");
    Ok(())
}
