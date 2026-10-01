//! What the samples go through between the file and the device, and whether any of it changed them.
//!
//! [`evaluate`] is the one place the verdict is decided. It is pure, the monitor runs it on its
//! tick, and the panel only ever renders what it answered.
//!
//! **Shared output never reads Bit-perfect.** The system mixer sits between the stream and the
//! card and may convert or mix, and nothing here can see past it. Only an exclusive claim, in a
//! format that holds the source, grades the output clean.
//!
//! **A fallback is the headline whatever the stages say**, since the user asked for exclusive and
//! didn't get it, and that is the one thing they need to hear first.
//!
//! **A lossy source is the headline after it.** No setting puts a lossy file right, so whatever
//! the later stages do, the answer that matters is that the device gets a reconstruction.

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
    /// Exclusive was asked for and refused; the reason is on the negotiated output.
    Fallback,
    /// The source was decoded from a lossy codec.
    Lossy,
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
        _ if inputs.negotiated.fallback.is_some() => Verdict::Fallback,
        _ if inputs.source.format.lossy => Verdict::Lossy,
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
    // With the device's own control carrying the level, only the silence at zero reaches the
    // samples, and the voices apply that themselves. Where the system left that control grades
    // nothing either way: the samples arrive untouched, and `device_level` only names it.
    let attenuated = if inputs.negotiated.hardware_volume {
        transport.volume == 0
    } else {
        transport.volume != MAX_VOLUME
    };

    let format = inputs.source.format;
    Stages {
        source: converted_if(format.lossy || !format.fits_sample()),
        dsp: chosen_if(inputs.source.dsp_engaged),
        // Exact comparisons, because the chain's own short circuits are: the converter passes
        // samples through only at a step of exactly one, and the voice skips its multiply only at
        // bitwise unity.
        speed: chosen_if(transport.speed.to_bits() != 1.0_f64.to_bits()),
        volume: chosen_if(transport.muted || attenuated),
        crossfade: chosen_if(transport.crossfading),
        rate: converted_if(source.rate != device.rate),
        // A source no wider than the device lands on its first channels untouched, the rest
        // silent or a mono duplicate. Wider is a drop, or a mean onto a mono device.
        channels: converted_if(source.channels > device.channels),
        output: converted_if(!inputs.negotiated.format.carries(format)),
    }
}

#[cfg(test)]
#[path = "tests/signal_path_tests.rs"]
mod tests;
