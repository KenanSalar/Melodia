//! The crossfade's ramp cell the audio thread reads, the settings cell the control layer writes,
//! and the two pure predicates deciding when a transition becomes a crossfade.
//! The ramp lives in [`EqSource`], counting media samples after the clamp, so it lands on the
//! track's end; the curve is complementary linear, so two decks sum inside ±1 unclamped.
//!
//! [`EqSource`]: super::equalizer::EqSource

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use super::dsp::{AtomicF32, Generation};
use melodia_core::entities::track::TrackSummary;

/// Bounded by the monitor's poll: a narrower window can be stepped over, and with the gapless
/// preload already suppressed the miss is a gapped cut. `handlers.rs` pins it with a `const` assert.
pub const MIN_CROSSFADE_MS: u32 = 1_000;
pub const MAX_CROSSFADE_MS: u32 = 12_000;
pub const DEFAULT_CROSSFADE_MS: u32 = 2_000;

/// Below this much remaining media the track drains instead: a fade clamped up past the real audio
/// would cut the outgoing track at a non-zero gain.
pub const MIN_FADE_MS: u64 = 250;

/// The surviving deck's climb back to unity on an abort, heard only when the aborting seek bails;
/// a seek that lands rebuilds the source at unity.
pub const ABORT_RAMP_MS: u64 = 40;

/// Fade length for pause / resume / user-initiated stop when [`CrossfadeSettings::fade_on_pause`]
/// is on.
pub const PAUSE_FADE_MS: u64 = 250;

/// A target gain this close to unity lets the source disengage the fade stage entirely and fall
/// back to its bit-identical bypass path.
const UNITY_EPSILON: f32 = 1e-4;

#[must_use]
pub fn clamp_crossfade_ms(ms: u32) -> u32 {
    ms.clamp(MIN_CROSSFADE_MS, MAX_CROSSFADE_MS)
}

/// The settings slider's seconds, clamped in float space so the cast lands in range.
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

/// Requires an album, or two `None`s comparing equal would never crossfade an untagged library.
#[must_use]
pub fn same_album(a: &TrackSummary, b: &TrackSummary) -> bool {
    a.album.is_some() && a.album == b.album && a.artist == b.artist
}

/// Raw `u8`s, the cell being read on the audio thread through an [`AtomicU8`].
const KIND_IDLE: u8 = 0;
const KIND_RAMP: u8 = 1;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FadeCmd {
    /// `None` ramps from the source's current gain.
    pub start: Option<f32>,
    pub target: f32,
    /// Media milliseconds, which only the source can convert.
    pub ramp_ms: u64,
    /// The source ends once the ramp lands, draining its deck, which is how `is_crossfading` sees
    /// the overlap finish.
    pub end_on_complete: bool,
}

/// Deck-scoped rather than source-scoped: two exist, each cloned into every [`EqSource`] on its
/// deck, so a fade armed there applies to a gapless successor too.
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
    /// No gain, so the source keeps its bit-identical bypass.
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

    fn bump(&self) {
        self.generation.bump();
    }

    /// Replaces any ramp already in flight on this deck.
    pub fn arm(&self, start: Option<f32>, target: f32, ramp_ms: u64, end_on_complete: bool) {
        self.start.store(start.unwrap_or(f32::NAN));
        self.target.store(target);
        self.ramp_ms.store(ramp_ms, Ordering::Relaxed);
        self.end_on_complete.store(end_on_complete, Ordering::Relaxed);
        self.kind.store(KIND_RAMP, Ordering::Relaxed);
        self.bump();
    }

    /// Back to transparency, the deck's sources taking the bypass from the next sample.
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

/// Read by the control layer, never the audio thread, so no generation counter.
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

/// Timing-independent, so the monitor gates its late gapless preload on the negation: a fade
/// shorter than the lead would lose to a preload it let through. `drains_to_end` is a track that
/// must reach `EndOfStream`: the sleep timer catches it only there, and no fade can cross a reopen.
#[must_use]
pub fn crossfade_eligible(
    xf: CrossfadeSettings,
    drains_to_end: bool,
    has_next: bool,
    same_album: bool,
) -> bool {
    xf.enabled
        && xf.duration_ms > 0
        && has_next
        && !drains_to_end
        && !(xf.skip_same_album && same_album)
}

/// Adds timing and liveness, returning the real remaining media, never clamped up, so the ramp
/// lands on the track end. The window also filters a position gone stale between the monitor's
/// read and its action.
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

/// The decision and the state it was made against, which `build_crossfade_actions` re-verifies
/// under the emit lock; a struct so the two `u64`s can't swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossfadeDecision {
    /// The real remaining media, per [`should_crossfade`].
    pub fade_ms: u64,
    pub track_id: Option<i64>,
    /// Non-zero — [`should_crossfade`] rejects zero.
    pub position_ms: u64,
}

/// `0` for a hard cut: resuming at a position, nothing playing to fade out of, or a staged source
/// sharing the deck's fade cell, which would inherit a self-ending fade-out.
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
