//! One source's correction, held in two banks so a change of set crossfades them rather than
//! moving coefficients: two sets differ in count and shape, and a path between unrelated
//! coefficients passes through curves neither asked for.

use std::mem;

use biquad::{Biquad, Coefficients, DirectForm1, Type};

use super::response::{FilterResponse, ResponseGrid};
use super::shared::{CorrectionShared, PublishedSet};
use crate::player::playback::dsp::{self, FrameRamp, PASSTHROUGH};
use melodia_audio::player::source::audio::SampleRate;
use melodia_core::entities::correction::{CorrectionFilter, FilterKind, MAX_CORRECTION_FILTERS};

/// The crossfade a swap takes, the incoming bank settling from rest as it fades in.
const CORRECTION_SWAP_MS: u64 = 50;

type Filters = [DirectForm1<f64>; MAX_CORRECTION_FILTERS];

pub(crate) struct CorrectionStage {
    current: Bank,
    outgoing: Bank,
    channels: Vec<ChannelFilters>,
    ramp: Option<FrameRamp>,
    swap_frames: u64,
    /// `None` until the source's first read, which takes its set with no ramp.
    last_sequence: Option<u64>,
    /// A change the owner rebuilds its headroom and bypass for, taken by the next poll.
    reshaped: bool,
    grid: ResponseGrid,
    sample_rate: f64,
}

impl CorrectionStage {
    pub(crate) fn new(channels: usize, rate: SampleRate) -> Self {
        let sample_rate = f64::from(rate.get());
        Self {
            current: Bank::EMPTY,
            outgoing: Bank::EMPTY,
            channels: (0..channels).map(|_| ChannelFilters::at_rest()).collect(),
            ramp: None,
            swap_frames: CORRECTION_SWAP_MS * u64::from(rate.get()) / 1000,
            last_sequence: None,
            reshaped: false,
            grid: ResponseGrid::new(sample_rate),
            sample_rate,
        }
    }

    /// At a frame boundary: whether the stage changed shape since the last poll, through a swap, a
    /// ramp's end or a seek.
    pub(crate) fn poll(&mut self, shared: &CorrectionShared) -> bool {
        match self.ramp {
            // A swap arriving mid-ramp waits for this one to land.
            Some(ramp) if !ramp.done() => return false,
            Some(_) => self.end_ramp(),
            None => {}
        }
        if let Some((sequence, set)) = shared.read_if_changed(self.last_sequence) {
            let first_read = self.last_sequence.is_none();
            if first_read {
                self.design_current(&set);
            } else {
                self.swap_to(&set);
            }
            self.last_sequence = Some(sequence);
            self.reshaped = true;
        }
        mem::take(&mut self.reshaped)
    }

    pub(crate) fn update_headroom(&mut self, bands: &[FilterResponse]) {
        self.current.update_headroom(&self.grid, bands);
        self.outgoing.update_headroom(&self.grid, bands);
    }

    /// A band's response on this source's grid, for the headroom to count.
    pub(crate) fn response(&self, coeffs: &Coefficients<f64>, centre_hz: f64) -> FilterResponse {
        self.grid.response(coeffs, centre_hz)
    }

    pub(crate) fn engaged(&self) -> bool {
        self.current.active > 0 || self.ramp.is_some()
    }

    /// Steps a ramp in flight, so it is called once a frame.
    pub(crate) fn frame_mix(&mut self) -> Mix {
        let Some(ramp) = &mut self.ramp else {
            return Mix { current: self.current.weighted(1.0), outgoing: None };
        };
        // Linear: both banks carry one signal, so an equal-power curve would bump the level.
        let incoming = ramp.step();
        Mix {
            current: self.current.weighted(incoming),
            outgoing: Some(self.outgoing.weighted(1.0 - incoming)),
        }
    }

    pub(crate) fn channels_mut(&mut self) -> std::slice::IterMut<'_, ChannelFilters> {
        self.channels.iter_mut()
    }

    /// A seek lands a ramp at once, the outgoing bank's state belonging to the audio left behind.
    pub(crate) fn reset(&mut self) {
        self.reset_state();
        if self.ramp.is_some() {
            self.end_ramp();
        }
    }

    /// Keeps a ramp in flight, so music returning mid-swap still crossfades.
    pub(crate) fn reset_state(&mut self) {
        for channel in &mut self.channels {
            for filter in channel.current.iter_mut().chain(&mut channel.outgoing) {
                filter.reset_state();
            }
        }
    }

    fn swap_to(&mut self, set: &PublishedSet) {
        mem::swap(&mut self.current, &mut self.outgoing);
        for channel in &mut self.channels {
            mem::swap(&mut channel.current, &mut channel.outgoing);
        }
        self.design_current(set);
        // Two idle banks have nothing to fade, and a ramp would hold the bypass off.
        if self.current.active > 0 || self.outgoing.active > 0 {
            self.ramp = Some(FrameRamp::new(0.0, 1.0, self.swap_frames));
        }
    }

    /// Every filter starts from rest, so the outgoing curve's state can't ring into the new one.
    fn design_current(&mut self, set: &PublishedSet) {
        let designs = set
            .filters()
            .iter()
            .filter_map(|filter| Some((coefficients(filter, self.sample_rate)?, filter.freq_hz)));
        let mut active = 0;
        for (coeffs, centre_hz) in designs {
            self.current.responses[active] = self.grid.response(&coeffs, centre_hz);
            for channel in &mut self.channels {
                channel.current[active] = DirectForm1::new(coeffs);
            }
            active += 1;
        }
        self.current.active = active;
    }

    fn end_ramp(&mut self) {
        self.ramp = None;
        self.outgoing.active = 0;
        self.reshaped = true;
    }
}

/// What every channel's filters share: their responses and their level.
struct Bank {
    responses: [FilterResponse; MAX_CORRECTION_FILTERS],
    /// A filter at or past Nyquist sits out, so this counts only those that run.
    active: usize,
    headroom: f64,
}

impl Bank {
    const EMPTY: Self = Self {
        responses: [FilterResponse::FLAT; MAX_CORRECTION_FILTERS],
        active: 0,
        headroom: 1.0,
    };

    fn weighted(&self, share: f64) -> Weighted {
        Weighted { gain: share * self.headroom, filters: self.active }
    }

    /// Nothing is added with nothing running, so a path with the bands alone keeps its level.
    fn update_headroom(&mut self, grid: &ResponseGrid, bands: &[FilterResponse]) {
        self.headroom = if self.active == 0 {
            1.0
        } else {
            grid.headroom(&self.responses[..self.active], bands)
        };
    }
}

pub(crate) struct ChannelFilters {
    current: Filters,
    outgoing: Filters,
}

impl ChannelFilters {
    fn at_rest() -> Self {
        let rest: Filters = [DirectForm1::new(PASSTHROUGH); MAX_CORRECTION_FILTERS];
        Self { current: rest, outgoing: rest }
    }

    /// With nothing engaged the input is scaled by one, which passes it bit for bit.
    #[inline]
    pub(crate) fn run(&mut self, input: f64, mix: Mix) -> f64 {
        let current = cascade(&mut self.current, mix.current, input);
        match mix.outgoing {
            Some(outgoing) => current + cascade(&mut self.outgoing, outgoing, input),
            None => current,
        }
    }
}

#[inline]
fn cascade(filters: &mut Filters, weighted: Weighted, input: f64) -> f64 {
    let output =
        filters.iter_mut().take(weighted.filters).fold(input, |sample, filter| filter.run(sample));
    output * weighted.gain
}

/// One frame's share of each bank, each scaled by its bank's headroom.
#[derive(Clone, Copy)]
pub(crate) struct Mix {
    current: Weighted,
    /// Only while a swap fades the outgoing bank out.
    outgoing: Option<Weighted>,
}

#[derive(Clone, Copy)]
struct Weighted {
    gain: f64,
    filters: usize,
}

fn coefficients(filter: &CorrectionFilter, sample_rate: f64) -> Option<Coefficients<f64>> {
    let shape = match filter.kind {
        FilterKind::Peaking => Type::PeakingEQ(filter.gain_db),
        FilterKind::LowShelf => Type::LowShelf(filter.gain_db),
        FilterKind::HighShelf => Type::HighShelf(filter.gain_db),
    };
    dsp::design(shape, sample_rate, filter.freq_hz, filter.q)
}

#[cfg(test)]
#[path = "tests/bank_tests.rs"]
mod tests;
