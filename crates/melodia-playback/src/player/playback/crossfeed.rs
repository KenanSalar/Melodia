//! Stereo crossfeed, each ear hearing a little of the other channel as from speakers; not the
//! decks' [`crossfade`](super::crossfade). Only the channels' difference crosses, low-passed
//! (`L - g·LP(L - R)`, `R + g·LP(L - R)`), so centre-panned sound passes untouched and the
//! low-pass's own delay stands in for the far ear's, with no delay line.

use std::mem;
use std::sync::Arc;

use biquad::{Biquad, DirectForm1, Type};

use super::dsp::{self, AtomicF64, FrameRamp};
use melodia_audio::player::source::audio::{ChannelCount, SampleRate};
use melodia_core::entities::crossfeed::CrossfeedSetting;

/// Where the cross path rolls off.
const CUTOFF_HZ: f64 = 700.0;

/// How far below the near ear the far ear hears hard-panned bass, at each end of the strength.
const SUBTLE_FEED_DB: f64 = 9.5;
const STRONG_FEED_DB: f64 = 4.5;

/// The glide to a new amount, so no change of setting clicks.
const CROSSFEED_RAMP_MS: u64 = 50;

/// Ignored by a one-pole design, which only checks it is positive.
const ONE_POLE_Q: f64 = 1.0;

/// Two channels only: a wider source reaches a stereo device as its front pair, half the mix, and
/// a mono one has no image to narrow.
pub fn reaches(channels: ChannelCount) -> bool {
    channels.get() == 2
}

/// `g`, zero when off. The near ear keeps `1 - g` of hard-panned bass and the far ear hears `g`,
/// the feed level apart.
fn amount(setting: CrossfeedSetting) -> f64 {
    if !setting.enabled {
        return 0.0;
    }
    let strength = f64::from(setting.strength.get());
    let feed_db = SUBTLE_FEED_DB + (STRONG_FEED_DB - SUBTLE_FEED_DB) * strength;
    1.0 / (1.0 + 10.0_f64.powf(feed_db / 20.0))
}

/// The amount every source glides toward, one cell so it needs no sequence count.
pub struct CrossfeedShared {
    target: AtomicF64,
}

impl CrossfeedShared {
    /// Off until the engine sets the device heard.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self { target: AtomicF64::new(0.0) })
    }

    pub fn set(&self, setting: CrossfeedSetting) {
        self.target.store(amount(setting));
    }

    fn target(&self) -> f64 {
        self.target.load()
    }
}

/// One source's crossfeed. The output is linear in the amount, so ramping the amount crossfades
/// two settings' outputs while one filter runs on.
pub(crate) struct CrossfeedStage {
    /// `None` where it doesn't reach: a source not in stereo, or a rate too low for the cutoff.
    filter: Option<DirectForm1<f64>>,
    /// This frame's, stepped by a ramp in flight.
    amount: f64,
    target: f64,
    ramp: Option<FrameRamp>,
    ramp_frames: u64,
    was_engaged: bool,
}

impl CrossfeedStage {
    /// Takes the amount set now with no ramp, so a gapless successor doesn't glide in from zero.
    pub(crate) fn new(channels: ChannelCount, rate: SampleRate, shared: &CrossfeedShared) -> Self {
        let sample_rate = f64::from(rate.get());
        let filter = if reaches(channels) {
            dsp::design(Type::SinglePoleLowPass, sample_rate, CUTOFF_HZ, ONE_POLE_Q)
                .map(DirectForm1::new)
        } else {
            None
        };
        let amount = if filter.is_some() { shared.target() } else { 0.0 };
        Self {
            filter,
            amount,
            target: amount,
            ramp: None,
            ramp_frames: CROSSFEED_RAMP_MS * u64::from(rate.get()) / 1000,
            was_engaged: false,
        }
    }

    /// At a frame boundary: whether the stage engaged or let go since the last poll, through a new
    /// target, a ramp's end or a seek.
    pub(crate) fn poll(&mut self, shared: &CrossfeedShared) -> bool {
        if self.filter.is_none() {
            return false;
        }
        if self.ramp.is_some_and(FrameRamp::done) {
            self.land_ramp();
        }
        let target = shared.target();
        #[expect(
            clippy::float_cmp,
            reason = "a change test on a value stored and loaded bit for bit"
        )]
        let moved = target != self.target;
        if moved {
            self.ramp_to(target);
        }
        let engaged = self.engaged();
        engaged != mem::replace(&mut self.was_engaged, engaged)
    }

    pub(crate) fn engaged(&self) -> bool {
        self.amount > 0.0 || self.ramp.is_some()
    }

    /// Steps a ramp in flight, so it is called once a frame.
    #[inline]
    pub(crate) fn run(&mut self, frame: &mut [f64]) {
        if !self.engaged() {
            return;
        }
        let (Some(filter), [left, right]) = (&mut self.filter, frame) else { return };
        if let Some(ramp) = &mut self.ramp {
            self.amount = ramp.step();
        }
        let crossed = self.amount * filter.run(*left - *right);
        *left -= crossed;
        *right += crossed;
    }

    /// A seek lands a ramp at once, the filter's state belonging to the audio left behind.
    pub(crate) fn reset(&mut self) {
        self.reset_state();
        if self.ramp.is_some() {
            self.land_ramp();
        }
    }

    /// Keeps a ramp in flight, so music returning mid-ramp still glides.
    pub(crate) fn reset_state(&mut self) {
        if let Some(filter) = &mut self.filter {
            filter.reset_state();
        }
    }

    /// From wherever the amount sits, so a target arriving mid-ramp glides on from there.
    fn ramp_to(&mut self, target: f64) {
        // Switching on starts from rest: the state is whatever the filter last ran on.
        if !self.engaged()
            && let Some(filter) = &mut self.filter
        {
            filter.reset_state();
        }
        self.ramp = Some(FrameRamp::new(self.amount, target, self.ramp_frames));
        self.target = target;
    }

    fn land_ramp(&mut self) {
        self.ramp = None;
        self.amount = self.target;
    }
}
