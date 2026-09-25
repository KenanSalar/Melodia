//! What the samples go through between the file and the device, and whether any of it changed them.
//!
//! [`evaluate`] is the one place the verdict is decided. It is pure, the monitor runs it on its
//! tick, and the panel only ever renders what it answered.
//!
//! **Shared output never reads Bit-perfect.** The system mixer sits between the stream and the
//! card and may convert or mix, and nothing here can see past it. Every output is shared until an
//! exclusive backend exists, so for now the headline is at best Converted and the stages carry
//! the detail.

use melodia_playback::player::playback::output::Negotiated;
use melodia_playback::player::playback::output::voice::PlayingSource;

use super::state::MAX_VOLUME;

/// How one stage left the samples, worst last so the verdict is the maximum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Grade {
    Clean,
    /// Changed by something the user chose and can turn off.
    Enhanced,
    /// Changed on the way to the device, whatever the user chose.
    Converted,
}

/// The headline over all the stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    BitPerfect,
    Enhanced,
    Converted,
}

/// Everything the verdict reads, kept on the answer so the panel can name what it saw.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalInputs {
    pub negotiated: Negotiated,
    pub source: PlayingSource,
    /// Which of the two settings behind `source.dsp_engaged` are on. They name the stage and
    /// never grade it: an EQ that is on with nothing to do at this rate leaves it clean.
    pub eq_on: bool,
    pub rg_on: bool,
    pub transport: Transport,
}

/// The half of the inputs the state machine holds rather than the backend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transport {
    pub volume: u32,
    pub muted: bool,
    pub speed: f64,
    pub crossfading: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stages {
    pub source: Grade,
    pub dsp: Grade,
    pub speed: Grade,
    pub volume: Grade,
    pub crossfade: Grade,
    pub rate: Grade,
    pub channels: Grade,
    pub output: Grade,
}

impl Stages {
    fn worst(self) -> Grade {
        [
            self.source,
            self.dsp,
            self.speed,
            self.volume,
            self.crossfade,
            self.rate,
            self.channels,
            self.output,
        ]
        .into_iter()
        .max()
        .unwrap_or(Grade::Clean)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SignalPath {
    pub inputs: SignalInputs,
    pub stages: Stages,
    pub verdict: Verdict,
}

/// Grades each stage and the whole path.
pub fn evaluate(inputs: SignalInputs) -> SignalPath {
    let stages = grade(&inputs);
    let verdict = match stages.worst() {
        Grade::Clean => Verdict::BitPerfect,
        Grade::Enhanced => Verdict::Enhanced,
        Grade::Converted => Verdict::Converted,
    };
    SignalPath { inputs, stages, verdict }
}

fn grade(inputs: &SignalInputs) -> Stages {
    let source = inputs.source.shape;
    let device = inputs.negotiated.shape;
    let transport = inputs.transport;
    let chosen_if = |touched: bool| if touched { Grade::Enhanced } else { Grade::Clean };
    let converted_if = |touched: bool| if touched { Grade::Converted } else { Grade::Clean };

    Stages {
        source: converted_if(!inputs.source.format.fits_sample()),
        dsp: chosen_if(inputs.source.dsp_engaged),
        // Exact comparisons, because the chain's own short circuits are: the converter passes
        // samples through only at a step of exactly one, and the voice skips its multiply only at
        // bitwise unity.
        speed: chosen_if(transport.speed.to_bits() != 1.0_f64.to_bits()),
        volume: chosen_if(transport.muted || transport.volume != MAX_VOLUME),
        crossfade: chosen_if(transport.crossfading),
        rate: converted_if(source.rate != device.rate),
        // A source no wider than the device lands on its first channels untouched, the rest
        // silent or a mono duplicate. Wider is a drop, or a mean onto a mono device.
        channels: converted_if(source.channels > device.channels),
        output: Grade::Converted,
    }
}

#[cfg(test)]
#[path = "tests/signal_path_tests.rs"]
mod tests;
