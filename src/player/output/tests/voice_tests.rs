//! Tests for one voice: the counts the transport reads, the clock, and the two control ops that
//! have to land before they return.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{Voice, VoicePull, pair};
use crate::player::audio::Shape;
use crate::player::tests::helpers::{TestSource, shape};

const RATE: u32 = 44_100;

/// One channel at `rate`, which is every device this suite stands up bar one.
fn mono(rate: u32) -> Shape {
    shape(1, rate)
}

fn silence(frames: u32, rate: u32) -> TestSource {
    TestSource::new(vec![0.0; usize::try_from(frames).unwrap_or(0)], 1, rate)
}

/// `seconds` of silence at `rate`, for the cases that assert on a clock rather than on samples.
fn seconds_of(seconds: u32, rate: u32) -> TestSource {
    silence(seconds.saturating_mul(rate), rate)
}

/// Pull `frames` from the voice on this thread, as the output callback would.
fn pump(pull: &mut VoicePull, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames];
    let reached = pull.render(&mut out);
    out.truncate(reached);
    out
}

/// A background thread standing in for the output callback, so a control op that blocks until it is
/// serviced has something to be serviced by. `tests/crossfade.rs` runs the same arrangement the
/// other way round, pulling on the test thread and driving the control op off it.
struct Callback {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Callback {
    fn start(mut pull: VoicePull) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                pump(&mut pull, 256);
                thread::sleep(Duration::from_millis(1));
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Callback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A voice must never read as empty between being fed and the callback noticing, because that is
/// exactly the window `is_crossfading` asks in — it reads the idle voice the instant after a
/// crossfade has started a source on it.
#[test]
fn append_is_counted_before_the_callback_has_seen_it() {
    let (voice, _pull) = pair(mono(RATE));

    assert!(voice.is_empty());
    voice.append(silence(64, RATE));
    assert_eq!(voice.len(), 1, "the count must not wait for the audio thread");
}

#[test]
fn a_source_that_runs_out_empties_the_voice() {
    let (voice, mut pull) = pair(mono(RATE));
    voice.append(silence(64, RATE));

    pump(&mut pull, 256);
    assert!(voice.is_empty());
}

#[test]
fn a_staged_source_starts_when_the_one_ahead_of_it_ends() {
    let (voice, mut pull) = pair(mono(RATE));
    voice.append(TestSource::new(vec![0.25; 4], 1, RATE));
    voice.append(TestSource::new(vec![0.75; 4], 1, RATE));
    assert_eq!(voice.len(), 2);

    let out = pump(&mut pull, 8);
    assert_eq!(out, vec![0.25, 0.25, 0.25, 0.25, 0.75, 0.75, 0.75, 0.75]);
    assert!(voice.is_empty());
}

/// The gapless successor is the reason the clock re-anchors on a transition rather than on a clear:
/// without it the second track's progress bar starts wherever the first one finished.
#[test]
fn the_clock_re_anchors_when_a_staged_source_takes_over() {
    let (voice, mut pull) = pair(mono(RATE));
    voice.append(silence(1_000, RATE));
    voice.append(silence(1_000, RATE));

    pump(&mut pull, 1_200);
    let position = voice.position();
    assert!(
        position < Duration::from_millis(10),
        "the successor's clock started at {position:?} rather than at its own beginning"
    );
}

/// Media time, whatever the speed. rodio counted output frames after its speed wrapper, so a
/// position had to be read on one timeline and reported on another; there is one timeline here.
#[test]
fn the_position_counts_media_frames_rather_than_output_frames() {
    for speed in [0.5, 1.0, 2.0] {
        let (voice, mut pull) = pair(mono(RATE));
        voice.append(seconds_of(1, RATE));
        voice.set_speed(speed);

        let output_frames: u32 = 4_410;
        pump(&mut pull, usize::try_from(output_frames).unwrap_or(0));

        let want = Duration::from_secs_f64(f64::from(output_frames) * speed / f64::from(RATE));
        let got = voice.position();
        assert!(
            got.abs_diff(want) < Duration::from_millis(2),
            "at speed {speed} the voice read {got:?}, expected about {want:?}"
        );
    }
}

#[test]
fn a_paused_voice_neither_advances_its_clock_nor_contributes() {
    let (voice, mut pull) = pair(mono(RATE));
    voice.append(TestSource::new(vec![0.5; 4_410], 1, RATE));

    pump(&mut pull, 441);
    let before = voice.position();
    assert!(before > Duration::ZERO);

    voice.pause();
    let out = pump(&mut pull, 441);

    assert_eq!(voice.position(), before, "a paused voice's clock moved");
    assert!(out.iter().all(|s| *s == 0.0), "a paused voice put samples in the block");
}

#[test]
fn volume_scales_what_reaches_the_block() {
    let (voice, mut pull) = pair(mono(RATE));
    voice.append(TestSource::new(vec![0.5; 16], 1, RATE));
    voice.set_volume(0.5);

    let out = pump(&mut pull, 8);
    assert!(out.iter().all(|s| (*s - 0.25).abs() < 1e-6), "{out:?}");
}

/// `Decks::cut_to` clears both decks and then starts a source on one of them, so a clear that had
/// not landed by the time it returned would take the incoming source with it.
#[test]
fn clear_returns_only_once_the_callback_has_serviced_it() {
    let (voice, pull) = pair(mono(RATE));
    let _callback = Callback::start(pull);

    voice.append(seconds_of(1, RATE));
    voice.append(seconds_of(1, RATE));
    assert_eq!(voice.len(), 2);

    voice.clear();

    assert_eq!(voice.len(), 0, "clear returned while sources were still on the voice");
    assert_eq!(voice.position(), Duration::ZERO);
    assert!(voice.is_paused(), "clear pauses the voice");
}

#[test]
fn clearing_a_voice_with_nothing_on_it_does_not_wait_for_the_callback() {
    let (voice, _pull) = pair(mono(RATE));

    // No callback is running, so anything that waited would take the full service timeout.
    let started = std::time::Instant::now();
    voice.clear();
    assert!(started.elapsed() < Duration::from_millis(100));
}

/// A voice whose source ran out keeps reporting where that source ended, which is what the transport
/// wants until it advances. What it must not do is carry that reading into the next track: both
/// `Decks::cut_to` and `Decks::crossfade_to` clear before they append, and the clear is the only
/// thing standing between a drained voice and a position that reads as the previous track's end for
/// as long as it takes the callback to pick the append up.
#[test]
fn clearing_a_voice_that_drained_on_its_own_still_rewinds_the_clock() {
    let (voice, mut pull) = pair(mono(RATE));
    voice.append(silence(64, RATE));

    pump(&mut pull, 256);
    assert!(voice.is_empty(), "the source never drained");
    assert!(voice.position() > Duration::ZERO, "a drained voice reports where its source ended");

    // Empty, so this takes the short circuit that never reaches the callback.
    voice.clear();

    assert_eq!(voice.position(), Duration::ZERO, "the clear left the drained source's position");
}

/// A seek is a `replace`, so the clock has to come from the caller's target rather than from the
/// frames the incoming source has handed over, which at the swap is none.
#[test]
fn a_replace_anchors_the_clock_where_the_caller_asked() {
    let (voice, mut pull) = pair(mono(RATE));

    voice.append(seconds_of(4, RATE));
    pump(&mut pull, 64);

    let seek = Duration::from_secs(2);
    voice.replace(seconds_of(4, RATE), seek, voice.mounted());
    pump(&mut pull, 64);

    let got = voice.position();
    assert!(
        got.abs_diff(seek) < Duration::from_millis(50),
        "the voice read {got:?} after seeking to {seek:?}"
    );
}

/// The counterpart to the in-place seek's "nothing playing is nowhere to go". A deck that drained
/// while the seek was reading the file must not have that file mounted on it: the monitor is about
/// to advance past the track, and a source arriving here would restart it under that.
#[test]
fn replacing_on_an_empty_voice_mounts_nothing() {
    let (voice, mut pull) = pair(mono(RATE));

    voice.replace(seconds_of(4, RATE), Duration::from_secs(1), voice.mounted());
    pump(&mut pull, 64);

    assert!(voice.is_empty(), "a replace mounted a source on a voice with nothing playing");
    assert_eq!(voice.position(), Duration::ZERO, "the clock moved for a source never mounted");
}

/// A seek can land on a source that has just handed over its last frame, the block boundary only
/// having to fall one frame short of the end. That source is drained but not finished, so the voice
/// still counts it and the swap has to take it rather than read the deck as empty and refuse.
#[test]
fn a_seek_keeps_a_source_that_had_just_reached_its_end() {
    const FRAMES: usize = 4_410;

    let (voice, mut pull) = pair(mono(RATE));
    voice.append(TestSource::new(vec![0.5; FRAMES], 1, RATE));

    // One short of the whole source, which leaves the converter holding its last frame with nothing
    // behind it: drained, but not yet finished.
    assert_eq!(pump(&mut pull, FRAMES - 1).len(), FRAMES - 1);
    assert_eq!(voice.len(), 1, "the source must still be on the voice to be seekable");

    voice.replace(TestSource::new(vec![0.5; FRAMES], 1, RATE), Duration::ZERO, voice.mounted());

    assert_eq!(
        pump(&mut pull, 64).len(),
        64,
        "the voice stopped at the pre-seek end of the source"
    );
    assert!(!voice.is_empty(), "the seek's own source was dropped");
}

/// The window the ticket is for: a staged gapless successor takes the deck over inside the
/// callback, so a seek that read the deck before its file was opened comes back naming a source
/// that has already ended. Mounting it would restart the track the deck just left.
#[test]
fn a_replace_prepared_against_a_source_that_has_since_ended_mounts_nothing() {
    const FRAMES: usize = 512;

    let (voice, mut pull) = pair(mono(RATE));
    voice.append(TestSource::new(vec![0.25; FRAMES], 1, RATE));
    voice.append(TestSource::new(vec![0.75; FRAMES], 1, RATE));

    // What a seek would have read before going off to open its file.
    let mounted = voice.mounted();

    // Drain the first source, which hands the deck to the staged one.
    assert_eq!(pump(&mut pull, FRAMES).len(), FRAMES);
    assert_eq!(voice.len(), 1, "the staged source should have taken over");

    voice.replace(TestSource::new(vec![0.25; FRAMES], 1, RATE), Duration::ZERO, mounted);

    let after = pump(&mut pull, 64);
    assert!(
        after.iter().all(|s| (*s - 0.75).abs() < 1e-6),
        "the stale seek mounted the track the deck had already left"
    );
    assert_eq!(voice.len(), 1, "the refused source was not accounted for");
}

/// The converter is built against the source's own shape, so a second source at a different rate
/// gets its own rather than inheriting whatever the first one negotiated. This is the fault
/// `tests/stream_rate.rs` covers end to end, asked at the level it now lives at.
#[test]
fn a_second_source_is_converted_from_its_own_rate() {
    let device = mono(48_000);
    let (voice, mut pull) = pair(device);

    voice.append(TestSource::new(vec![0.5; 12], 1, 24_000));
    voice.append(TestSource::new(vec![0.25; 12], 1, 48_000));

    // The first is upsampled 2:1 and the second passes through, so together they are 24 + 12.
    let out = pump(&mut pull, 64);
    let voiced = out.iter().filter(|s| **s != 0.0).count();
    assert_eq!(voiced, 36, "each source must be converted from the rate it reports");
}

/// A source's own `sample_rate` is what the clock divides by, so it has to follow the source rather
/// than the device — otherwise a 24 kHz station reads back at half its elapsed time.
#[test]
fn the_clock_follows_the_source_rate_not_the_device_rate() {
    let (voice, mut pull) = pair(mono(48_000));
    voice.append(silence(24_000, 24_000));

    pump(&mut pull, 48_000);
    let got = voice.position();
    assert!(
        got.abs_diff(Duration::from_secs(1)) < Duration::from_millis(10),
        "a one-second 24 kHz source read back as {got:?}"
    );
}

/// Both halves cross a thread boundary at boot — the pull into the output callback, the voice into
/// whichever task drives the transport — so losing `Send` on either is a compile error here rather
/// than at the one call site that moves them.
const _: fn() = || {
    fn check<T: Send>() {}
    check::<Voice>();
    check::<VoicePull>();
};
