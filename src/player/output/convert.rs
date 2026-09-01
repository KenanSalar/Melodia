//! Bringing one source's samples to the device's rate and channel count.
//!
//! Pull-driven, because the whole chain above it is: the voice asks for a block and this reaches
//! back through the source for however many frames that takes. Linear interpolation, which is what
//! rodio did and what the one reference player that owns its output ships; a better kernel is a
//! change to [`Converter::fill`] and nothing else.
//!
//! **Playback speed lives here.** rodio expressed it by reporting a multiplied `sample_rate()`
//! upward and letting its mixer resample the difference, which is why a position had to be read on
//! one timeline and reported on another. Folding it into the ratio instead leaves
//! [`AudioSource::sample_rate`] meaning the source's own rate at every level, so frames pulled are
//! media frames and the clock needs no conversion.

use super::super::audio::{AudioSource, Sample, Shape};

/// What one [`Converter::fill`] did.
///
/// A struct rather than a pair: both halves are counts, they are never equal once a rate or a
/// channel count differs, and swapping them would run the clock at the device's rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filled {
    /// Samples written to the output block, interleaved at the device's channel count.
    pub samples: usize,
    /// Frames taken from the source, on its own timeline. This is what the clock counts.
    pub source_frames: u64,
}

pub struct Converter {
    source: Shape,
    device: Shape,
    /// The two source frames the output is interpolated between, and where between them the next
    /// output frame falls. Held across calls, so a block boundary is not a source boundary and the
    /// interpolation window never restarts mid-track.
    prev: Box<[Sample]>,
    next: Box<[Sample]>,
    position: f64,
    primed: bool,
    /// The source ran out. `prev` still holds its final frame until that has been handed over.
    drained: bool,
    done: bool,
}

impl Converter {
    pub fn new(source: Shape, device: Shape) -> Self {
        let width = usize::from(source.channels.get());
        Self {
            source,
            device,
            prev: vec![0.0; width].into_boxed_slice(),
            next: vec![0.0; width].into_boxed_slice(),
            position: 0.0,
            primed: false,
            drained: false,
            done: false,
        }
    }

    /// Write up to `out.len()` samples, pulling from `src` as the ratio demands.
    ///
    /// A short return means the source ended; `out` past that point is untouched. `speed` scales
    /// the source's rate rather than the device's, so `1.0` against equal rates steps exactly one
    /// source frame per output frame and every sample passes through untouched.
    pub fn fill(&mut self, out: &mut [Sample], src: &mut dyn AudioSource, speed: f64) -> Filled {
        let width = usize::from(self.device.channels.get());
        let step = f64::from(self.source.rate.get()) * speed / f64::from(self.device.rate.get());

        let mut filled = Filled {
            samples: 0,
            source_frames: 0,
        };
        if self.done {
            return filled;
        }

        for frame in out.chunks_exact_mut(width) {
            if !self.primed {
                filled.source_frames += self.prime(src);
                if self.done {
                    break;
                }
            }

            // Exactly `prev` while `position` is zero, which is the whole of the equal-rate case:
            // the bit-identical passthrough falls out of the general path rather than needing one
            // of its own.
            self.write_frame(frame);
            filled.samples += width;

            self.position += step;
            while self.position >= 1.0 {
                self.position -= 1.0;
                filled.source_frames += self.advance(src);
                if self.done {
                    // The final frame has been written; anything past it is the next source's.
                    return filled;
                }
            }
        }
        filled
    }

    /// Whether the source behind this converter has been read to its end.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Take the first two frames. A source with none at all is already over.
    fn prime(&mut self, src: &mut dyn AudioSource) -> u64 {
        let mut taken = 0;
        if pull_frame(src, &mut self.prev) {
            taken += 1;
        } else {
            self.done = true;
            return taken;
        }
        if pull_frame(src, &mut self.next) {
            taken += 1;
        } else {
            // A one-frame source: hold it so the single output frame is that frame rather than an
            // interpolation toward silence.
            self.next.copy_from_slice(&self.prev);
            self.drained = true;
        }
        self.primed = true;
        taken
    }

    /// Step to the next pair of source frames.
    fn advance(&mut self, src: &mut dyn AudioSource) -> u64 {
        if self.drained {
            self.done = true;
            return 0;
        }
        self.prev.copy_from_slice(&self.next);
        if pull_frame(src, &mut self.next) {
            1
        } else {
            // `prev` is the source's last frame and has not been handed over yet, so it is held
            // rather than interpolated away: dropping it is a click at a gapless boundary.
            self.next.copy_from_slice(&self.prev);
            self.drained = true;
            0
        }
    }

    /// Interpolate the current source frame and map it onto the device's channels.
    ///
    /// A mono device is the ladder's *second* rung — cpal ranks stereo, then mono, ahead of every
    /// wider count — so its fold is the one that has to be right, and dropping to channel 0 would
    /// play the left half of every stereo file. The mean is what makes that fold safe: [`Shape`]
    /// counts channels without naming them, so what goes into the sum is unknown, and at `1/n` an
    /// unknown channel costs a wide source some level in its mains rather than routing LFE and
    /// surrounds there at full scale. A device wider than mono has no such divisor to hide behind,
    /// so it keeps the first `min(from, to)` channels and folds nothing.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the offset is bounded to [0, 1) by the loop that advances it"
    )]
    fn write_frame(&self, frame: &mut [Sample]) {
        let from = usize::from(self.source.channels.get());
        let position = self.position as Sample;
        let source = |channel: usize| interpolate(self.prev[channel], self.next[channel], position);

        if frame.len() == 1 && from > 1 {
            let sum: Sample = (0..from).map(source).sum();
            frame[0] = sum / Sample::from(self.source.channels.get());
            return;
        }

        for (channel, slot) in frame.iter_mut().enumerate() {
            *slot = match channel {
                c if c < from => source(c),
                // Duplicated at unity, not attenuated: this is the path every mono file on an
                // ordinary stereo device takes, so a pan-law trim here would quietly restage the
                // whole library, and each channel would stop being the source's own sample.
                1 if from == 1 => source(0),
                _ => 0.0,
            };
        }
    }
}

/// Exactly `a` at a zero offset, so equal rates pass samples through bit-identically rather than
/// through a multiply-add that turns `-0.0` into `0.0`.
#[inline]
fn interpolate(a: Sample, b: Sample, t: Sample) -> Sample {
    if t == 0.0 { a } else { a + (b - a) * t }
}

/// Take one whole frame, or nothing. A partial frame is dropped rather than padded: half a frame
/// would flip this voice's channel parity for everything that plays on it after.
fn pull_frame(src: &mut dyn AudioSource, frame: &mut [Sample]) -> bool {
    for slot in frame.iter_mut() {
        match src.next() {
            Some(sample) => *slot = sample,
            None => return false,
        }
    }
    true
}

#[cfg(test)]
#[path = "tests/convert_tests.rs"]
mod tests;
