//! The crossfeed as a source plays it. Signals stay far under the limiter's knee, so the limiter
//! rests and a passthrough reads bit-identical.

use std::sync::Arc;
use std::time::Duration;

use super::{CROSSFEED_RAMP_MS, CrossfeedShared};
use crate::player::playback::crossfade::FadeShared;
use crate::player::playback::equalizer::{DspControls, EqSource};
use crate::player::playback::replaygain::TrackReplayGain;
use crate::player::playback::tests::helpers::{TestSource, bits};
use melodia_audio::player::source::audio::AudioSource;
use melodia_core::entities::crossfeed::{CrossfeedSetting, CrossfeedStrength};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const RATE: u32 = 48_000;

const QUIET: [f32; 8] = [-0.25, -0.15, -0.05, 0.05, 0.15, 0.25, 0.1, -0.1];

/// The level the split's expected values are worked from.
const BASS_LEVEL: f32 = 0.5;

/// Far past the cross path's settling, so the split reads at DC.
const SETTLED_FRAMES: usize = 4_800;

/// Room for the six decimals the expected values are worked to, far under a step of strength.
const SPLIT_TOLERANCE: f32 = 1e-5;

fn ramp_frames() -> Result<usize, std::num::TryFromIntError> {
    usize::try_from(CROSSFEED_RAMP_MS * u64::from(RATE) / 1_000)
}

fn on_at(strength: f32) -> Result<CrossfeedSetting, Box<dyn std::error::Error>> {
    Ok(CrossfeedSetting { enabled: true, strength: CrossfeedStrength::try_from(strength)? })
}

fn shared_at(setting: CrossfeedSetting) -> Arc<CrossfeedShared> {
    let shared = CrossfeedShared::new();
    shared.set(setting);
    shared
}

/// Two channels interleaved as a source carries them.
fn stereo(left: &[f32], right: &[f32]) -> Vec<f32> {
    left.iter().zip(right).flat_map(|(&l, &r)| [l, r]).collect()
}

fn quiet(frames: usize) -> Vec<f32> {
    QUIET.iter().copied().cycle().take(frames).collect()
}

/// [`QUIET`] in the left ear alone.
fn left_only(frames: usize) -> Vec<f32> {
    stereo(&quiet(frames), &vec![0.0; frames])
}

/// [`BASS_LEVEL`] held in the left ear alone, bass at its lowest.
fn bass_on_the_left(frames: usize) -> Vec<f32> {
    stereo(&vec![BASS_LEVEL; frames], &vec![0.0; frames])
}

fn crossfed_source(
    shared: &Arc<CrossfeedShared>,
    input: Vec<f32>,
    channels: u16,
) -> EqSource<TestSource> {
    let controls = DspControls { crossfeed: Arc::clone(shared), ..DspControls::default() };
    EqSource::new(
        TestSource::new(input, channels, RATE),
        controls,
        TrackReplayGain::default(),
        FadeShared::idle(),
    )
}

/// One frame of a stereo source, left then right.
fn next_frame(source: &mut EqSource<TestSource>) -> Option<[f32; 2]> {
    Some([source.next()?, source.next()?])
}

fn pull_frames(source: &mut EqSource<TestSource>, frames: usize) {
    for _ in 0..frames {
        let _ = next_frame(source);
    }
}

/// Whether the source reads as processed after each of the next `frames` stereo frames.
fn engaged_per_frame(source: &mut EqSource<TestSource>, frames: usize) -> Vec<bool> {
    (0..frames)
        .map(|_| {
            let _ = next_frame(source);
            source.dsp_engaged()
        })
        .collect()
}

/// Switching it on must not move a vocal or a mono mix in a stereo file: only what differs between
/// the channels crosses.
#[test]
fn centre_panned_sound_plays_bit_identical_with_crossfeed_on() -> TestResult {
    let input = stereo(&quiet(512), &quiet(512));
    let mut source = crossfed_source(&shared_at(on_at(1.0)?), input.clone(), 2);

    let played: Vec<f32> = source.by_ref().collect();

    assert!(source.dsp_engaged(), "the crossfeed has to be running for this to mean anything");
    assert_eq!(bits(&played), bits(&input));
    Ok(())
}

/// Bass in one ear plays `1 - g` there and `g` in the other, `g = 1 / (1 + 10^(feed / 20))`, the
/// feed level running 9.5 dB at Subtle, 6 dB at the default and 4.5 dB at Strong. The two ears
/// still sum to the level that came in.
#[test]
fn hard_panned_bass_splits_between_the_ears_by_the_strength() -> TestResult {
    let cases = [
        ("subtle", 0.0, [0.374_542, 0.125_458]),
        ("the default", 0.7, [0.333_070, 0.166_930]),
        ("strong", 1.0, [0.313_350, 0.186_650]),
    ];
    for (what, strength, [near, far]) in cases {
        let shared = shared_at(on_at(strength)?);
        let mut source = crossfed_source(&shared, bass_on_the_left(SETTLED_FRAMES + 1), 2);
        pull_frames(&mut source, SETTLED_FRAMES);

        let Some([left, right]) = next_frame(&mut source) else {
            return Err(format!("{what}: the source ended before the split settled").into());
        };

        assert!(
            (left - near).abs() < SPLIT_TOLERANCE && (right - far).abs() < SPLIT_TOLERANCE,
            "{what}: expected ({near}, {far}), got ({left}, {right})"
        );
    }
    Ok(())
}

/// A mono file has no image to narrow, and a wider one reaches a stereo device as its front pair,
/// half the mix: neither engages it, so both keep the bit-identical bypass.
#[test]
fn crossfeed_reaches_stereo_sources_only() -> TestResult {
    let cases = [("mono", 1, false), ("stereo", 2, true), ("three", 3, false), ("5.1", 6, false)];
    for (what, channels, engaged) in cases {
        let input = quiet(64 * usize::from(channels));
        let mut source = crossfed_source(&shared_at(on_at(1.0)?), input, channels);

        let _ = source.next();

        assert_eq!(source.dsp_engaged(), engaged, "{what}");
    }
    Ok(())
}

/// Off glides out over the ramp so it doesn't click, and once it lands the source is a
/// passthrough again.
#[test]
fn switching_off_glides_over_the_ramp_then_bypasses() -> TestResult {
    let ramp = ramp_frames()?;
    let shared = shared_at(on_at(1.0)?);
    let mut source = crossfed_source(&shared, left_only(2 * ramp), 2);
    pull_frames(&mut source, 1);

    shared.set(CrossfeedSetting::default());
    let engaged = engaged_per_frame(&mut source, ramp + 5);

    assert_eq!(engaged, [vec![true; ramp], vec![false; 5]].concat());
    Ok(())
}

/// A slider dragged mid-glide has to carry on from where the amount sits: restarting from either
/// end steps the far ear by the whole setting, which is the click the ramp exists to remove.
#[test]
fn a_change_mid_ramp_glides_on_from_where_the_amount_sits() -> TestResult {
    let ramp = ramp_frames()?;
    let strong = on_at(1.0)?;
    let input = bass_on_the_left(2 * ramp);
    let mut steady = crossfed_source(&shared_at(strong), input.clone(), 2);
    let shared = CrossfeedShared::new();
    let mut gliding = crossfed_source(&shared, input, 2);
    shared.set(strong);
    pull_frames(&mut steady, ramp / 2);
    pull_frames(&mut gliding, ramp / 2);

    shared.set(CrossfeedSetting::default());
    let (Some([_, steady_far]), Some([_, gliding_far])) =
        (next_frame(&mut steady), next_frame(&mut gliding))
    else {
        return Err("a source ended halfway through the ramp".into());
    };

    // Room for the turn repeating the last frame's amount, far under the 0.09 a restart moves it.
    let tolerance = 1e-3;
    let half_of_steady = 0.5 * steady_far;
    assert!(
        (gliding_far - half_of_steady).abs() < tolerance,
        "the far ear read {gliding_far} on the turn, against {half_of_steady} halfway up"
    );
    Ok(())
}

#[test]
fn a_seek_lands_a_crossfeed_ramp_at_its_target() -> TestResult {
    let input = left_only(256);
    let shared = shared_at(on_at(1.0)?);
    let mut source = crossfed_source(&shared, input.clone(), 2);
    pull_frames(&mut source, 1);
    shared.set(CrossfeedSetting::default());
    pull_frames(&mut source, 10);

    source.try_seek(Duration::ZERO)?;
    let after_seek: Vec<f32> = source.collect();

    assert_eq!(bits(&after_seek), bits(&input), "switched off, it must be gone at the seek");
    Ok(())
}

/// The cross path's state belongs to the audio a seek left behind.
#[test]
fn a_seek_starts_the_crossfeed_from_rest() -> TestResult {
    let input = left_only(400);
    let shared = shared_at(on_at(1.0)?);
    let fresh: Vec<f32> = crossfed_source(&shared, input.clone(), 2).collect();
    let mut warmed = crossfed_source(&shared, input, 2);
    pull_frames(&mut warmed, 200);

    warmed.try_seek(Duration::ZERO)?;
    let after_seek: Vec<f32> = warmed.collect();

    assert_eq!(bits(&after_seek), bits(&fresh));
    Ok(())
}
