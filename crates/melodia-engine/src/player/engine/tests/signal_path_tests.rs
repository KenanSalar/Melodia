//! Tests for the verdict: each stage against the input that moves it, one row at a time, over a
//! path where everything else is clean.

use std::num::NonZero;

use melodia_audio::player::source::audio::{Shape, SourceFormat};
use melodia_playback::player::playback::output::Negotiated;
use melodia_playback::player::playback::output::voice::PlayingSource;

use super::{Grade, SignalInputs, Stages, Transport, Verdict, evaluate};

fn shape(channels: u16, rate: u32) -> Shape {
    Shape {
        channels: NonZero::new(channels).unwrap_or(NonZero::<u16>::MIN),
        rate: NonZero::new(rate).unwrap_or(NonZero::<u32>::MIN),
    }
}

/// A CD rip on a device at its rate, with nothing the user chose in the way.
fn clean_inputs() -> SignalInputs {
    SignalInputs {
        negotiated: Negotiated {
            device_name: Some("Test DAC".to_owned()),
            shape: shape(2, 44_100),
            format: cpal::SampleFormat::F32,
            requested_period: None,
            period: None,
        },
        source: PlayingSource {
            shape: shape(2, 44_100),
            format: SourceFormat { bits: 16, float: false },
            dsp_engaged: false,
        },
        eq_on: false,
        rg_on: false,
        transport: Transport { volume: 100, muted: false, speed: 1.0, crossfading: false },
    }
}

/// Every stage clean but the output, which shared mode always converts.
const CLEAN: Stages = Stages {
    source: Grade::Clean,
    dsp: Grade::Clean,
    speed: Grade::Clean,
    volume: Grade::Clean,
    crossfade: Grade::Clean,
    rate: Grade::Clean,
    channels: Grade::Clean,
    output: Grade::Converted,
};

/// Shared output is out of sight, so no amount of care upstream of it earns the claim.
#[test]
fn shared_output_never_reads_bit_perfect() {
    let path = evaluate(clean_inputs());

    assert_eq!(path.stages, CLEAN);
    assert_eq!(path.verdict, Verdict::Converted);
}

/// What a row names, the one input it moves, and the stages that should come out.
type Case = (&'static str, fn(&mut SignalInputs), Stages);

/// One input moved per row, each on or either side of the boundary the chain's own short circuits
/// draw, so a row failing names the stage whose rule moved.
#[test]
fn each_stage_grades_the_input_that_moves_it() {
    let cases: [Case; 18] = [
        ("24-bit integer fits f32", |i| i.source.format.bits = 24, CLEAN),
        (
            "32-bit integer loses its low bits",
            |i| i.source.format.bits = 32,
            Stages { source: Grade::Converted, ..CLEAN },
        ),
        ("32-bit float fits", |i| i.source.format = SourceFormat::F32, CLEAN),
        (
            "64-bit float is narrowed",
            |i| i.source.format = SourceFormat { bits: 64, float: true },
            Stages { source: Grade::Converted, ..CLEAN },
        ),
        (
            "engaged DSP is the user's",
            |i| i.source.dsp_engaged = true,
            Stages { dsp: Grade::Enhanced, ..CLEAN },
        ),
        ("an EQ that is on with nothing to do", |i| i.eq_on = true, CLEAN),
        (
            "any speed but exactly one",
            |i| i.transport.speed = 1.000_001,
            Stages { speed: Grade::Enhanced, ..CLEAN },
        ),
        (
            "one step under full volume",
            |i| i.transport.volume = 99,
            Stages { volume: Grade::Enhanced, ..CLEAN },
        ),
        (
            "muted at full volume",
            |i| i.transport.muted = true,
            Stages { volume: Grade::Enhanced, ..CLEAN },
        ),
        (
            "a crossfade in flight",
            |i| i.transport.crossfading = true,
            Stages { crossfade: Grade::Enhanced, ..CLEAN },
        ),
        (
            "a device at another rate",
            |i| i.negotiated.shape = shape(2, 48_000),
            Stages { rate: Grade::Converted, ..CLEAN },
        ),
        (
            "a file at another rate",
            |i| i.source.shape = shape(2, 96_000),
            Stages { rate: Grade::Converted, ..CLEAN },
        ),
        ("mono duplicated onto stereo", |i| i.source.shape = shape(1, 44_100), CLEAN),
        ("stereo onto a wider device", |i| i.negotiated.shape = shape(6, 44_100), CLEAN),
        (
            "surround onto stereo drops channels",
            |i| i.source.shape = shape(6, 44_100),
            Stages { channels: Grade::Converted, ..CLEAN },
        ),
        (
            "stereo onto a mono device is a mean",
            |i| i.negotiated.shape = shape(1, 44_100),
            Stages { channels: Grade::Converted, ..CLEAN },
        ),
        (
            "three channels onto stereo, one past the device",
            |i| i.source.shape = shape(3, 44_100),
            Stages { channels: Grade::Converted, ..CLEAN },
        ),
        ("an equally wide device", |i| i.negotiated.shape = shape(2, 44_100), CLEAN),
    ];

    for (what, change, expected) in cases {
        let mut inputs = clean_inputs();
        change(&mut inputs);
        assert_eq!(evaluate(inputs).stages, expected, "{what}");
    }
}
