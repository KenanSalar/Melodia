//! Tests for one voice: the counts the transport reads, the clock, and the two control ops that
//! have to land before they return.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{Deck, DeckVoice, pair};
use crate::player::output::convert::Shape;
use crate::player::tests::helpers::{TestSource, nz_u16, nz_u32};

const RATE: u32 = 44_100;

fn mono(rate: u32) -> Shape {
    Shape {
        channels: nz_u16(1),
        rate: nz_u32(rate),
    }
}

fn silence(frames: u32, rate: u32) -> TestSource {
    TestSource::new(vec![0.0; usize::try_from(frames).unwrap_or(0)], 1, rate)
}

/// `seconds` of silence at `rate`, for the cases that assert on a clock rather than on samples.
fn seconds_of(seconds: u32, rate: u32) -> TestSource {
    silence(seconds.saturating_mul(rate), rate)
}

/// Pull `frames` from the voice on this thread, as the output callback would.
fn pump(voice: &mut DeckVoice, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0; frames];
    let reached = voice.render(&mut out);
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
    fn start(mut voice: DeckVoice) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                pump(&mut voice, 256);
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

/// A deck must never read as empty between being fed and the callback noticing, because that is
/// exactly the window `is_crossfading` asks in — it reads the idle deck the instant after a
/// crossfade has started a source on it.
#[test]
fn append_is_counted_before_the_callback_has_seen_it() {
    let (deck, _voice) = pair(mono(RATE));

    assert!(deck.is_empty());
    deck.append(silence(64, RATE));
    assert_eq!(deck.len(), 1, "the count must not wait for the audio thread");
}

#[test]
fn a_source_that_runs_out_empties_the_deck() {
    let (deck, mut voice) = pair(mono(RATE));
    deck.append(silence(64, RATE));

    pump(&mut voice, 256);
    assert!(deck.is_empty());
}

#[test]
fn a_staged_source_starts_when_the_one_ahead_of_it_ends() {
    let (deck, mut voice) = pair(mono(RATE));
    deck.append(TestSource::new(vec![0.25; 4], 1, RATE));
    deck.append(TestSource::new(vec![0.75; 4], 1, RATE));
    assert_eq!(deck.len(), 2);

    let out = pump(&mut voice, 8);
    assert_eq!(out, vec![0.25, 0.25, 0.25, 0.25, 0.75, 0.75, 0.75, 0.75]);
    assert!(deck.is_empty());
}

/// The gapless successor is the reason the clock re-anchors on a transition rather than on a clear:
/// without it the second track's progress bar starts wherever the first one finished.
#[test]
fn the_clock_re_anchors_when_a_staged_source_takes_over() {
    let (deck, mut voice) = pair(mono(RATE));
    deck.append(silence(1_000, RATE));
    deck.append(silence(1_000, RATE));

    pump(&mut voice, 1_200);
    let position = deck.position();
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
        let (deck, mut voice) = pair(mono(RATE));
        deck.append(seconds_of(1, RATE));
        deck.set_speed(speed);

        let output_frames: u32 = 4_410;
        pump(&mut voice, usize::try_from(output_frames).unwrap_or(0));

        let want = Duration::from_secs_f64(f64::from(output_frames) * speed / f64::from(RATE));
        let got = deck.position();
        assert!(
            got.abs_diff(want) < Duration::from_millis(2),
            "at speed {speed} the deck read {got:?}, expected about {want:?}"
        );
    }
}

#[test]
fn a_paused_deck_neither_advances_its_clock_nor_contributes() {
    let (deck, mut voice) = pair(mono(RATE));
    deck.append(TestSource::new(vec![0.5; 4_410], 1, RATE));

    pump(&mut voice, 441);
    let before = deck.position();
    assert!(before > Duration::ZERO);

    deck.pause();
    let out = pump(&mut voice, 441);

    assert_eq!(deck.position(), before, "a paused deck's clock moved");
    assert!(out.iter().all(|s| *s == 0.0), "a paused deck put samples in the block");
}

#[test]
fn volume_scales_what_reaches_the_block() {
    let (deck, mut voice) = pair(mono(RATE));
    deck.append(TestSource::new(vec![0.5; 16], 1, RATE));
    deck.set_volume(0.5);

    let out = pump(&mut voice, 8);
    assert!(out.iter().all(|s| (*s - 0.25).abs() < 1e-6), "{out:?}");
}

/// `Decks::cut_to` clears both decks and then starts a source on one of them, so a clear that had
/// not landed by the time it returned would take the incoming source with it.
#[test]
fn clear_returns_only_once_the_callback_has_serviced_it() {
    let (deck, voice) = pair(mono(RATE));
    let _callback = Callback::start(voice);

    deck.append(seconds_of(1, RATE));
    deck.append(seconds_of(1, RATE));
    assert_eq!(deck.len(), 2);

    deck.clear();

    assert_eq!(deck.len(), 0, "clear returned while sources were still on the deck");
    assert_eq!(deck.position(), Duration::ZERO);
    assert!(deck.is_paused(), "clear pauses the deck, as rodio's did");
}

#[test]
fn clearing_a_deck_with_nothing_on_it_does_not_wait_for_the_callback() {
    let (deck, _voice) = pair(mono(RATE));

    // No callback is running, so anything that waited would take the full service timeout.
    let started = std::time::Instant::now();
    deck.clear();
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn a_seek_re_anchors_the_clock_on_where_it_asked_to_be() {
    let (deck, voice) = pair(mono(RATE));
    let _callback = Callback::start(voice);

    deck.append(seconds_of(4, RATE));
    let seek = Duration::from_secs(2);
    assert!(deck.try_seek(seek).is_ok());

    let got = deck.position();
    assert!(
        got.abs_diff(seek) < Duration::from_millis(50),
        "the deck read {got:?} after seeking to {seek:?}"
    );
}

#[test]
fn seeking_an_empty_deck_is_not_an_error() {
    let (deck, _voice) = pair(mono(RATE));
    assert!(deck.try_seek(Duration::from_secs(1)).is_ok());
}

/// A seek can land on a source that has just handed over its last frame, the block boundary only
/// having to fall one frame short of the end. Told nothing, the converter ends that source again on
/// its next advance and the deck drops the very track the seek had moved.
///
/// Driven through `DeckVoice::seek` rather than `Deck::try_seek` because the window is one frame
/// wide and the public op blocks until a callback services it, which here would be the pull that
/// closes the window.
#[test]
fn a_seek_keeps_a_source_that_had_just_reached_its_end() {
    const FRAMES: usize = 4_410;

    let (deck, mut voice) = pair(mono(RATE));
    deck.append(TestSource::new(vec![0.5; FRAMES], 1, RATE));

    // One short of the whole source, which leaves the converter holding its last frame with nothing
    // behind it: drained, but not yet finished.
    assert_eq!(pump(&mut voice, FRAMES - 1).len(), FRAMES - 1);
    assert_eq!(deck.len(), 1, "the source must still be on the deck to be seekable");

    voice.seek(Duration::ZERO);

    assert_eq!(
        pump(&mut voice, 64).len(),
        64,
        "the deck stopped at the pre-seek end of the source"
    );
    assert!(!deck.is_empty(), "the seek's own source was dropped");
}

/// The converter is built against the source's own shape, so a second source at a different rate
/// gets its own rather than inheriting whatever the first one negotiated. This is the fault
/// `tests/stream_rate.rs` covers end to end, asked at the level it now lives at.
#[test]
fn a_second_source_is_converted_from_its_own_rate() {
    let device = mono(48_000);
    let (deck, mut voice) = pair(device);

    deck.append(TestSource::new(vec![0.5; 12], 1, 24_000));
    deck.append(TestSource::new(vec![0.25; 12], 1, 48_000));

    // The first is upsampled 2:1 and the second passes through, so together they are 24 + 12.
    let out = pump(&mut voice, 64);
    let voiced = out.iter().filter(|s| **s != 0.0).count();
    assert_eq!(voiced, 36, "each source must be converted from the rate it reports");
}

/// A source's own `sample_rate` is what the clock divides by, so it has to follow the source rather
/// than the device — otherwise a 24 kHz station reads back at half its elapsed time.
#[test]
fn the_clock_follows_the_source_rate_not_the_device_rate() {
    let (deck, mut voice) = pair(mono(48_000));
    deck.append(silence(24_000, 24_000));

    pump(&mut voice, 48_000);
    let got = deck.position();
    assert!(
        got.abs_diff(Duration::from_secs(1)) < Duration::from_millis(10),
        "a one-second 24 kHz source read back as {got:?}"
    );
}

/// Both halves cross a thread boundary at boot — the voice into the output callback, the deck into
/// whichever task drives the transport — so losing `Send` on either is a compile error here rather
/// than at the one call site that moves them.
const _: fn() = || {
    fn check<T: Send>() {}
    check::<Deck>();
    check::<DeckVoice>();
};
