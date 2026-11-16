//! The correction as a source plays it: a change of set crossfades rather than snapping, the
//! bit-identical bypass comes back once nothing is left running, and the level comes down by
//! exactly the curve's loudest point.
//!
//! Read through `dsp_engaged`, which reports the source's bypass, one mono sample at a time. At
//! [`SWAP_RATE`] a frame is a millisecond, so a swap's ramp is [`CORRECTION_SWAP_MS`] samples.

use std::time::Duration;

use super::CORRECTION_SWAP_MS;
use crate::player::playback::correction::shared::CorrectionWriter;
use crate::player::playback::crossfade::FadeShared;
use crate::player::playback::equalizer::{DspControls, EqShared, EqSource, NUM_BANDS};
use crate::player::playback::replaygain::TrackReplayGain;
use crate::player::playback::tests::helpers::{TestSource, bits, fill_sine};
use melodia_audio::player::source::audio::AudioSource;
use melodia_core::entities::correction::{CorrectionFilter, CorrectionFilters, FilterKind};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SWAP_RATE: u32 = 1_000;

/// Far below full scale, so the limiter never moves and only the correction shapes the output.
const QUIET: [f32; 8] = [-0.25, -0.15, -0.05, 0.05, 0.15, 0.25, 0.1, -0.1];

const BOOST: CorrectionFilter =
    CorrectionFilter { kind: FilterKind::Peaking, freq_hz: 100.0, gain_db: 6.0, q: 1.0 };

fn quiet(len: usize) -> Vec<f32> {
    QUIET.iter().copied().cycle().take(len).collect()
}

fn swap_frames() -> Result<usize, std::num::TryFromIntError> {
    usize::try_from(CORRECTION_SWAP_MS)
}

fn writer_with(filter: CorrectionFilter) -> Result<CorrectionWriter, Box<dyn std::error::Error>> {
    let mut writer = CorrectionWriter::default();
    writer.publish(&CorrectionFilters::try_from(vec![filter])?);
    Ok(writer)
}

/// A mono source on `writer`'s set with the equalizer and `ReplayGain` off, which gate nothing of
/// the correction's.
fn corrected_source(writer: &CorrectionWriter, input: Vec<f32>, rate: u32) -> EqSource<TestSource> {
    let controls = DspControls { correction: writer.shared(), ..DspControls::default() };
    EqSource::new(
        TestSource::new(input, 1, rate),
        controls,
        TrackReplayGain::default(),
        FadeShared::idle(),
    )
}

/// Whether the source reads as processed after each of the next `samples` it plays.
fn engaged_per_sample(source: &mut EqSource<TestSource>, samples: usize) -> Vec<bool> {
    (0..samples)
        .map(|_| {
            let _ = source.next();
            source.dsp_engaged()
        })
        .collect()
}

fn pull(source: &mut EqSource<TestSource>, samples: usize) {
    for _ in 0..samples {
        let _ = source.next();
    }
}

/// The outgoing curve fades for the whole swap, and the moment it lands with nothing left to run
/// the source is a passthrough again.
#[test]
fn a_clear_fades_the_correction_out_over_the_swap_then_bypasses() -> TestResult {
    let swap = swap_frames()?;
    let mut writer = writer_with(BOOST)?;
    let mut source = corrected_source(&writer, quiet(512), SWAP_RATE);
    pull(&mut source, swap + 1);

    writer.clear();
    let engaged = engaged_per_sample(&mut source, swap + 5);

    assert_eq!(engaged, [vec![true; swap], vec![false; 5]].concat());
    Ok(())
}

/// A new track takes the correction as it stands, or every track would open on a fade into it.
#[test]
fn a_source_takes_the_set_playing_when_it_starts_with_no_ramp() -> TestResult {
    let swap = swap_frames()?;
    let mut writer = writer_with(BOOST)?;
    let mut source = corrected_source(&writer, quiet(512), SWAP_RATE);
    pull(&mut source, 1);

    writer.clear();
    let engaged = engaged_per_sample(&mut source, swap + 5);

    assert_eq!(
        engaged,
        [vec![true; swap], vec![false; 5]].concat(),
        "a clear right after the first sample has to start its swap at once"
    );
    Ok(())
}

/// Switching device twice in a row plays both swaps whole, back to back, rather than cutting the
/// first short.
#[test]
fn a_swap_arriving_mid_ramp_waits_for_the_ramp_to_land() -> TestResult {
    let swap = swap_frames()?;
    let mut writer = CorrectionWriter::default();
    let mut source = corrected_source(&writer, quiet(512), SWAP_RATE);
    pull(&mut source, 1);
    writer.publish(&CorrectionFilters::try_from(vec![BOOST])?);
    let before = engaged_per_sample(&mut source, 10);

    writer.clear();
    let after = engaged_per_sample(&mut source, 2 * swap + 5 - 10);

    assert_eq!([before, after].concat(), [vec![true; 2 * swap], vec![false; 5]].concat());
    Ok(())
}

#[test]
fn a_seek_lands_a_swap_in_flight_at_its_target() -> TestResult {
    let swap = swap_frames()?;
    let input = quiet(256);
    let mut writer = writer_with(BOOST)?;
    let mut source = corrected_source(&writer, input.clone(), SWAP_RATE);
    pull(&mut source, swap + 1);
    writer.clear();
    pull(&mut source, 10);

    source.try_seek(Duration::ZERO)?;
    let after_seek: Vec<f32> = source.collect();

    assert_eq!(bits(&after_seek), bits(&input), "the cleared correction must be gone at the seek");
    Ok(())
}

#[test]
fn a_seek_starts_the_correction_from_rest() -> TestResult {
    let input = quiet(400);
    let writer = writer_with(BOOST)?;
    let fresh: Vec<f32> = corrected_source(&writer, input.clone(), SWAP_RATE).collect();
    let mut warmed = corrected_source(&writer, input, SWAP_RATE);
    pull(&mut warmed, 200);

    warmed.try_seek(Duration::ZERO)?;
    let after_seek: Vec<f32> = warmed.collect();

    assert_eq!(bits(&after_seek), bits(&fresh));
    Ok(())
}

#[test]
fn a_swap_between_two_sets_with_nothing_running_keeps_the_bypass() -> TestResult {
    let above_nyquist = CorrectionFilter { freq_hz: 20_000.0, ..BOOST };
    let mut writer = writer_with(above_nyquist)?;
    let mut source = corrected_source(&writer, quiet(512), 8_000);
    pull(&mut source, 1);

    writer.clear();
    let engaged = engaged_per_sample(&mut source, 10);

    assert_eq!(engaged, vec![false; 10]);
    Ok(())
}

#[test]
fn a_correction_filter_sits_out_from_nyquist_up() -> TestResult {
    let cases =
        [("just under", 3_999.0, true), ("at", 4_000.0, false), ("just over", 4_001.0, false)];
    for (what, freq_hz, engaged) in cases {
        let writer = writer_with(CorrectionFilter { freq_hz, ..BOOST })?;
        let mut source = corrected_source(&writer, quiet(64), 8_000);

        let _ = source.next();

        assert_eq!(source.dsp_engaged(), engaged, "{what} Nyquist at 8 kHz");
    }
    Ok(())
}

const TONE_RATE: u32 = 48_000;
/// [`TONE_RATE`] as the sine is drawn at it.
const TONE_RATE_HZ: f32 = 48_000.0;
const TONE_HZ: f32 = 1_000.0;

/// Far past `f32` rounding and what is left of the filters' settling, far under the factor of two
/// a missing headroom leaves.
const TONE_TOLERANCE: f32 = 1e-4;

/// A quiet tone where both curves below peak, long enough for the filters to settle.
fn tone() -> Vec<f32> {
    let mut samples = vec![0.0; 4_800];
    fill_sine(&mut samples, TONE_HZ, TONE_RATE_HZ, 0.1);
    samples
}

const PEAK_AT_TONE: CorrectionFilter =
    CorrectionFilter { kind: FilterKind::Peaking, freq_hz: 1_000.0, gain_db: 6.0, q: 1.41 };

/// The last 10 ms of each, once the filters have settled.
fn settled_tails<'a>(output: &'a [f32], input: &'a [f32]) -> (&'a [f32], &'a [f32]) {
    (&output[output.len() - 480..], &input[input.len() - 480..])
}

/// The level comes down by the loudest point and no further, so a tone at the peak plays at the
/// level it came in at: boosted, it would reach the limiter.
#[test]
fn a_tone_at_the_corrections_peak_plays_at_its_own_level() -> TestResult {
    let input = tone();
    let writer = writer_with(PEAK_AT_TONE)?;

    let output: Vec<f32> = corrected_source(&writer, input.clone(), TONE_RATE).collect();

    let (played, came_in) = settled_tails(&output, &input);
    assert!(
        played.iter().zip(came_in).all(|(o, i)| (o - i).abs() < TONE_TOLERANCE),
        "the 1 kHz tone left the +6 dB peak at another level"
    );
    Ok(())
}

/// A slider boosting under the correction's peak stacks on it, so its rebuild has to take the
/// band into the headroom or the tone plays twice as loud.
#[test]
fn a_slider_moved_under_a_correction_counts_in_its_headroom() -> TestResult {
    const ONE_KHZ_BAND: usize = 5;
    let input = tone();
    let writer = writer_with(PEAK_AT_TONE)?;
    let eq = EqShared::new(true, &[0.0; NUM_BANDS]);
    let controls =
        DspControls { eq: eq.clone(), correction: writer.shared(), ..DspControls::default() };
    let mut source = EqSource::new(
        TestSource::new(input.clone(), 1, TONE_RATE),
        controls,
        TrackReplayGain::default(),
        FadeShared::idle(),
    );
    let mut output: Vec<f32> = source.by_ref().take(2_400).collect();

    eq.set_gain(ONE_KHZ_BAND, 6.0);
    output.extend(source);

    let (played, came_in) = settled_tails(&output, &input);
    assert!(
        played.iter().zip(came_in).all(|(o, i)| (o - i).abs() < TONE_TOLERANCE),
        "the 1 kHz tone left a +6 dB band over the +6 dB correction at another level"
    );
    Ok(())
}
