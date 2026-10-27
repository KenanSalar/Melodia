//! What a device agreed to, beside what [`super::request`] asked it for.

use std::fmt;

use melodia_audio::player::source::audio::{Shape, SourceFormat};

use super::claim::Fallback;
use super::encode::DeviceFormat;
use super::rates::RateSet;

/// The sample format a stream was opened with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// cpal's, on a shared stream. The system mixer sits past it, so it proves nothing about what
    /// the card receives.
    Shared(cpal::SampleFormat),
    Exclusive(DeviceFormat),
}

impl OutputFormat {
    /// Whether a source in `source` reaches the card unchanged through this format.
    pub fn carries(self, source: SourceFormat) -> bool {
        match self {
            Self::Shared(_) => false,
            Self::Exclusive(format) => format.carries(source),
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shared(format) => format.fmt(f),
            Self::Exclusive(format) => format.fmt(f),
        }
    }
}

/// Reported rather than assumed: every part can differ from the request, and a bit-perfect mode is
/// only checkable against this end of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiated {
    /// What the host calls the device, or `None` where it would not say. After a reopen this is
    /// the only thing telling a bug report which output the audio moved to.
    pub device_name: Option<String>,
    pub shape: Shape,
    pub format: OutputFormat,
    /// Why an exclusive claim fell back to this shared stream, or `None` where none was refused.
    pub fallback: Option<Fallback>,
    /// Whether the device's own control carries the volume, so the voices hand it the samples at
    /// unity. Only an exclusive claim that asked for it, on a device with one in hardware.
    pub hardware_volume: bool,
    /// The control as the claim read it: the samples arrive untouched, but a level the system left
    /// low or muted plays them quieter. Where it carries the volume only the switch is the
    /// system's. `None` on a shared stream and where the backend can't read one.
    pub device_level: Option<DeviceLevel>,
    /// The standard rates offered a claim allowed to resample, which says whether another rate
    /// lands the device where it runs; `None` where no claim asked. A backend owes a superset of
    /// what its own fresh claim could see: narrower keeps a track converted that a fresh claim
    /// would play natively, wider costs a reopen at most.
    pub offered: Option<RateSet>,
    /// `None` where the host named its own; kept because it says which ladder pass won.
    pub requested_period: Option<cpal::FrameCount>,
    /// What the host says it hands the callback at a time, advisory and `None` where it can't say:
    /// for a bug report to read back, not a bound anything sizes against.
    pub period: Option<cpal::FrameCount>,
}

/// A device's own volume control, as the system left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceLevel {
    /// The percentage the system's own slider shows for it.
    pub percent: u8,
    pub muted: bool,
}

impl DeviceLevel {
    /// `level`, the fraction the system's slider shows, held to `0..=1`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a fraction held to 0..=1, scaled to 100 and rounded, fits a u8 exactly"
    )]
    pub fn new(level: f32, muted: bool) -> Self {
        let percent = (level.clamp(0.0, 1.0) * 100.0).round() as u8;
        Self { percent, muted }
    }
}
