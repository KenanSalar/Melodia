//! Which of its own rates a claim runs a device at when the device lacks the source's.
//!
//! The voices convert to whatever the device runs, so the pick is about what the conversion costs
//! the sound: a rate of the source's own family keeps the ratio simple, and one above the source
//! keeps its whole band.

use std::cmp::Reverse;

use melodia_audio::player::source::audio::SampleRate;

/// Every rate a device is asked about, ascending.
pub(super) const LADDER: [u32; 15] = [
    8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000,
    352_800, 384_000, 705_600, 768_000,
];

/// What every rate of the 44.1 kHz family, and every rate of the 48 kHz family, is a multiple of.
const FAMILY_BASES: [u32; 2] = [11_025, 8_000];

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
