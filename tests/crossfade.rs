//! End-to-end crossfade check against the real audio chain, no audio device.
//!
//! rodio's `mixer()` builds a device-less `Mixer` + `MixerSource`, so two real
//! `Player` decks can be connected to it and the summed output pulled by hand —
//! exercising the Symphonia decoder, `EqSource`'s fade stage, the visualizer
//! tap, rodio's per-deck wrappers and the unclamped sum for real.
//!
//! The fixtures are constant-amplitude DC WAVs, because two perfectly correlated
//! signals under a complementary *linear* crossfade sum to a constant: the mixed
//! output must hold steady at the source amplitude across the whole overlap and
//! never exceed it, which is exactly what keeps the unclamped mixer from
//! clipping.
//!
//! ⚠ Pulling `MixerSource` *is* the audio thread here, and some `Player` calls
//! block until that thread services them, so a control op that makes one must
//! run on a separate thread while this one keeps pulling. See [`drive_until`].

use std::io::Write;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use melodia::player::replaygain::TrackReplayGain;
use melodia::player::rodio_backend::RodioPlayer;

const RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
/// Half scale, so the clamp in `EqSource` can never mask a summing bug.
const AMPLITUDE: f32 = 0.5;
/// Long enough that rodio's fixed pipeline latency is a small fraction of the
/// ramp. Also the shipped default.
const FADE_MS: u64 = 2_000;

/// Frames discarded before asserting on steady-state amplitude: rodio's queue
/// reports a placeholder span until its first source arrives, and deck
/// volume/pause land through a `periodic_access` hook, so the start of a deck's
/// output isn't representative.
const WARMUP_FRAMES: usize = (RATE as usize) / 10;

/// Fractional slack on the summed amplitude during an overlap.
///
/// `Mixer::add` wraps each deck's queue in a `UniformSourceIterator`, which
/// buffers a span, so a freshly appended deck reaches the mixer a few hundred
/// frames behind the one already playing and the two ramps are skewed by that
/// constant. Deliberately far tighter than any real curve error — an equal-power
/// crossfade peaks at √2 and a non-complementary one dips by tens of percent —
/// so it absorbs pipeline latency and nothing else.
const SKEW: f32 = 0.01;

fn frames_for_ms(ms: u64) -> usize {
    let ms = usize::try_from(ms).unwrap_or(0);
    (ms * RATE as usize) / 1_000
}

fn nz_u16(v: u16) -> rodio::ChannelCount {
    NonZero::new(v).unwrap_or(NonZero::<u16>::MIN)
}

fn nz_u32(v: u32) -> rodio::SampleRate {
    NonZero::new(v).unwrap_or(NonZero::<u32>::MIN)
}

/// Write a 16-bit PCM WAV of constant amplitude. Hand-rolled — the project has
/// no WAV encoder dependency and the canonical header is 44 bytes.
fn write_dc_wav(path: &Path, seconds: u32, amplitude: f32) -> std::io::Result<()> {
    let frames = RATE * seconds;
    let data_len = frames * u32::from(CHANNELS) * 2;
    let mut buf = Vec::with_capacity(44 + data_len as usize);

    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_len).to_le_bytes());
    buf.extend_from_slice(b"WAVEfmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&CHANNELS.to_le_bytes());
    buf.extend_from_slice(&RATE.to_le_bytes());
    buf.extend_from_slice(&(RATE * u32::from(CHANNELS) * 2).to_le_bytes()); // byte rate
    buf.extend_from_slice(&(CHANNELS * 2).to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());

    let sample = pcm_sample(amplitude);
    for _ in 0..frames * u32::from(CHANNELS) {
        buf.extend_from_slice(&sample.to_le_bytes());
    }

    let mut f = std::fs::File::create(path)?;
    f.write_all(&buf)
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "amplitude is a small constant well inside i16 range"
)]
fn pcm_sample(amplitude: f32) -> i16 {
    (amplitude * f32::from(i16::MAX)) as i16
}

/// Pull `frames` interleaved frames and return the per-frame amplitude of
/// channel 0. `MixerSource` never ends (both decks keep-alive), so a `None`
/// here would itself be a bug.
fn pull(src: &mut rodio::mixer::MixerSource, frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames);
    for _ in 0..frames {
        let left = src.next();
        let right = src.next();
        assert!(left.is_some() && right.is_some(), "mixer must stay alive");
        if let (Some(l), Some(r)) = (left, right) {
            // Both channels carry the same DC value, so they must track. Loose
            // because the mixer's resampler can leave two decks a sample out of
            // phase; exact per-frame gain coupling is pinned against the raw
            // `EqSource` by
            // `a_fade_advances_once_per_frame_not_once_per_sample`.
            assert!((l - r).abs() < 1e-3, "channels sheared: {l} vs {r}");
            out.push(l);
        }
    }
    out
}

/// Pull `frames` frames, asserting only that the mixer stays alive.
///
/// rodio's `periodic_access` hook counts samples rather than frames, so a
/// `pause()` can take effect *between* a frame's two channels. That step is
/// rodio's and not what [`pull`]'s channel-coupling check exists to catch, so
/// flush a transition through here before asserting on what follows.
fn pull_lenient(src: &mut rodio::mixer::MixerSource, frames: usize) {
    for _ in 0..frames * usize::from(CHANNELS) {
        assert!(src.next().is_some(), "mixer must stay alive");
    }
}

/// Run a blocking `Player` control op on another thread while this one keeps the
/// mixer turning.
///
/// `clear()` on a live deck waits for rodio's `periodic_access` hook and
/// `try_seek()` for seek feedback, both of which arrive only when someone pulls
/// samples — in production the audio callback thread, here us. Pulls
/// *leniently*, the op being a transition by definition; a caller asserts
/// [`pull`]'s steady-state coupling after this returns, not during it.
fn drive_until<F>(src: &mut rodio::mixer::MixerSource, op: F)
where
    F: FnOnce() + Send + 'static,
{
    // Bound the wait so a genuine deadlock fails the test rather than hanging
    // the suite — orders of magnitude more than the service interval needs.
    let cap = RATE as usize * 2;
    let handle = std::thread::spawn(op);
    let mut pulled = 0usize;
    while !handle.is_finished() {
        assert!(pulled < cap, "control op never completed — deadlock?");
        pull_lenient(src, 64);
        pulled += 64;
    }
    assert!(handle.join().is_ok(), "control op panicked");
}

/// The load-bearing property: `MixerSource` sums its voices unclamped, so two
/// overlapping decks must never push it past the amplitude either carries alone
/// — what a complementary linear curve buys and an equal-power one would not.
/// Checked across the *whole* overlap, warmup included.
fn assert_no_clipping(samples: &[f32], what: &str) {
    let peak = samples.iter().fold(0.0_f32, |a, s| a.max(s.abs()));
    assert!(
        peak <= AMPLITUDE * (1.0 + SKEW),
        "{what}: mixer summed to {peak}, past the {AMPLITUDE} each deck carries alone — the crossfade curve is not complementary"
    );
}

/// Assert every sample sits within `tol` of `expected`. A per-sample bound, so
/// a transient excursion can't hide behind an average.
fn assert_holds_at(samples: &[f32], expected: f32, tol: f32, what: &str) {
    assert!(!samples.is_empty(), "{what}: nothing sampled");
    let worst = samples.iter().fold(0.0_f32, |a, s| a.max((s - expected).abs()));
    assert!(
        worst <= tol,
        "{what}: expected ~{expected}, worst deviation {worst} (first few: {:?})",
        &samples[..samples.len().min(4)]
    );
}

struct Fixture {
    _tmp: tempfile::TempDir,
    track_a: String,
    track_b: String,
}

fn fixture() -> std::io::Result<Fixture> {
    let tmp = tempfile::tempdir()?;
    let a: PathBuf = tmp.path().join("a.wav");
    let b: PathBuf = tmp.path().join("b.wav");
    write_dc_wav(&a, 6, AMPLITUDE)?;
    write_dc_wav(&b, 6, AMPLITUDE)?;
    Ok(Fixture {
        _tmp: tmp,
        track_a: a.to_string_lossy().into_owned(),
        track_b: b.to_string_lossy().into_owned(),
    })
}

/// `RodioPlayer` + the `MixerSource` its two decks feed.
fn player() -> (Arc<RodioPlayer>, rodio::mixer::MixerSource) {
    let (mixer, source) = rodio::mixer::mixer(nz_u16(CHANNELS), nz_u32(RATE));
    let player = RodioPlayer::new(&mixer, tokio::runtime::Handle::current());
    (Arc::new(player), source)
}

fn start(rodio: &RodioPlayer, path: &str) {
    let r = rodio.play_media(path, 1.0, 1.0, None, TrackReplayGain::default());
    assert!(r.is_ok(), "play_media failed: {:?}", r.err());
}

fn crossfade_into(rodio: &RodioPlayer, path: &str) {
    let r = rodio.begin_crossfade(path, TrackReplayGain::default(), FADE_MS, 1.0, 1.0);
    assert!(r.is_ok(), "begin_crossfade failed: {:?}", r.err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crossfade_overlaps_two_decks_without_ever_clipping_the_mixer() -> std::io::Result<()> {
    let fx = fixture()?;
    let (rodio, mut mix) = player();
    start(&rodio, &fx.track_a);

    // Deck A alone, at full amplitude.
    let solo = pull(&mut mix, WARMUP_FRAMES * 2);
    assert_holds_at(&solo[WARMUP_FRAMES..], AMPLITUDE, 1e-3, "deck A alone");
    assert!(!rodio.is_crossfading());

    crossfade_into(&rodio, &fx.track_b);
    assert!(rodio.is_crossfading(), "the outgoing deck is still draining");

    // Two correlated DC signals under complementary linear ramps must sum to a
    // constant: never above, the mixer not clamping, and never meaningfully below.
    let during = pull(&mut mix, frames_for_ms(FADE_MS));
    assert_no_clipping(&during, "auto crossfade");
    assert_holds_at(&during[WARMUP_FRAMES..], AMPLITUDE, AMPLITUDE * SKEW, "mid-overlap sum");

    // Past the ramp the outgoing deck has ended itself, which is the backend's
    // signal that the overlap is over.
    let after = pull(&mut mix, WARMUP_FRAMES * 2);
    assert!(!rodio.is_crossfading(), "outgoing deck must drain when its ramp lands");
    assert_holds_at(&after[WARMUP_FRAMES..], AMPLITUDE, 1e-3, "deck B alone");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_visualizer_tap_reads_the_mix_not_the_two_decks_interleaved() -> std::io::Result<()> {
    /// A spectrum window's worth — 46 ms at this rate.
    const WINDOW: usize = 2_048;

    let fx = fixture()?;
    let (rodio, mut mix) = player();
    // The backend's own spelling; the Now-Playing view reaches the same cell
    // through `visualizer()`, one line down.
    rodio.set_visualizer_enabled(true);
    let viz = rodio.visualizer();
    let mut window = vec![0.0; WINDOW];

    start(&rodio, &fx.track_a);
    pull(&mut mix, WARMUP_FRAMES * 2);

    viz.snapshot(&mut window);
    assert_holds_at(&window, AMPLITUDE, 1e-3, "one deck playing");

    crossfade_into(&rodio, &fx.track_b);
    // A quarter of the way in, so the two ramps are far apart.
    pull(&mut mix, frames_for_ms(FADE_MS / 4));
    viz.snapshot(&mut window);
    // The same property the mixer output above is pinned on, which is the point:
    // the tap must see what the mixer sees. Interleaved into one ring instead,
    // this window would alternate between the two ramps sample by sample and
    // average half the level — which a per-sample bound cannot hide.
    assert_holds_at(&window, AMPLITUDE, AMPLITUDE * SKEW, "mid-overlap tap");

    // The handover: the outgoing ramp lands, its source ends and releases the
    // ring, and the survivor carries the window alone.
    pull(&mut mix, frames_for_ms(FADE_MS) + WARMUP_FRAMES * 2);
    assert!(!rodio.is_crossfading(), "outgoing deck must drain when its ramp lands");
    viz.snapshot(&mut window);
    assert_holds_at(&window, AMPLITUDE, 1e-3, "after the overlap");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seeking_mid_crossfade_drops_the_outgoing_deck_and_restores_unity() -> std::io::Result<()> {
    let fx = fixture()?;
    let (rodio, mut mix) = player();
    start(&rodio, &fx.track_a);
    let _ = pull(&mut mix, WARMUP_FRAMES);

    crossfade_into(&rodio, &fx.track_b);

    // Land mid-overlap, where the incoming deck sits at ~half gain.
    let _ = pull(&mut mix, frames_for_ms(FADE_MS) / 2);

    // `seek` clears the live outgoing deck and seeks the survivor; both calls
    // block on the audio thread, which is us.
    let r = rodio.clone();
    drive_until(&mut mix, move || r.seek(0));
    assert!(!rodio.is_crossfading(), "a seek must abort the crossfade");

    // The survivor ramps back to unity from its partial fade-in gain. Had the
    // abort left it stranded, this would sit low.
    let after = pull(&mut mix, WARMUP_FRAMES * 2);
    assert_holds_at(&after[WARMUP_FRAMES..], AMPLITUDE, 1e-3, "surviving deck after abort");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_manual_crossfade_also_holds_the_amplitude() -> std::io::Result<()> {
    // With `crossfade_manual` on, `play_media` fades rather than cuts. It clears
    // the *idle* target deck first, which doesn't block.
    let fx = fixture()?;
    let (rodio, mut mix) = player();
    rodio.set_crossfade_enabled(true);
    rodio.set_crossfade_manual(true);
    rodio.set_crossfade_duration_ms(u32::try_from(FADE_MS).unwrap_or(2_000));

    start(&rodio, &fx.track_a);
    let solo = pull(&mut mix, WARMUP_FRAMES * 2);
    assert_holds_at(&solo[WARMUP_FRAMES..], AMPLITUDE, 1e-3, "deck A alone");

    // Now a "manual next" onto the other deck.
    let r = rodio.clone();
    let path = fx.track_b.clone();
    drive_until(&mut mix, move || start(&r, &path));
    assert!(rodio.is_crossfading(), "a manual track change must fade, not cut");

    let during = pull(&mut mix, frames_for_ms(FADE_MS));
    assert_no_clipping(&during, "manual crossfade");
    assert_holds_at(
        &during[WARMUP_FRAMES..],
        AMPLITUDE,
        AMPLITUDE * SKEW,
        "manual mid-overlap sum",
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plain_play_is_transparent() -> std::io::Result<()> {
    // Crossfade off must leave the signal exactly as decoded — the fade cell is
    // idle, so `EqSource` keeps its bypass path. The mixer's own rate/channel
    // adapter sits in the way, so compare against the decoded PCM value rather
    // than bit-for-bit.
    let fx = fixture()?;
    let (rodio, mut mix) = player();
    start(&rodio, &fx.track_a);

    let out = pull(&mut mix, WARMUP_FRAMES * 2);
    let expected = f32::from(pcm_sample(AMPLITUDE)) / 32_768.0;
    assert_holds_at(&out[WARMUP_FRAMES..], expected, 1e-6, "bypass passthrough");
    assert!(!rodio.is_crossfading());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_a_paused_deck_clears_it_immediately() -> std::io::Result<()> {
    // `player_stop` passes the pause-fade length whatever the player is doing,
    // so a stop routinely lands on an already-paused deck — which is never
    // pulled, so a ramp armed on it never advances. Deferring the clear behind
    // it would leave the decks loaded while the UI already reads Stopped.
    let fx = fixture()?;
    let (rodio, mut mix) = player();
    start(&rodio, &fx.track_a);
    let _ = pull(&mut mix, WARMUP_FRAMES);

    // `pause()` is the immediate one, so the deck is genuinely paused with no
    // deferred op pending — the state under test. Flush the boundary so what
    // follows runs against steady silence.
    rodio.pause();
    pull_lenient(&mut mix, WARMUP_FRAMES);

    let stopper = Arc::clone(&rodio);
    drive_until(&mut mix, move || {
        stopper.stop_with_fade(melodia::player::crossfade::PAUSE_FADE_MS);
    });

    assert_eq!(
        rodio.check_playback_state(),
        melodia::player::rodio_backend::PlaybackCheck::EndOfStream,
        "the decks must be cleared by the time `stop_with_fade` returns, not \
         behind a deferred clear waiting on a ramp that can never advance"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zero_length_pause_fade_silences_the_deck_at_once() -> std::io::Result<()> {
    // Next / previous pressed *while paused* emit `PlayMedia` — which starts the
    // deck — then `Pause { fade_ms: 0 }` purely to restore the paused state. A
    // fade there would ramp the freshly-started track down from full volume
    // rather than pausing it, making its first quarter-second audible.
    let fx = fixture()?;
    let (rodio, mut mix) = player();
    // Fade-on-pause ON, so the length passed in is the only thing that can make
    // this immediate — the backend must not reach for the setting itself.
    rodio.set_crossfade_fade_on_pause(true);
    start(&rodio, &fx.track_a);
    let _ = pull(&mut mix, WARMUP_FRAMES);

    rodio.pause_with_fade(0);
    // A pause lands through rodio's `periodic_access` hook, so flush a generous
    // window first.
    pull_lenient(&mut mix, frames_for_ms(20));

    // Well inside a `PAUSE_FADE_MS` ramp, which would still be near full volume
    // across all of this — so silence here is the whole point.
    let after = pull(&mut mix, frames_for_ms(50));
    assert_holds_at(&after, 0.0, 1e-4, "a zero-length pause fade");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_with_a_gapless_track_staged_clears_both_decks_at_once() -> std::io::Result<()> {
    // A staged gapless source shares its deck's fade cell, so a fade-out armed
    // there would be inherited the moment the outgoing source drained — starting
    // at full volume and audibly fading out. `can_fade_out` refuses while one is
    // staged, and the immediate stop takes the staged source with it.
    //
    // The alternative — arming the fade and clearing `gapless_pending` eagerly —
    // leaves the flag lying for the length of the ramp: a `play_media` landing
    // there reads "nothing staged", takes the manual-crossfade branch that
    // clears only the *idle* deck, and strands the staged source behind an
    // outgoing track armed to self-end.
    let fx = fixture()?;
    let (rodio, mut mix) = player();
    rodio.set_crossfade_fade_on_pause(true);
    start(&rodio, &fx.track_a);
    let _ = pull(&mut mix, WARMUP_FRAMES);

    rodio.preload_gapless(Some(&fx.track_b), TrackReplayGain::default());
    assert!(rodio.is_gapless_preloaded(), "the next track must really be staged");

    let stopper = Arc::clone(&rodio);
    drive_until(&mut mix, move || {
        stopper.stop_with_fade(melodia::player::crossfade::PAUSE_FADE_MS);
    });

    // The load-bearing one: `EndOfStream` needs the active deck genuinely
    // *empty*, so it holds only if the staged source went with the outgoing one.
    // Clearing `gapless_pending` alone leaves the deck two sources deep and
    // still reporting `Playing`.
    assert_eq!(
        rodio.check_playback_state(),
        melodia::player::rodio_backend::PlaybackCheck::EndOfStream,
        "both decks — staged source included — must be cleared by the time \
         `stop_with_fade` returns, not behind a deferred clear"
    );
    assert!(!rodio.is_gapless_preloaded(), "and the flag must agree");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pausing_with_a_gapless_track_staged_is_immediate() -> std::io::Result<()> {
    // Same gate as the stop above, and the same reason: a fade here holds the
    // deck at zero gain while the outgoing source drains into the staged one,
    // which then inherits the ramp and burns its own first quarter-second.
    let fx = fixture()?;
    let (rodio, mut mix) = player();
    rodio.set_crossfade_fade_on_pause(true);
    start(&rodio, &fx.track_a);
    let _ = pull(&mut mix, WARMUP_FRAMES);

    rodio.preload_gapless(Some(&fx.track_b), TrackReplayGain::default());
    assert!(rodio.is_gapless_preloaded(), "the next track must really be staged");

    rodio.pause_with_fade(melodia::player::crossfade::PAUSE_FADE_MS);
    pull_lenient(&mut mix, frames_for_ms(20));

    // A ramp would still be near full volume this early, so silence is the tell.
    let after = pull(&mut mix, frames_for_ms(50));
    assert_holds_at(&after, 0.0, 1e-4, "a pause with a gapless track staged");

    // A pause keeps the deck contents, so the staged source and the flag must
    // both survive it.
    assert!(rodio.is_gapless_preloaded(), "a pause must not discard the staged source");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deferred_clear_takes_a_late_preload_with_it() -> std::io::Result<()> {
    // `can_fade_out` only refuses a fade for a source staged *before* it looks. A
    // preload entering after `stop_with_fade`'s epoch bump snapshots the new
    // epoch, passes its own re-check and really does stage while the ramp is in
    // flight — the monitor calling `preload_gapless` off the `exec_lock` that
    // serializes every other control op. The deferred clear is `stop()`'s
    // deferred half, so it must take that source *and* the flag; leaving the
    // flag set over empty decks reads as a `GaplessTransition` that never
    // happened and advances the queue out of a stopped player.
    let fx = fixture()?;
    let (rodio, mut mix) = player();
    rodio.set_crossfade_fade_on_pause(true);
    start(&rodio, &fx.track_a);
    let _ = pull(&mut mix, WARMUP_FRAMES);

    // Nothing staged yet, so the fade is allowed and the clear really defers.
    rodio.stop_with_fade(melodia::player::crossfade::PAUSE_FADE_MS);
    rodio.preload_gapless(Some(&fx.track_b), TrackReplayGain::default());
    assert!(
        rodio.is_gapless_preloaded(),
        "a preload past the epoch bump must still be able to stage — that is the \
         window under test"
    );

    // ⚠ Poll the lock-free flag only. `check_playback_state` takes the decks
    // lock, which the deferred task holds while `Player::clear()` waits on the
    // audio thread — this one — so reaching for it here deadlocks the test. The
    // flag is stored *after* the clear for the same reason.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while rodio.is_gapless_preloaded() {
        assert!(std::time::Instant::now() < deadline, "the deferred clear never landed");
        pull_lenient(&mut mix, 64);
    }

    assert_eq!(
        rodio.check_playback_state(),
        melodia::player::rodio_backend::PlaybackCheck::EndOfStream,
        "the deferred clear must empty both decks, staged source included"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_preload_that_outlives_the_stop_it_raced_is_refused() -> std::io::Result<()> {
    // The mirror of the test above, and the half the deferred clear's flag-drop
    // can't reach: neither `stop()` nor `DeferredOp::ClearAll` bumps the deck
    // epoch, so a preload that snapshots it, decodes slowly, and reaches the deck
    // lock *after* the stop emptied the decks passes its own re-check, appends
    // behind nothing and re-sets `gapless_pending` over decks the stop cleared.
    // `preload_gapless` refuses an empty active deck for exactly that reason.
    //
    // Calling the preload after `stop()` returns *is* that race — the epoch it
    // reads is the post-bump one either way, which is why the epoch cannot be
    // what catches this.
    let fx = fixture()?;
    let (rodio, mut mix) = player();
    start(&rodio, &fx.track_a);
    let _ = pull(&mut mix, WARMUP_FRAMES);

    // `stop()` clears a live deck, so it blocks on the audio thread — us.
    let stopper = Arc::clone(&rodio);
    drive_until(&mut mix, move || stopper.stop());

    rodio.preload_gapless(Some(&fx.track_b), TrackReplayGain::default());

    assert!(
        !rodio.is_gapless_preloaded(),
        "a preload behind an emptied deck must be refused, not staged"
    );
    assert_eq!(
        rodio.check_playback_state(),
        melodia::player::rodio_backend::PlaybackCheck::EndOfStream,
        "and the decks must stay empty — a source staged here would read as a \
         `GaplessTransition` off a stopped player"
    );
    Ok(())
}
