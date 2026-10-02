//! Tests for the rate policy both backends read: the rates a claim keeps, the rate a fresh claim
//! would run a device at, and the pick for a rate the device lacks.

use melodia_audio::player::source::audio::SampleRate;

use super::{LADDER, RateSet, claim_rate, device_rate_for};
use crate::player::playback::output::RateFallback;
use crate::player::playback::tests::helpers::nz_u32;

/// The rates the UMC22's sweep found on Windows.
const UMC22: [u32; 3] = [32_000, 44_100, 48_000];

fn offered(rates: &[u32]) -> RateSet {
    rates.iter().copied().collect()
}

/// A set travels with every claim, so a rung it dropped would be a rate a later track could never
/// be served at.
#[test]
fn every_ladder_rung_round_trips_through_a_rate_set_in_ascending_order() {
    let set = offered(&LADDER);

    let rates: Vec<u32> = set.rates().collect();

    assert_eq!(rates, LADDER);
}

/// A set has a bit for each rung and nothing else, so it can't say whether a device takes a rate
/// off the ladder, and says so rather than reading as a refusal.
#[test]
fn a_set_answers_for_its_own_rungs_and_not_for_a_rate_off_the_ladder() {
    let set = offered(&[24_000, 44_100]);
    let rows = [(44_100, Some(true)), (48_000, Some(false)), (24_000, None)];
    for (rate, expected) in rows {
        assert_eq!(set.contains(rate), expected, "{rate} Hz");
    }
}

/// What a fresh claim would run the device at, which is what decides whether the open one serves.
/// Only Resample converts, and a rate off the ladder gets no prediction at all.
#[test]
fn a_fresh_claim_runs_the_source_rate_where_offered_and_converts_only_under_resample() {
    let rows = [
        ("an offered rate", 44_100, RateFallback::Resample, Some(44_100)),
        ("an offered rate, under the system mixer", 44_100, RateFallback::Shared, Some(44_100)),
        ("a missing rate", 96_000, RateFallback::Resample, Some(48_000)),
        ("a missing rate, under the system mixer", 96_000, RateFallback::Shared, None),
        ("a rate off the ladder", 24_000, RateFallback::Resample, None),
    ];
    for (what, source, fallback, expected) in rows {
        let rate = claim_rate(nz_u32(source), offered(&UMC22), fallback);

        assert_eq!(rate.map(SampleRate::get), expected, "{what}");
    }
}

/// The source's family first, so the ratio stays simple: the lowest multiple above, then the
/// highest below. Past those the nearest, where a tie goes to the higher rate, which keeps more of
/// the band.
#[test]
fn a_missing_rate_is_converted_within_its_family_before_the_nearest() {
    let rows: [(&str, u32, &[u32], Option<u32>); 6] = [
        ("the lowest multiple above", 44_100, &[48_000, 88_200, 176_400], Some(88_200)),
        ("a multiple above before the family below", 48_000, &[32_000, 96_000], Some(96_000)),
        ("the highest of the family below", 96_000, &UMC22, Some(48_000)),
        ("the nearest, where the family offers none", 44_100, &[32_000, 48_000], Some(48_000)),
        ("the higher of two equally near", 38_050, &[32_000, 44_100], Some(44_100)),
        ("nothing offered", 44_100, &[], None),
    ];
    for (what, source, supported, expected) in rows {
        let rate = device_rate_for(nz_u32(source), supported);

        assert_eq!(rate.map(SampleRate::get), expected, "{what}");
    }
}
