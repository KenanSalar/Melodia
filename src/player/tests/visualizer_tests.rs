//! Tests for the visualizer's sample rings and its tap source.

use std::time::Duration;

use super::{RING_CAP, VisualizerShared, VisualizerTap};
use crate::player::audio::AudioSource;
use crate::player::tests::helpers::{TestSource, bits};
use crate::test_support::UNBOUNDED;

// --- helpers ---------------------------------------------------------------

/// `len` samples, every one distinct and exactly representable — so a snapshot
/// can't pass by accidentally matching at the wrong offset. `u16 → f32` is
/// lossless and the divisor is a power of two, so no value is rounded.
fn counted(len: usize) -> Vec<f32> {
    (0..len).map(|i| f32::from(u16::try_from(i + 1).unwrap_or(u16::MAX)) / 65_536.0).collect()
}

/// Drain a tap over `input` and return `(passed-through samples, ring window)`.
///
/// The tap is drained through `by_ref` so it is still alive at the snapshot: a
/// dropped source releases its deck, and a released deck is not mixed in.
fn run_tap(
    input: Vec<f32>,
    channels: u16,
    sample_rate: u32,
    enabled: bool,
    window: usize,
) -> (Vec<f32>, Vec<f32>) {
    let viz = VisualizerShared::new(enabled);
    let mut tap = VisualizerTap::new(TestSource::new(input, channels, sample_rate), &viz, 0);
    let out: Vec<f32> = tap.by_ref().collect();
    let mut ring = vec![0.0; window];
    viz.snapshot(&mut ring);
    (out, ring)
}

// --- VisualizerShared ------------------------------------------------------

#[test]
fn a_disabled_push_never_touches_the_ring() {
    let viz = VisualizerShared::new(false);
    let run = viz.begin_run(0);
    for s in counted(64) {
        run.push(s);
    }
    let mut out = vec![f32::NAN; 8];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out), bits(&[0.0; 8]));
}

#[test]
fn enabling_starts_the_ring_mid_stream() {
    let viz = VisualizerShared::new(false);
    let run = viz.begin_run(0);
    run.push(0.5);
    viz.set_enabled(true);
    assert!(viz.is_enabled());
    run.push(0.25);

    let mut out = vec![0.0; 2];
    viz.snapshot(&mut out);
    // Only the second push landed, and it sits at the newest end.
    assert_eq!(bits(&out), bits(&[0.0, 0.25]));
}

#[test]
fn arming_drops_the_history_left_behind_by_the_last_arm() {
    // Closing Now Playing disarms the tap without clearing anything, so the ring
    // still holds the window it stopped on. Coming back to it minutes later must
    // ramp in from silence, not resume on a stale spectrum.
    let viz = VisualizerShared::new(true);
    let run = viz.begin_run(0);
    for s in counted(64) {
        run.push(s);
    }
    viz.set_enabled(false);
    viz.set_enabled(true);
    run.push(0.5);

    let mut out = vec![f32::NAN; 4];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out), bits(&[0.0, 0.0, 0.0, 0.5]));
}

#[test]
fn snapshot_returns_the_most_recent_samples_oldest_first() {
    let viz = VisualizerShared::new(true);
    let run = viz.begin_run(0);
    let pushed = counted(64);
    for &s in &pushed {
        run.push(s);
    }
    let mut out = vec![0.0; 8];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out), bits(&pushed[56..]));
}

#[test]
fn a_short_history_is_padded_at_the_front() {
    let viz = VisualizerShared::new(true);
    let run = viz.begin_run(0);
    let pushed = counted(3);
    for &s in &pushed {
        run.push(s);
    }
    let mut out = vec![0.0; 8];
    viz.snapshot(&mut out);
    let mut want = vec![0.0; 5];
    want.extend_from_slice(&pushed);
    assert_eq!(bits(&out), bits(&want));
}

#[test]
fn the_ring_wraps_without_losing_the_newest_window() {
    let viz = VisualizerShared::new(true);
    let run = viz.begin_run(0);
    // Two and a half laps, so the window we ask for spans a wrap point.
    let pushed = counted(RING_CAP * 5 / 2);
    for &s in &pushed {
        run.push(s);
    }
    let mut out = vec![0.0; 1024];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out), bits(&pushed[pushed.len() - 1024..]));
}

#[test]
fn a_window_wider_than_the_ring_is_padded_not_repeated() {
    let viz = VisualizerShared::new(true);
    let run = viz.begin_run(0);
    let pushed = counted(RING_CAP * 2);
    for &s in &pushed {
        run.push(s);
    }
    let mut out = vec![f32::NAN; RING_CAP + 16];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out[..16]), bits(&[0.0; 16]));
    assert_eq!(bits(&out[16..]), bits(&pushed[pushed.len() - RING_CAP..]));
}

// --- mixing the decks ------------------------------------------------------

#[test]
fn two_live_decks_are_summed() {
    // What a crossfade looks like from here: both decks pulled for the same
    // output frames. Interleaving them instead — one ring, two writers — reads
    // as each track at half rate plus a square wave at Nyquist.
    let viz = VisualizerShared::new(true);
    let a = viz.begin_run(0);
    let b = viz.begin_run(1);
    for i in 0..8 {
        a.push(0.25);
        b.push(if i % 2 == 0 { -0.25 } else { 0.5 });
    }

    let mut out = vec![0.0; 4];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out), bits(&[0.0, 0.75, 0.0, 0.75]));
}

#[test]
fn a_deck_whose_source_ended_stops_contributing() {
    // The outgoing deck of a crossfade keeps a full window of audio after it
    // drains. Mixing that frozen tail into every later frame would leave a ghost
    // of the track that ended.
    let viz = VisualizerShared::new(true);
    let a = viz.begin_run(0);
    {
        let b = viz.begin_run(1);
        for _ in 0..8 {
            b.push(0.5);
        }
    }
    for _ in 0..8 {
        a.push(0.25);
    }

    let mut out = vec![0.0; 4];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out), bits(&[0.25; 4]));
}

#[test]
fn nothing_playing_reads_as_silence() {
    let viz = VisualizerShared::new(true);
    {
        let run = viz.begin_run(0);
        for s in counted(64) {
            run.push(s);
        }
    }
    let mut out = vec![f32::NAN; 8];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out), bits(&[0.0; 8]));
}

#[test]
fn a_new_run_cannot_see_what_the_deck_played_before() {
    // A crossfade lands its incoming track on the deck that played two tracks
    // ago, and that ring still holds the end of it.
    let viz = VisualizerShared::new(true);
    {
        let old = viz.begin_run(0);
        for _ in 0..64 {
            old.push(0.5);
        }
    }
    let new = viz.begin_run(0);
    new.push(0.25);

    let mut out = vec![f32::NAN; 4];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out), bits(&[0.0, 0.0, 0.0, 0.25]));
}

#[test]
fn a_second_run_on_a_live_deck_keeps_its_history() {
    // The gapless case: the successor is staged behind a track that is still
    // playing, on the same deck. That audio is continuous, so the predecessor's
    // tail belongs in the window.
    let viz = VisualizerShared::new(true);
    let playing = viz.begin_run(0);
    for _ in 0..4 {
        playing.push(0.5);
    }
    let staged = viz.begin_run(0);
    drop(playing);
    staged.push(0.25);

    let mut out = vec![0.0; 4];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out), bits(&[0.5, 0.5, 0.5, 0.25]));
}

#[test]
fn a_deck_that_does_not_exist_is_a_no_op() {
    let viz = VisualizerShared::new(true);
    let live = viz.begin_run(0);
    let nowhere = viz.begin_run(99);
    live.push(0.5);
    nowhere.push(1.0);

    let mut out = vec![0.0; 2];
    viz.snapshot(&mut out);
    assert_eq!(bits(&out), bits(&[0.0, 0.5]));
}

#[test]
fn the_sample_rate_round_trips() {
    let viz = VisualizerShared::new(true);
    assert_eq!(viz.sample_rate(), 0);
    viz.set_sample_rate(48_000);
    assert_eq!(viz.sample_rate(), 48_000);
}

#[test]
fn the_analysis_rate_is_the_sample_rate_at_unity_speed() {
    let viz = VisualizerShared::new(true);
    viz.set_sample_rate(44_100);
    // Unity is the default and must round-trip exactly, not through the cast.
    assert_eq!(viz.analysis_rate(), 44_100);
    viz.set_speed(1.0);
    assert_eq!(viz.analysis_rate(), 44_100);
}

#[test]
fn the_analysis_rate_follows_the_playback_speed() {
    // The tap sits above the deck's converter, so a 2x listener hears every
    // frequency an octave up — the band edges have to follow or the bars plot
    // the file's pitch instead of the one playing.
    let viz = VisualizerShared::new(true);
    viz.set_sample_rate(44_100);
    viz.set_speed(2.0);
    assert_eq!(viz.analysis_rate(), 88_200);
    viz.set_speed(0.25);
    assert_eq!(viz.analysis_rate(), 11_025);
}

#[test]
fn the_analysis_rate_is_zero_until_something_has_played() {
    // No rate to scale. Zero is what tells the analyzer to skip the transform.
    let viz = VisualizerShared::new(true);
    viz.set_speed(2.0);
    assert_eq!(viz.analysis_rate(), 0);
}

#[test]
fn a_nonsense_speed_leaves_the_analysis_rate_unscaled() {
    // Falling back to the unscaled rate misplaces the bands; falling through to
    // a rate of zero would blank the display entirely.
    for speed in [0.0, -2.0, f64::NAN, f64::from(UNBOUNDED)] {
        let viz = VisualizerShared::new(true);
        viz.set_sample_rate(44_100);
        viz.set_speed(speed);
        assert_eq!(viz.analysis_rate(), 44_100, "speed {speed}");
    }
}

// --- VisualizerTap ---------------------------------------------------------

#[test]
fn the_tap_is_bit_identical_whether_enabled_or_not() {
    let input = counted(512);
    let (on, _) = run_tap(input.clone(), 2, 44_100, true, 8);
    let (off, _) = run_tap(input.clone(), 2, 44_100, false, 8);
    // The tap is transparent both ways round; only the ring differs.
    assert_eq!(bits(&on), bits(&input));
    assert_eq!(bits(&off), bits(&input));
}

#[test]
fn a_stereo_frame_is_pushed_as_its_channel_average() {
    // Four frames: (-1.0, 1.0) (-0.5, 0.5) (0.25, 0.75) (1.0, 0.0).
    let input = vec![-1.0, 1.0, -0.5, 0.5, 0.25, 0.75, 1.0, 0.0];
    let (_, ring) = run_tap(input, 2, 44_100, true, 4);
    assert_eq!(bits(&ring), bits(&[0.0, 0.0, 0.5, 0.5]));
}

#[test]
fn a_mono_source_pushes_one_value_per_sample() {
    let input = counted(6);
    let (_, ring) = run_tap(input.clone(), 1, 44_100, true, 6);
    assert_eq!(bits(&ring), bits(&input));
}

#[test]
fn the_tap_publishes_its_rate_on_the_first_frame() {
    let viz = VisualizerShared::new(true);
    let mut tap = VisualizerTap::new(TestSource::new(counted(4), 2, 48_000), &viz, 0);
    // Constructing it announces nothing — a gapless successor is built long
    // before it plays.
    assert_eq!(viz.sample_rate(), 0);
    let _ = tap.next();
    assert_eq!(viz.sample_rate(), 0);
    let _ = tap.next();
    assert_eq!(viz.sample_rate(), 48_000);
}

#[test]
fn a_partial_trailing_frame_is_dropped() {
    // Three samples of a stereo stream: one whole frame, then a stray sample
    // whose partner never arrives.
    let input = vec![0.25, 0.75, 1.0];
    let (out, ring) = run_tap(input.clone(), 2, 44_100, true, 4);
    assert_eq!(bits(&out), bits(&input));
    assert_eq!(bits(&ring), bits(&[0.0, 0.0, 0.0, 0.5]));
}

#[test]
fn seeking_realigns_the_downmix_to_the_frame_grid() {
    let viz = VisualizerShared::new(true);
    // Two stereo frames, averaging to 0.75 and 0.5.
    let data = vec![0.5, 1.0, 0.25, 0.75];
    let mut tap = VisualizerTap::new(TestSource::new(data.clone(), 2, 44_100), &viz, 0);
    // Consume half a frame, then seek. `TestSource::try_seek` rewinds to 0, so a
    // tap that kept its half-full accumulator would pair the pre-seek sample with
    // the first post-seek one and stay a channel out of step from there on.
    let _ = tap.next();
    assert!(matches!(tap.try_seek(Duration::ZERO), Ok(())));

    let rest: Vec<f32> = tap.by_ref().collect();
    assert_eq!(bits(&rest), bits(&data));

    let mut ring = vec![0.0; 2];
    viz.snapshot(&mut ring);
    assert_eq!(bits(&ring), bits(&[0.75, 0.5]));
}
