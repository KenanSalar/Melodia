//! Audio crossfade: the ramp cell the audio thread reads, the master settings cell the control
//! layer writes, and the two pure predicates that decide when a transition becomes a crossfade.
//!
//! **`src/player/CLAUDE.md` argues the design** — why the ramp lives inside [`EqSource`] rather
//! than on `Player::set_volume`, and why the curve is complementary linear (the mixer sums with no
//! clamping, so `g_out + g_in ≡ 1` is what stops it clipping). The constants below carry the
//! bounds a later edit could reverse.
//!
//! [`EqSource`]: super::equalizer::EqSource

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use super::dsp::{AtomicF32, Generation};
use melodia_core::entities::track::TrackSummary;

/// Shortest crossfade the user can select.
///
/// **Bounded from below by the playback monitor's poll interval.** [`should_crossfade`]'s window
/// is sampled once per poll, so a narrower one can be stepped clean over and the crossfade never
/// happens — and since [`crossfade_eligible`] has already suppressed the gapless preload, the miss
/// degrades to a *gapped* hard cut. `handlers.rs` pins
/// `MIN_CROSSFADE_MS >= MIN_FADE_MS + POLL_INTERVAL_MS` with a `const` assert.
pub const MIN_CROSSFADE_MS: u32 = 1_000;
/// Longest crossfade the user can select.
pub const MAX_CROSSFADE_MS: u32 = 12_000;
/// Default crossfade length. Matches Strawberry's default.
pub const DEFAULT_CROSSFADE_MS: u32 = 2_000;

/// Below this much remaining media, don't start a crossfade at all — let the track drain to
/// `EndOfStream` normally. Keeps the fade from being clamped *up* past the real remaining audio,
/// which would cut the outgoing track at a non-zero gain (an audible click).
pub const MIN_FADE_MS: u64 = 250;

/// How long the surviving deck takes to climb back to unity when a crossfade is aborted. Long
/// enough to avoid a step discontinuity, short enough to be imperceptible.
///
/// Only reaches the ear when the seek that aborted then *bails* — an empty deck, a file that
/// would not reopen. A seek that lands rebuilds the source, and a fresh [`EqSource`] has no gain
/// of its own for [`FadeCmd::start`] = `None` to resume from, so it opens at unity. Nothing is
/// lost there: the audio either side of a seek is a splice already.
///
/// [`EqSource`]: super::equalizer::EqSource
pub const ABORT_RAMP_MS: u64 = 40;

/// Fade length for pause / resume / user-initiated stop when [`CrossfadeSettings::fade_on_pause`]
/// is on. Matches Strawberry's default.
pub const PAUSE_FADE_MS: u64 = 250;

/// A target gain this close to unity lets the source disengage the fade stage entirely and fall
/// back to its bit-identical bypass path.
const UNITY_EPSILON: f32 = 1e-4;

/// Clamp a user-supplied crossfade duration into the supported range.
#[must_use]
pub fn clamp_crossfade_ms(ms: u32) -> u32 {
    ms.clamp(MIN_CROSSFADE_MS, MAX_CROSSFADE_MS)
}

/// The settings slider's seconds → the milliseconds everything else stores. Clamped in float
/// space, so the narrowing cast always lands in range.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value is clamped to MIN_CROSSFADE_MS..=MAX_CROSSFADE_MS before the cast, so it is a small non-negative integer"
)]
#[must_use]
pub fn secs_to_crossfade_ms(secs: f32) -> u32 {
    let ms = (f64::from(secs) * 1000.0)
        .round()
        .clamp(f64::from(MIN_CROSSFADE_MS), f64::from(MAX_CROSSFADE_MS));
    ms as u32
}

/// Milliseconds → seconds, for seeding the settings slider.
#[allow(
    clippy::cast_precision_loss,
    reason = "clamped to at most MAX_CROSSFADE_MS, far inside f32's exact-integer range"
)]
#[must_use]
pub fn crossfade_ms_to_secs(ms: u32) -> f32 {
    clamp_crossfade_ms(ms) as f32 / 1000.0
}

/// Gain at ramp position `pos` of `total`, interpolating `start` → `target`. A zero `total`, or a
/// `pos` past the end, lands on `target`.
#[allow(
    clippy::cast_precision_loss,
    reason = "pos <= total and both are bounded by a few seconds of samples, well inside f32's exact-integer range"
)]
#[must_use]
pub fn ramp_gain(start: f32, target: f32, pos: u64, total: u64) -> f32 {
    if total == 0 || pos >= total {
        return target;
    }
    let t = pos as f32 / total as f32;
    start + (target - start) * t
}

/// Whether a completed ramp to `target` returns the source to transparency.
#[must_use]
pub fn is_unity_target(target: f32) -> bool {
    (target - 1.0).abs() < UNITY_EPSILON
}

/// Two tracks belong to the same album. Requires the album tag to be *present* — two `None` albums
/// comparing equal would mark every transition in an untagged library same-album, and it would
/// never crossfade.
#[must_use]
pub fn same_album(a: &TrackSummary, b: &TrackSummary) -> bool {
    a.album.is_some() && a.album == b.album && a.artist == b.artist
}

/// `kind` discriminants, raw `u8` because the cell is read on the audio thread through an
/// [`AtomicU8`].
const KIND_IDLE: u8 = 0;
const KIND_RAMP: u8 = 1;

/// A ramp command, read by the source when the generation advances.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FadeCmd {
    /// `None` means "ramp from whatever gain the source is currently at" — used to fade out a
    /// track already playing at unity, and to recover a partially faded-in track on abort.
    pub start: Option<f32>,
    pub target: f32,
    /// Ramp length in **media** milliseconds. Only the source can convert it — the two decks may
    /// hold tracks at different sample rates.
    pub ramp_ms: u64,
    /// Fade-out only: the source ends once the ramp lands, draining its deck, which is how
    /// `engine::backend::PlaybackEngine::is_crossfading` sees the overlap finish.
    pub end_on_complete: bool,
}

/// Lock-free ramp state on [`EqShared`](super::equalizer::EqShared)'s generation-poll pattern.
///
/// The cell is **deck-scoped**, not source-scoped: exactly two exist for the app's lifetime, each
/// cloned into every [`EqSource`] appended to its deck. So a fade armed on a deck applies to
/// whatever that deck is playing, a gapless-appended successor included.
///
/// [`EqSource`]: super::equalizer::EqSource
pub struct FadeShared {
    generation: Generation,
    kind: AtomicU8,
    /// `f32::NAN` stands for [`FadeCmd::start`] = `None`.
    start: AtomicF32,
    target: AtomicF32,
    ramp_ms: AtomicU64,
    end_on_complete: AtomicBool,
}

impl FadeShared {
    /// A cell in the idle state — the source applies no gain and keeps its bit-identical bypass
    /// path.
    #[must_use]
    pub fn idle() -> Arc<Self> {
        Arc::new(Self {
            generation: Generation::new(),
            kind: AtomicU8::new(KIND_IDLE),
            start: AtomicF32::new(f32::NAN),
            target: AtomicF32::new(1.0),
            ramp_ms: AtomicU64::new(0),
            end_on_complete: AtomicBool::new(false),
        })
    }

    /// Publish a state change — see [`Generation::bump`].
    fn bump(&self) {
        self.generation.bump();
    }

    /// Arm a ramp. Replaces any ramp already in flight on this deck.
    pub fn arm(&self, start: Option<f32>, target: f32, ramp_ms: u64, end_on_complete: bool) {
        self.start.store(start.unwrap_or(f32::NAN));
        self.target.store(target);
        self.ramp_ms.store(ramp_ms, Ordering::Relaxed);
        self.end_on_complete.store(end_on_complete, Ordering::Relaxed);
        self.kind.store(KIND_RAMP, Ordering::Relaxed);
        self.bump();
    }

    /// Return the deck to transparency. Any source on it drops back to its bypass fast path on the
    /// next sample.
    pub fn reset(&self) {
        self.kind.store(KIND_IDLE, Ordering::Relaxed);
        self.bump();
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// Read the current command. `None` when the cell is idle.
    #[must_use]
    pub fn snapshot(&self) -> Option<FadeCmd> {
        if self.kind.load(Ordering::Relaxed) != KIND_RAMP {
            return None;
        }
        let start = self.start.load();
        Some(FadeCmd {
            start: if start.is_nan() { None } else { Some(start) },
            target: self.target.load(),
            ramp_ms: self.ramp_ms.load(Ordering::Relaxed),
            end_on_complete: self.end_on_complete.load(Ordering::Relaxed),
        })
    }
}

/// A `Copy` snapshot of the settings, taken once per decision so the flags can't shear against
/// each other mid-evaluation.
#[allow(
    clippy::struct_excessive_bools,
    reason = "one field per independent user-facing toggle; a bitflags wrapper would only obscure them"
)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CrossfadeSettings {
    pub enabled: bool,
    pub duration_ms: u32,
    /// Also crossfade when the user picks a track / hits next or previous.
    pub manual: bool,
    /// Leave same-album transitions gapless (protects continuous-mix albums).
    pub skip_same_album: bool,
    /// Fade out on pause and user-initiated stop; fade back in on resume.
    pub fade_on_pause: bool,
}

/// Lock-free crossfade settings. Unlike [`FadeShared`] this is read by the *control* layer (the
/// playback monitor and the backend), never by the audio thread, so it needs no generation
/// counter.
pub struct CrossfadeShared {
    enabled: AtomicBool,
    duration_ms: AtomicU32,
    manual: AtomicBool,
    skip_same_album: AtomicBool,
    fade_on_pause: AtomicBool,
}

impl CrossfadeShared {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(false),
            duration_ms: AtomicU32::new(DEFAULT_CROSSFADE_MS),
            manual: AtomicBool::new(false),
            skip_same_album: AtomicBool::new(true),
            fade_on_pause: AtomicBool::new(false),
        })
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn set_duration_ms(&self, ms: u32) {
        self.duration_ms.store(clamp_crossfade_ms(ms), Ordering::Relaxed);
    }

    pub fn set_manual(&self, on: bool) {
        self.manual.store(on, Ordering::Relaxed);
    }

    pub fn set_skip_same_album(&self, on: bool) {
        self.skip_same_album.store(on, Ordering::Relaxed);
    }

    pub fn set_fade_on_pause(&self, on: bool) {
        self.fade_on_pause.store(on, Ordering::Relaxed);
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn fade_on_pause(&self) -> bool {
        self.fade_on_pause.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn snapshot(&self) -> CrossfadeSettings {
        CrossfadeSettings {
            enabled: self.enabled.load(Ordering::Relaxed),
            duration_ms: self.duration_ms.load(Ordering::Relaxed),
            manual: self.manual.load(Ordering::Relaxed),
            skip_same_album: self.skip_same_album.load(Ordering::Relaxed),
            fade_on_pause: self.fade_on_pause.load(Ordering::Relaxed),
        }
    }
}

/// **Timing-independent**: is this transition a crossfade transition at all?
///
/// The monitor gates its late gapless preload on `!crossfade_eligible`, which is why this must not
/// read the current position. Gated on [`should_crossfade`] instead, any crossfade shorter than
/// `PRELOAD_LEAD_MS` would fire the preload first — setting `gapless_pending` and permanently
/// blocking the crossfade through its own `!gapless_pending` gate.
#[must_use]
pub fn crossfade_eligible(
    xf: CrossfadeSettings,
    pause_at_end: bool,
    has_next: bool,
    same_album: bool,
) -> bool {
    xf.enabled
        && xf.duration_ms > 0
        && has_next
        // Sleep-timer "pause at end of track" needs the track to drain to `EndOfStream`, the only
        // boundary that gate can catch.
        && !pause_at_end
        && !(xf.skip_same_album && same_album)
}

/// Adds the timing and liveness terms to [`crossfade_eligible`], returning the fade length in
/// **media** milliseconds or `None` to leave this tick alone.
///
/// The length is the *actual* remaining media, never clamped up to the configured duration, so the
/// ramp lands exactly on the declared track end — which self-corrects for poll granularity.
///
/// The window doubles as a stale-position filter, and stays one now that a deck re-anchors its
/// clock on every source it starts and zeroes it on every clear. What it still catches is the gap
/// between the monitor reading a position and acting on it: too high saturates `remaining` to zero,
/// too low pushes it past the cap. Both arms are cheap and both are pinned.
#[must_use]
pub fn should_crossfade(
    eligible: bool,
    gapless_pending: bool,
    is_crossfading: bool,
    position_ms: u64,
    duration_ms: u64,
    duration_cap_ms: u32,
) -> Option<u64> {
    if !eligible || gapless_pending || is_crossfading {
        return None;
    }
    if duration_ms == 0 || position_ms == 0 {
        return None;
    }
    let remaining = duration_ms.saturating_sub(position_ms);
    if remaining < MIN_FADE_MS || remaining > u64::from(duration_cap_ms) {
        return None;
    }
    Some(remaining)
}

/// What the playback monitor decided, plus the state it decided *against*.
///
/// The monitor reads that state under the `PlayerState` lock but executes only after taking
/// `exec_lock`, so any control op can land in the gap;
/// `engine::state::PlayerState::build_crossfade_actions`
/// re-verifies the whole snapshot under the emit lock and drops the crossfade if anything moved. A
/// struct rather than loose scalars so the two `u64`s can't be swapped at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossfadeDecision {
    /// The real remaining media, per [`should_crossfade`].
    pub fade_ms: u64,
    pub track_id: Option<i64>,
    /// Non-zero — [`should_crossfade`] rejects zero.
    pub position_ms: u64,
}

/// Fade length for a **manual** track change, or `0` for a hard cut.
///
/// - `resuming_at_position`: a restored position should start clean, not fade in from silence.
/// - `deck_busy`: something is actually playing to fade *out* of.
/// - `gapless_pending`: a staged source shares the active deck's fade cell, so a self-ending
///   fade-out armed there would be inherited the moment the current source ends — starting at full
///   volume and audibly fading out. Hard-cut instead. Only reachable from the manual path.
#[must_use]
pub fn manual_fade_ms(
    xf: CrossfadeSettings,
    resuming_at_position: bool,
    deck_busy: bool,
    gapless_pending: bool,
) -> u64 {
    if !xf.enabled || !xf.manual || resuming_at_position || !deck_busy || gapless_pending {
        return 0;
    }
    u64::from(xf.duration_ms)
}

#[cfg(test)]
#[path = "tests/crossfade_tests.rs"]
mod tests;
