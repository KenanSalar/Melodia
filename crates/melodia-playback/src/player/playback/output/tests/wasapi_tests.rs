//! Tests for the claim's own decisions: which rates it asks at around a refusal, and the stream
//! mode each drive asks for. Opening a device needs a device, and is tested by hand.

use wasapi::StreamMode;

use super::{retry_rates, source_rate_lacking, stream_mode};
use crate::player::playback::output::Drive;
use crate::player::playback::output::rates::RateSet;
use crate::player::playback::tests::helpers::nz_u32;
use melodia_audio::player::source::audio::SampleRate;

/// The rates the UMC22's sweep found on Windows.
const UMC22: [u32; 3] = [32_000, 44_100, 48_000];

fn offered(rates: &[u32]) -> RateSet {
    rates.iter().copied().collect()
}

/// What a retry is asked from, and what it answers: what the row stands for, the source's rate,
/// the device's own, the rates it offered, and the rates retried, in turn.
type RetryRow = (&'static str, u32, Option<SampleRate>, &'static [u32], &'static [u32]);

/// A rate the sweep found missing is refused at every candidate, one call each, so the claim skips
/// them. Each other row keeps the asks: the set holds the rate, there is no set, the set can't say
/// for a rate off the ladder, or the device runs the source's rate already, where skipping would
/// leave a busy device asked nothing and read as refusing the format.
#[test]
fn a_claim_skips_the_source_rate_only_where_the_offered_set_already_lacks_it() {
    let rows = [
        ("a rate the set lacks", 96_000, SampleRate::new(48_000), Some(offered(&UMC22)), true),
        ("a rate the set holds", 44_100, SampleRate::new(48_000), Some(offered(&UMC22)), false),
        ("no set, as under the system mixer", 96_000, SampleRate::new(48_000), None, false),
        ("a rate off the ladder", 24_000, SampleRate::new(48_000), Some(offered(&UMC22)), false),
        ("the device's own rate", 96_000, SampleRate::new(96_000), Some(offered(&UMC22)), false),
        ("a mix format naming no rate", 96_000, SampleRate::new(0), Some(offered(&UMC22)), false),
    ];
    for (what, source, own, offered, expected) in rows {
        let skipped = source_rate_lacking(nz_u32(source), own, offered);

        assert_eq!(skipped, expected, "{what}");
    }
}

/// The pick first and the mix rate behind it, since a driver can pass a rate and refuse to
/// initialise it. The rows cover the pick above the source and below it, a pick that is the mix
/// rate, asked once, and either half missing.
#[test]
fn a_refused_rate_is_retried_at_the_pick_then_at_the_mix_rate() {
    let rows: [RetryRow; 5] = [
        ("the pick is the mix rate", 96_000, SampleRate::new(48_000), &UMC22, &[48_000]),
        (
            "a pick below the source",
            88_200,
            SampleRate::new(48_000),
            &[44_100, 48_000, 96_000, 192_000],
            &[44_100, 48_000],
        ),
        (
            "a pick above the source",
            96_000,
            SampleRate::new(48_000),
            &[48_000, 192_000],
            &[192_000, 48_000],
        ),
        ("nothing to pick from", 96_000, SampleRate::new(48_000), &[], &[48_000]),
        ("no mix rate", 96_000, SampleRate::new(0), &UMC22, &[48_000]),
    ];
    for (what, source, own, rates, expected) in rows {
        let retried: Vec<u32> =
            retry_rates(nz_u32(source), own, Some(offered(rates))).map(SampleRate::get).collect();

        assert_eq!(retried, expected, "{what}");
    }
}

/// The source's rate has just been refused, so it is never retried, even where the sweep found it:
/// a driver can pass a rate and still refuse to initialise it. Left among the others, it is the
/// rate nearest itself, which is where the pick lands once nothing of its family is offered.
#[test]
fn a_refused_rate_is_never_retried_even_where_it_was_offered() {
    let swept = offered(&[44_100, 88_200, 176_400, 192_000]);

    let retried: Vec<u32> = retry_rates(nz_u32(192_000), SampleRate::new(44_100), Some(swept))
        .map(SampleRate::get)
        .collect();

    assert_eq!(retried, [176_400, 44_100]);
}

/// Under events the device signals once per buffer, so the buffer is the period.
#[test]
fn an_event_driven_claim_asks_for_one_period_as_its_whole_buffer() {
    let mode = stream_mode(Drive::Events, 200_000);

    assert_eq!(mode, StreamMode::EventsExclusive { period_hns: 200_000 });
}

/// A polled writer wakes on a timer that can run late, and the buffer's depth past one period is
/// all the lateness it can absorb before an underrun.
#[test]
fn a_polled_claim_asks_for_a_buffer_four_periods_deep() {
    let rows = [
        (50_000, 200_000),
        (200_000, 800_000),
        (1_000_000, 4_000_000),
        // Past any period a claim asks for, but the multiply must not be the thing that panics.
        (i64::MAX, i64::MAX),
    ];
    for (period_hns, buffer_duration_hns) in rows {
        let mode = stream_mode(Drive::Polling, period_hns);

        assert_eq!(
            mode,
            StreamMode::PollingExclusive { period_hns, buffer_duration_hns },
            "{period_hns}"
        );
    }
}
