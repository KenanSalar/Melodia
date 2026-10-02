//! Which of its own rates a claim runs a device at when the device lacks the source's.
//!
//! The voices convert to whatever the device runs, so the pick is about what the conversion costs
//! the sound: a rate of the source's own family keeps the ratio simple, and one above the source
//! keeps its whole band.
//!
//! A claim keeps the rates it was offered as a [`RateSet`], so a later track can ask where a fresh
//! claim would put the device, and play on the open one wherever that is where it runs now.

use std::cmp::Reverse;
use std::fmt;

use melodia_audio::player::source::audio::SampleRate;

use super::RateFallback;

/// Every rate a device is asked about, ascending.
pub(super) const LADDER: [u32; 15] = [
    8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
    352_800, 384_000, 705_600, 768_000,
];

const _: () = assert!(LADDER.len() <= RateSet::CAPACITY, "every rung needs a bit in a RateSet");

/// What every rate of the 44.1 kHz family, and every rate of the 48 kHz family, is a multiple of.
const FAMILY_BASES: [u32; 2] = [11_025, 8_000];

/// The rungs of [`LADDER`] a device offered, one bit each, so the answer travels with a claim as a
/// plain copy.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct RateSet(u16);

impl RateSet {
    const CAPACITY: usize = u16::BITS as usize;

    /// Whether the device offered `rate`, or `None` for a rate off the ladder, which the set has no
    /// bit to say either way for.
    pub(super) fn contains(self, rate: u32) -> Option<bool> {
        rung(rate).map(|bit| self.0 & bit != 0)
    }

    pub(super) fn rates(self) -> impl Iterator<Item = u32> {
        LADDER.into_iter().filter(move |&rate| self.contains(rate) == Some(true))
    }
}

impl FromIterator<u32> for RateSet {
    fn from_iter<I: IntoIterator<Item = u32>>(rates: I) -> Self {
        Self(rates.into_iter().filter_map(rung).fold(0, |bits, bit| bits | bit))
    }
}

impl fmt::Debug for RateSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.rates()).finish()
    }
}

/// `rate`'s bit in a [`RateSet`], or `None` for a rate off the ladder.
fn rung(rate: u32) -> Option<u16> {
    LADDER.iter().position(|&rung| rung == rate).map(|index| 1 << index)
}

/// The rate a fresh claim runs a device offering `offered` at for a `source` rate: the source's
/// own where it is offered, else, where `fallback` lets the claim convert, the one
/// [`device_rate_for`] picks, else none.
///
/// Also none for a source off the ladder, a 24 kHz MP3 say: the set has no bit for it, so it can't
/// say whether a fresh claim would run the device at that rate itself.
pub(super) fn claim_rate(
    source: SampleRate,
    offered: RateSet,
    fallback: RateFallback,
) -> Option<SampleRate> {
    if offered.contains(source.get())? {
        return Some(source);
    }
    match fallback {
        RateFallback::Shared => None,
        RateFallback::Resample => device_rate_for(source, &offered.rates().collect::<Vec<_>>()),
    }
}

/// The rate to run a device at for a `source` rate it lacks, out of the `supported` rungs of
/// [`LADDER`] it offers, or `None` where it offers none.
///
/// The source's family first: the lowest multiple above it, then the highest rate below it. Past
/// those, the nearest of the rest, taking the higher of two equally near, which gives up less of
/// the band.
pub(super) fn device_rate_for(source: SampleRate, supported: &[u32]) -> Option<SampleRate> {
    let hz = source.get();
    let rates = || supported.iter().copied();
    let multiple = rates().filter(|&rate| rate > hz && rate.is_multiple_of(hz)).min();
    let below = || rates().filter(|&rate| rate < hz && same_family(rate, hz)).max();
    let nearest = || rates().min_by_key(|&rate| (rate.abs_diff(hz), Reverse(rate)));
    multiple.or_else(below).or_else(nearest).and_then(SampleRate::new)
}

fn same_family(a: u32, b: u32) -> bool {
    FAMILY_BASES.iter().any(|&base| a.is_multiple_of(base) && b.is_multiple_of(base))
}

#[cfg(test)]
#[path = "tests/rates_tests.rs"]
mod tests;
