//! Bringing one source's samples to the device's rate and channel count.
//!
//! Pull-driven, because the whole chain above it is: the voice asks for a block and this reaches
//! back through the source for however many frames that takes. Where the rates differ, or the
//! speed isn't one, each output frame is interpolated by [`super::resample`]'s band-limited kernel.
//! Where they match exactly, the source's own frames are handed over untouched, which is what
//! bit-perfect output rests on.
//!
//! **Playback speed lives here.** rodio expressed it by reporting a multiplied `sample_rate()`
//! upward and letting its mixer resample the difference, which is why a position had to be read on
//! one timeline and reported on another. Folding it into the ratio instead leaves
//! [`AudioSource::sample_rate`] meaning the source's own rate at every level, so frames pulled are
//! media frames and the clock needs no conversion.

use melodia_audio::player::source::audio::{AudioSource, Sample, Shape};

use super::resample::{self, CENTRE, Kernel, SPAN, Weighed};

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

/// The device is not held here but handed to every [`Self::fill`], because nothing below depends
/// on it between calls: the buffers are sized by the source and the interpolation state lives on
/// the source's timeline. That is what lets an output reopen at another shape under a source that
/// is part way through, with no rebuild and nothing built on the control thread to go stale.
///
/// **Running out is not the end.** A source that runs dry leaves the converter [starved], still
/// owed frames, and whoever holds it decides what follows: a successor of the same shape takes
/// this converter over and pays them, so a gapless seam is converted as one stream, or
/// [`Self::drain`] ends it where nothing follows.
///
/// [starved]: Self::is_starved
pub struct Converter {
    source: Shape,
    kernel: &'static Kernel,
    /// The frames around the one being written. Held across calls and, at a gapless seam, across
    /// sources, so neither a block boundary nor a track boundary restarts the kernel.
    window: Window,
    /// How far past the centre frame the next output frame falls.
    position: f64,
    /// Frames to take in before the next output frame: one for every whole step the position
    /// crosses that the lookahead doesn't already cover.
    owed: u32,
    /// Frames taken in past the centre. None while the output copies the source's own frames, so
    /// a bit-perfect path runs no later than the source; the kernel's reach once it interpolates.
    ahead: usize,
    /// Silent frames pushed since the source ran out. The centre reaching one ends the converter.
    pads: usize,
    state: State,
    incoming: Box<[Sample]>,
    weights: Box<[f32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Pulling,
    /// The last pull came back empty with frames still owed.
    Starved,
    /// Nothing follows the source; what the window holds of it plays out against silence.
    Draining,
    Done,
}

impl Converter {
    pub fn new(source: Shape) -> Self {
        let width = usize::from(source.channels.get());
        Self {
            source,
            kernel: resample::kernel(),
            window: Window::new(width),
            position: 0.0,
            owed: 1,
            ahead: 0,
            pads: 0,
            state: State::Pulling,
            incoming: vec![0.0; width].into_boxed_slice(),
            weights: vec![0.0; SPAN].into_boxed_slice(),
        }
    }

    /// Write up to `out.len()` samples at `device`'s shape, pulling from `src` as the ratio demands.
    ///
    /// A short return means the converter starved or finished; `out` past that point is untouched.
    /// `speed` scales the source's rate rather than the device's, so `1.0` against equal rates
    /// steps exactly one source frame per output frame and every sample passes through untouched.
    pub fn fill(
        &mut self,
        out: &mut [Sample],
        src: &mut dyn AudioSource,
        device: Shape,
        speed: f64,
    ) -> Filled {
        let width = usize::from(device.channels.get());
        let step = f64::from(self.source.rate.get()) * speed / f64::from(device.rate.get());
        let passthrough = step.to_bits() == 1.0_f64.to_bits();
        let reach = if passthrough {
            // Only a position of zero hands the source's own frames over, so one an earlier speed
            // left part way between two is dropped once the step is back to exactly one.
            self.position = 0.0;
            0
        } else {
            resample::reach(step)
        };

        let mut filled = Filled { samples: 0, source_frames: 0 };
        if !self.settle(src, reach, &mut filled.source_frames) {
            return filled;
        }
        for frame in out.chunks_exact_mut(width) {
            let weighed =
                (!passthrough).then(|| self.kernel.weigh(self.position, step, &mut self.weights));
            self.write_frame(frame, weighed.as_ref());
            filled.samples += width;

            self.position += step;
            while self.position >= 1.0 {
                self.position -= 1.0;
                self.advance();
            }
            // Settled straight away rather than before the next write, so a source that has just
            // handed over its last frame is seen to be over in the fill that wrote it.
            if !self.settle(src, reach, &mut filled.source_frames) {
                break;
            }
        }
        filled
    }

    /// Whether the last fill ran the source dry with frames still owed.
    pub fn is_starved(&self) -> bool {
        self.state == State::Starved
    }

    /// Whether everything the source handed over has been written.
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    /// Nothing follows the source this converter starved on: what the window still holds of it
    /// plays out against silence, and the converter is done once the centre reaches that.
    ///
    /// The silence the position has already crossed into is taken here rather than at the next
    /// fill, so a source whose last frame has been written is done as soon as it is drained.
    pub fn drain(&mut self) {
        if self.state == State::Done {
            return;
        }
        self.state = State::Draining;
        while self.owed > 0 && self.pads <= self.ahead {
            self.window.push_silence();
            self.pads += 1;
            self.owed -= 1;
        }
        if self.pads > self.ahead {
            self.state = State::Done;
        }
    }

    /// Source frames taken in past the one being written, which the ear is behind the clock by.
    pub fn frames_ahead(&self) -> u64 {
        self.ahead.saturating_sub(self.pads) as u64
    }

    /// Move the centre on a frame, into the lookahead where there is some.
    fn advance(&mut self) {
        if self.ahead > 0 {
            self.ahead -= 1;
        } else {
            self.owed += 1;
        }
    }

    /// Take in what the next output frame needs: the frames the position has crossed, then
    /// lookahead up to `reach`. False where the window can't be brought there.
    ///
    /// A lookahead pushed while the output is copying stays and drains away as the centre moves
    /// through it, rather than being thrown out: the output never skips a frame, and a bit-perfect
    /// path is back to running no later than the source once it has.
    fn settle(&mut self, src: &mut dyn AudioSource, reach: usize, taken: &mut u64) -> bool {
        loop {
            if self.pads > self.ahead {
                self.state = State::Done;
            }
            if self.state == State::Done {
                return false;
            }
            let owed = self.owed > 0;
            if !owed && self.ahead >= reach {
                return true;
            }
            if !self.push(src, taken) {
                return false;
            }
            if owed {
                self.owed -= 1;
            } else {
                self.ahead += 1;
            }
        }
    }

    /// Take the source's next frame into the window, or a silent one once it has run out.
    fn push(&mut self, src: &mut dyn AudioSource, taken: &mut u64) -> bool {
        match self.state {
            State::Pulling | State::Starved => {
                if !pull_frame(src, &mut self.incoming) {
                    self.state = State::Starved;
                    return false;
                }
                self.state = State::Pulling;
                self.window.push(&self.incoming);
                *taken += 1;
            }
            State::Draining => {
                self.window.push_silence();
                self.pads += 1;
            }
            State::Done => return false,
        }
        true
    }

    /// Write the centre frame, or the kernel's interpolation around it where it has been
    /// `weighed`, and map it onto the device's channels.
    ///
    /// A mono device is the ladder's *second* rung — cpal ranks stereo, then mono, ahead of every
    /// wider count — so its fold is the one that has to be right, and dropping to channel 0 would
    /// play the left half of every stereo file. The mean is what makes that fold safe: [`Shape`]
    /// counts channels without naming them, so what goes into the sum is unknown, and at `1/n` an
    /// unknown channel costs a wide source some level in its mains rather than routing LFE and
    /// surrounds there at full scale. A device wider than mono has no such divisor to hide behind,
    /// so it keeps the first `min(from, to)` channels and folds nothing.
    fn write_frame(&self, frame: &mut [Sample], weighed: Option<&Weighed>) {
        let from = usize::from(self.source.channels.get());
        let source = |channel: usize| match weighed {
            Some(weighed) => self.window.interpolate(channel, self.ahead, weighed, &self.weights),
            None => self.window.behind_newest(channel, self.ahead),
        };

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

/// The last [`SPAN`] frames, one ring per channel. Each frame is written twice, a ring's length
/// apart, so any run of frames the kernel reads is one slice.
struct Window {
    samples: Box<[Sample]>,
    /// Where the next frame goes, which is also the oldest.
    head: usize,
}

impl Window {
    fn new(channels: usize) -> Self {
        Self { samples: vec![0.0; channels * 2 * SPAN].into_boxed_slice(), head: 0 }
    }

    fn push(&mut self, frame: &[Sample]) {
        let head = self.head;
        for (ring, &sample) in self.samples.chunks_exact_mut(2 * SPAN).zip(frame) {
            ring[head] = sample;
            ring[head + SPAN] = sample;
        }
        self.head = (head + 1) % SPAN;
    }

    fn push_silence(&mut self) {
        let head = self.head;
        for ring in self.samples.chunks_exact_mut(2 * SPAN) {
            ring[head] = 0.0;
            ring[head + SPAN] = 0.0;
        }
        self.head = (head + 1) % SPAN;
    }

    /// `channel`'s frame `ahead` frames before the newest.
    fn behind_newest(&self, channel: usize, ahead: usize) -> Sample {
        self.samples[channel * 2 * SPAN + self.head + SPAN - 1 - ahead]
    }

    /// `channel`'s frames under the `weighed` taps, laid out as [`Kernel::weigh`] lays its
    /// `weights`: the frame `ahead` before the newest at [`CENTRE`].
    fn interpolate(
        &self,
        channel: usize,
        ahead: usize,
        weighed: &Weighed,
        weights: &[f32],
    ) -> Sample {
        let Weighed { taps, gain } = weighed;
        let start = channel * 2 * SPAN + self.head + SPAN - 1 - ahead - CENTRE;
        let frames = &self.samples[start + taps.start..start + taps.end];
        dot(frames, &weights[taps.clone()]) * gain
    }
}

/// The sum of `a` and `b`'s products, kept in independent lanes that the compiler can hold in
/// vector registers: a single running float sum has to be added in order and can't be.
fn dot(a: &[Sample], b: &[f32]) -> Sample {
    const LANES: usize = 8;
    let (a_lanes, a_rest) = a.as_chunks::<LANES>();
    let (b_lanes, b_rest) = b.as_chunks::<LANES>();
    let mut lanes = [0.0; LANES];
    for (a, b) in a_lanes.iter().zip(b_lanes) {
        for lane in 0..LANES {
            lanes[lane] += a[lane] * b[lane];
        }
    }
    let rest: Sample = a_rest.iter().zip(b_rest).map(|(a, b)| a * b).sum();
    lanes.iter().sum::<Sample>() + rest
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
