//! Bringing one source's samples to the device's rate and channel count, pull-driven like the
//! chain above. Where the rates differ or the speed isn't one, each output frame is interpolated,
//! linearly or through [`super::resample`]'s kernel; where they match, the source's own frames
//! pass untouched, which bit-perfect output rests on.
//!
//! Playback speed lives here, in the ratio, so [`AudioSource::sample_rate`] means the source's own
//! rate at every level and the clock counts media frames with no conversion.

use melodia_audio::player::source::audio::{AudioSource, Sample, Shape};

use super::resample::{self, CENTRE, Kernel, SPAN, Weighed};

/// How a converter interpolates wherever the rates differ or the speed isn't one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resampler {
    /// Between the two frames either side of the point. Cheap at any device rate, but it softens
    /// the top octave and only attenuates the source's images, so what lands past the output's
    /// Nyquist folds back into the band, loudest on a downsample and on an upsample short of twice
    /// the source's rate (44.1 to 48 kHz among them), and fainter above that.
    Linear,
    /// Through [`super::resample`]'s kernel, which keeps the whole band. Its cost per output frame
    /// is the kernel's span, so it follows the device's rate rather than the track's.
    BandLimited,
}

impl Default for Resampler {
    /// Linear on Windows alone, whose shared mode converts every stream to the system's mix format
    /// however high it runs. Elsewhere a shared stream can follow the file's rate, and on Linux
    /// otherwise opens at 48 kHz, where the kernel is cheap and 44.1 kHz material is the case
    /// linear aliases most.
    fn default() -> Self {
        if cfg!(target_os = "windows") { Self::Linear } else { Self::BandLimited }
    }
}

/// What one [`Converter::fill`] converts to.
#[derive(Debug, Clone, Copy)]
pub struct Conversion {
    pub device: Shape,
    /// Scales the source's rate rather than the device's, so `1.0` against equal rates steps
    /// exactly one source frame per output frame and every sample passes through untouched.
    pub speed: f64,
    pub resampler: Resampler,
}

/// A struct so the two counts, unequal once a rate or channel count differs, can't swap and run the
/// clock at the device's rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filled {
    /// Samples written to the output block, interleaved at the device's channel count.
    pub samples: usize,
    /// Frames taken from the source, on its own timeline. This is what the clock counts.
    pub source_frames: u64,
}

/// The device is handed to every [`Self::fill`] rather than held, nothing here depending on it
/// between calls, so an output can reopen at another shape under a source part way through.
///
/// A source running dry leaves the converter [starved], owed frames: a successor of the same shape
/// takes it over, converting a gapless seam as one stream, or [`Self::drain`] ends it.
///
/// [starved]: Self::is_starved
pub struct Converter {
    source: Shape,
    /// The frames around the one being written. Held across calls and, at a gapless seam, across
    /// sources, so neither a block boundary nor a track boundary restarts the interpolation.
    window: Window,
    /// How far past the centre frame the next output frame falls.
    position: f64,
    /// Frames to take in before the next output frame: one for every whole step the position
    /// crosses that the lookahead doesn't already cover.
    owed: u32,
    /// Frames taken in past the centre. None while the output copies the source's own frames, so
    /// a bit-perfect path runs no later than the source; the interpolation's reach once it
    /// interpolates.
    ahead: usize,
    /// Silent frames pushed since the source ran out. The centre reaching one ends the converter.
    pads: usize,
    state: State,
    incoming: Box<[Sample]>,
    weights: Box<[f32]>,
    /// What `weights` hold. An integer step brings the position back to where it was, so there
    /// every output frame would weigh exactly what the one before did.
    laid: Option<Laid>,
}

/// The position and step `weights` were laid out for, as bits, and what laying them out found.
struct Laid {
    position: u64,
    step: u64,
    weighed: Weighed,
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

/// How one fill makes its frames, settled once for the whole block.
#[derive(Clone, Copy)]
enum Method {
    Copy,
    Linear,
    Kernel(&'static Kernel),
}

impl Method {
    fn of(step: f64, resampler: Resampler) -> Self {
        if step.to_bits() == 1.0_f64.to_bits() {
            return Self::Copy;
        }
        match resampler {
            Resampler::Linear => Self::Linear,
            // An unbuilt table means it was asked for without being prepared, and building it
            // here would allocate on the audio thread. The block goes linear instead.
            Resampler::BandLimited => resample::prepared().map_or(Self::Linear, Self::Kernel),
        }
    }

    /// Frames past the centre this method reads.
    fn reach(self, step: f64) -> usize {
        match self {
            Self::Copy => 0,
            Self::Linear => 1,
            Self::Kernel(_) => resample::reach(step),
        }
    }
}

/// How one output frame is made from the window.
enum Made {
    Copied,
    /// This fraction of the way from the centre frame to the next.
    Lerped(Sample),
    Weighed(Weighed),
}

/// A position inside one frame, at the width of the samples it weighs.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a fraction in [0, 1) loses nothing a sample's own precision would keep"
)]
fn fraction(position: f64) -> Sample {
    position as Sample
}

impl Converter {
    pub fn new(source: Shape) -> Self {
        let width = usize::from(source.channels.get());
        Self {
            source,
            window: Window::new(width),
            position: 0.0,
            owed: 1,
            ahead: 0,
            pads: 0,
            state: State::Pulling,
            incoming: vec![0.0; width].into_boxed_slice(),
            weights: vec![0.0; SPAN].into_boxed_slice(),
            laid: None,
        }
    }

    /// Write up to `out.len()` samples at the device's shape, pulling from `src` as the ratio
    /// demands.
    ///
    /// A short return means the converter starved or finished; `out` past that point is untouched.
    pub fn fill(
        &mut self,
        out: &mut [Sample],
        src: &mut dyn AudioSource,
        conversion: Conversion,
    ) -> Filled {
        let Conversion { device, speed, resampler } = conversion;
        let width = usize::from(device.channels.get());
        let step = f64::from(self.source.rate.get()) * speed / f64::from(device.rate.get());
        let method = Method::of(step, resampler);
        if matches!(method, Method::Copy) {
            // Only a position of zero hands the source's own frames over, so one an earlier speed
            // left part way between two is dropped once the step is back to exactly one.
            self.position = 0.0;
        }
        let reach = method.reach(step);

        let mut filled = Filled { samples: 0, source_frames: 0 };
        if !self.settle(src, reach, &mut filled.source_frames) {
            return filled;
        }
        for frame in out.chunks_exact_mut(width) {
            let made = match method {
                Method::Copy => Made::Copied,
                Method::Linear => Made::Lerped(fraction(self.position)),
                Method::Kernel(kernel) => Made::Weighed(self.weigh(kernel, step)),
            };
            self.write_frame(frame, &made);
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

    /// Nothing follows: the window plays out against silence. The silence already crossed into is
    /// taken here rather than at the next fill, so a finished source is done once drained.
    pub fn drain(&mut self) {
        if self.state == State::Done {
            return;
        }
        self.state = State::Draining;
        while self.owed > 0 && self.pads <= self.ahead {
            self.pad();
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

    /// Lay the kernel's weights out for an output frame at the current position, unless they are
    /// already laid out for this position and step.
    fn weigh(&mut self, kernel: &Kernel, step: f64) -> Weighed {
        let (position, step_bits) = (self.position.to_bits(), step.to_bits());
        if let Some(laid) = &self.laid
            && laid.position == position
            && laid.step == step_bits
        {
            return laid.weighed.clone();
        }
        let weighed = kernel.weigh(self.position, step, &mut self.weights);
        self.laid = Some(Laid { position, step: step_bits, weighed: weighed.clone() });
        weighed
    }

    /// Move the centre on a frame, into the lookahead where there is some.
    fn advance(&mut self) {
        if self.ahead > 0 {
            self.ahead -= 1;
        } else {
            self.owed += 1;
        }
    }

    /// The frames the position crossed, then lookahead up to `reach`; false where the window can't
    /// get there. A lookahead pushed while copying drains as the centre moves through it rather
    /// than being dropped, so the output never skips a frame.
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
            State::Draining => self.pad(),
            State::Done => return false,
        }
        true
    }

    /// Take a silent frame in past the source's end.
    fn pad(&mut self) {
        self.window.push_silence();
        self.pads += 1;
    }

    /// A mono device is the ladder's second rung, so its fold has to be right: the mean, since
    /// [`Shape`] names no channels and at `1/n` an unknown one costs some level rather than routing
    /// LFE in at full scale. A wider device keeps the first `min(from, to)` channels, folding none.
    fn write_frame(&self, frame: &mut [Sample], made: &Made) {
        let from = usize::from(self.source.channels.get());
        let source = |channel: usize| match made {
            Made::Copied => self.window.behind_newest(channel, self.ahead),
            Made::Lerped(fraction) => self.window.lerp(channel, self.ahead, *fraction),
            Made::Weighed(weighed) => {
                self.window.interpolate(channel, self.ahead, weighed, &self.weights)
            }
        };

        if frame.len() == 1 && from > 1 {
            let sum: Sample = (0..from).map(source).sum();
            frame[0] = sum / Sample::from(self.source.channels.get());
            return;
        }

        if from == 1 {
            // At unity: a pan-law trim would restage every mono file on a stereo device, and each
            // channel would stop being the source's own sample.
            let sample = source(0);
            for (channel, slot) in frame.iter_mut().enumerate() {
                *slot = if channel < 2 { sample } else { 0.0 };
            }
            return;
        }

        for (channel, slot) in frame.iter_mut().enumerate() {
            *slot = if channel < from { source(channel) } else { 0.0 };
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
        self.samples[self.slot(channel, ahead)]
    }

    /// Where in `samples` [`Self::behind_newest`] reads.
    fn slot(&self, channel: usize, ahead: usize) -> usize {
        channel * 2 * SPAN + self.head + SPAN - 1 - ahead
    }

    /// `channel`'s value `fraction` of the way from the frame `ahead` before the newest to the one
    /// after it.
    fn lerp(&self, channel: usize, ahead: usize, fraction: Sample) -> Sample {
        let at = self.slot(channel, ahead);
        let (from, to) = (self.samples[at], self.samples[at + 1]);
        from + (to - from) * fraction
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
        let start = self.slot(channel, ahead) - CENTRE;
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
