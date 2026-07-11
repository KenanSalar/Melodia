//! Audio crossfade: the ramp cell the audio thread reads, the master settings
//! cell the control layer writes, and the two pure predicates that decide when a
//! transition becomes a crossfade.
//!
//! # Why the ramp lives in the source
//!
//! Rodio's `Player` *sequences* sources — `append` schedules a source to start
//! when the previous one ends, so two tracks can never overlap on one `Player`.
//! Overlapping requires two `Player`s on the same [`Mixer`](rodio::mixer::Mixer)
//! (rodio calls each one a "voice"), which is what [`RodioPlayer`] does: two
//! decks, alternating roles.
//!
//! The obvious way to fade a deck is `Player::set_volume` from a ticker. We
//! don't, because rodio wraps every appended source as
//! `EqSource → speed → track_position → pausable → amplify(volume) → …`. Our
//! [`EqSource`] is **innermost**, and a ramp applied there:
//!
//! - is **sample-accurate** (no 5 ms `periodic_access` quantization, no tokio
//!   ticker jitter);
//! - counts **media samples**, the same clock `remaining_ms` uses, so the fade
//!   still lands on the track end at any playback speed (a `set_volume` ramp is
//!   wall-clock and desyncs at speed ≠ 1.0);
//! - leaves `Player::set_volume` as purely the *user's* volume, instead of
//!   racing a ramp against live volume changes;
//! - lands **after** [`EqSource`]'s existing `clamp(-1.0, 1.0)`, which is what
//!   makes the mixer sum safe (below).
//!
//! # Why the curve is linear
//!
//! `Mixer::sum_current_sources` sums its voices with **no clamping**. With a
//! complementary linear curve the two decks' gains satisfy `g_out + g_in ≡ 1`,
//! so post-clamp samples sum to at most 1.0 and the mixer can never clip —
//! preserving the same never-exceed-unity invariant as `MAX_VOLUME`.
//!
//! Equal-power (`sin`/`cos`) would hold perceived loudness across the overlap,
//! but its amplitude sum peaks at √2. The cost of linear is the familiar dip at
//! the midpoint for uncorrelated material — the same trade-off `GStreamer`'s
//! linear volume ramps make in Strawberry and Clementine.
//!
//! [`RodioPlayer`]: super::rodio_backend::RodioPlayer
//! [`EqSource`]: super::equalizer::EqSource

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

use crate::entities::track::TrackSummary;

/// Shortest crossfade the user can select.
///
/// **Bounded from below by the playback monitor's poll interval.**
/// [`should_crossfade`] only fires while `remaining ∈ [MIN_FADE_MS, duration]`,
/// and the monitor samples that window once per poll. If the window were
/// narrower than one poll, `remaining` could step straight over it (e.g.
/// 700 ms → 200 ms) and the crossfade would simply not happen — and because
/// [`crossfade_eligible`] has already suppressed the gapless preload by then,
/// the miss degrades into a gapped hard cut, *worse* than the gapless
/// behaviour the user had before enabling crossfade. `handlers.rs` pins
/// `MIN_CROSSFADE_MS >= MIN_FADE_MS + POLL_INTERVAL_MS` with a `const` assert.
pub const MIN_CROSSFADE_MS: u32 = 1_000;
/// Longest crossfade the user can select.
pub const MAX_CROSSFADE_MS: u32 = 12_000;
/// Default crossfade length. Matches Strawberry's default.
pub const DEFAULT_CROSSFADE_MS: u32 = 2_000;

/// Below this much remaining media, don't start a crossfade at all — let the
/// track drain to `EndOfStream` normally. Keeps the fade from being clamped
/// *up* past the real remaining audio, which would cut the outgoing track at a
/// non-zero gain (an audible click).
pub const MIN_FADE_MS: u64 = 250;

/// How long the surviving deck takes to climb back to unity when a crossfade is
/// aborted (seek, or a new track picked mid-overlap). Long enough to avoid a
/// step discontinuity, short enough to be imperceptible.
pub const ABORT_RAMP_MS: u64 = 40;

/// Fade length for pause / resume / user-initiated stop when
/// [`CrossfadeSettings::fade_on_pause`] is on. Matches Strawberry's default.
pub const PAUSE_FADE_MS: u64 = 250;

/// A target gain this close to unity lets the source disengage the fade stage
/// entirely and fall back to its bit-identical bypass path.
const UNITY_EPSILON: f32 = 1e-4;

/// Clamp a user-supplied crossfade duration into the supported range.
#[must_use]
pub fn clamp_crossfade_ms(ms: u32) -> u32 {
    ms.clamp(MIN_CROSSFADE_MS, MAX_CROSSFADE_MS)
}

/// Seconds (the settings slider's unit) → milliseconds (what the backend and
/// `settings.json` store). The clamp happens in float space, so the narrowing
/// cast always lands inside the supported range.
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

/// Gain at ramp position `pos` of `total`, interpolating `start` → `target`.
///
/// A `total` of zero (or a `pos` past the end) lands on `target`. Linear, so
/// two complementary ramps (`1 → 0` and `0 → 1`) always sum to exactly 1.0.
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

/// Two tracks belong to the same album. Requires the album tag to be *present*:
/// comparing two `None` albums as equal would mark every transition in an
/// untagged library as same-album, so it would never crossfade.
#[must_use]
pub fn same_album(a: &TrackSummary, b: &TrackSummary) -> bool {
    a.album.is_some() && a.album == b.album && a.artist == b.artist
}

// ---------------------------------------------------------------------------
// FadeShared — the per-deck ramp cell
// ---------------------------------------------------------------------------

/// `kind` discriminants. Kept as raw `u8` because the cell is read on the audio
/// thread through an [`AtomicU8`].
const KIND_IDLE: u8 = 0;
const KIND_RAMP: u8 = 1;

/// A ramp command, read by the source when the generation advances.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FadeCmd {
    /// `None` means "ramp from whatever gain the source is currently at" — used
    /// to fade out a track already playing at unity, and to recover a partially
    /// faded-in track when a crossfade is aborted.
    pub start: Option<f32>,
    pub target: f32,
    /// Ramp length in **media** milliseconds. The source converts this with its
    /// own sample rate and channel count; the controller can't, because the two
    /// decks may hold tracks at different sample rates.
    pub ramp_ms: u64,
    /// Fade-out only: once the ramp lands, the source returns `None` and ends.
    /// That drains its deck, which is how [`RodioPlayer::is_crossfading`]
    /// detects the overlap finishing.
    ///
    /// [`RodioPlayer::is_crossfading`]: super::rodio_backend::RodioPlayer::is_crossfading
    pub end_on_complete: bool,
}

/// Lock-free ramp state shared between the control layer (writer) and the audio
/// thread (reader), mirroring [`EqShared`](super::equalizer::EqShared)'s
/// generation-poll pattern.
///
/// The cell is **deck-scoped**, not source-scoped: exactly two exist for the
/// app's lifetime, and each is cloned into every [`EqSource`] appended to its
/// deck. A fade armed on a deck therefore applies to whatever source that deck
/// is currently playing, including a gapless-appended one.
///
/// [`EqSource`]: super::equalizer::EqSource
pub struct FadeShared {
    generation: AtomicU64,
    kind: AtomicU8,
    /// `f32` bit pattern, or `f32::NAN` for [`FadeCmd::start`] = `None`.
    start_bits: AtomicU32,
    target_bits: AtomicU32,
    ramp_ms: AtomicU64,
    end_on_complete: AtomicBool,
}

impl FadeShared {
    /// A cell in the idle state — the source applies no gain and keeps its
    /// bit-identical bypass path.
    #[must_use]
    pub fn idle() -> Arc<Self> {
        Arc::new(Self {
            generation: AtomicU64::new(1),
            kind: AtomicU8::new(KIND_IDLE),
            start_bits: AtomicU32::new(f32::NAN.to_bits()),
            target_bits: AtomicU32::new(1.0_f32.to_bits()),
            ramp_ms: AtomicU64::new(0),
            end_on_complete: AtomicBool::new(false),
        })
    }

    /// Publish a state change. `Release` pairs with the reader's `Acquire` load
    /// of `generation`, so the field writes above it are visible once the
    /// reader observes the new generation.
    fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Arm a ramp. Replaces any ramp already in flight on this deck.
    pub fn arm(&self, start: Option<f32>, target: f32, ramp_ms: u64, end_on_complete: bool) {
        self.start_bits
            .store(start.unwrap_or(f32::NAN).to_bits(), Ordering::Relaxed);
        self.target_bits.store(target.to_bits(), Ordering::Relaxed);
        self.ramp_ms.store(ramp_ms, Ordering::Relaxed);
        self.end_on_complete.store(end_on_complete, Ordering::Relaxed);
        self.kind.store(KIND_RAMP, Ordering::Relaxed);
        self.bump();
    }

    /// Return the deck to transparency. Any source on it drops back to its
    /// bypass fast path on the next sample.
    pub fn reset(&self) {
        self.kind.store(KIND_IDLE, Ordering::Relaxed);
        self.bump();
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Read the current command. `None` when the cell is idle.
    #[must_use]
    pub fn snapshot(&self) -> Option<FadeCmd> {
        if self.kind.load(Ordering::Relaxed) != KIND_RAMP {
            return None;
        }
        let start = f32::from_bits(self.start_bits.load(Ordering::Relaxed));
        Some(FadeCmd {
            start: if start.is_nan() { None } else { Some(start) },
            target: f32::from_bits(self.target_bits.load(Ordering::Relaxed)),
            ramp_ms: self.ramp_ms.load(Ordering::Relaxed),
            end_on_complete: self.end_on_complete.load(Ordering::Relaxed),
        })
    }
}

// ---------------------------------------------------------------------------
// CrossfadeShared — the master settings cell
// ---------------------------------------------------------------------------

/// A `Copy` snapshot of the crossfade settings, taken once per decision so the
/// four flags can't shear against each other mid-evaluation.
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

/// Lock-free crossfade settings. Unlike [`FadeShared`] this is read by the
/// *control* layer (the playback monitor and the backend), never by the audio
/// thread, so it needs no generation counter.
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

// ---------------------------------------------------------------------------
// The two decision predicates
// ---------------------------------------------------------------------------

/// **Timing-independent**: is this transition a crossfade transition at all?
///
/// The playback monitor gates its late gapless preload on `!crossfade_eligible`,
/// which is why this must not depend on the current position. If the preload
/// were gated on [`should_crossfade`] instead, then for any crossfade shorter
/// than `PRELOAD_LEAD_MS` the preload would fire first (while `should_crossfade`
/// is still `None`), set `gapless_pending`, and permanently block the crossfade
/// via its own `!gapless_pending` gate — or, worse, stage the next track twice.
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
        // Sleep-timer "pause at end of track" needs the track to drain to
        // `EndOfStream`, the only boundary that gate can catch.
        && !pause_at_end
        && !(xf.skip_same_album && same_album)
}

/// Adds the timing and liveness terms to [`crossfade_eligible`]. Returns the
/// fade length in **media** milliseconds, or `None` to leave this tick alone.
///
/// The returned length is the *actual* remaining media, never clamped up to the
/// configured duration, so the ramp lands exactly on the declared track end.
/// That self-corrects for the monitor's poll granularity: the trigger fires on
/// the first tick inside the window, and the fade shortens to match.
///
/// The trigger window is `[MIN_FADE_MS, duration_cap_ms]`, so it is only ever
/// sampled once per poll. It must therefore stay **at least one poll wide** or a
/// tick can step over it and no crossfade fires at all — see the note on
/// [`MIN_CROSSFADE_MS`], which is what guarantees that.
///
/// The `remaining ∈ [MIN_FADE_MS, cap]` window also rejects a stale position
/// read. rodio only zeroes a `Player`'s tracked position when it actually
/// clears a source, so an already-drained deck can report its *previous*
/// track's position for a few milliseconds: too high saturates `remaining` to
/// zero, too low pushes it past the cap. Both fall outside the window.
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

/// Fade length for a **manual** track change (next / previous / picking a
/// track), or `0` for a hard cut.
///
/// - `resuming_at_position`: a restored playback position should start clean,
///   not fade in from silence.
/// - `deck_busy`: something is actually playing to fade *out* of.
/// - `gapless_pending`: a gapless source is staged behind the current one on
///   the active deck, sharing its fade cell. A self-ending fade-out armed there
///   would be inherited by that staged source the moment the current one ends —
///   it would start at full volume and audibly fade out. Hard-cut instead. The
///   preload only stages in the last `PRELOAD_LEAD_MS` of a track, so the
///   window is tiny. The *automatic* path can't reach this state: the preload
///   is suppressed for crossfade transitions, and [`should_crossfade`] refuses
///   to fire while one is staged.
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
