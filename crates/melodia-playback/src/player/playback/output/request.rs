//! What an open asks the device for: the route, the device, and for an exclusive claim the shape,
//! format and pacing it is asked at. [`super::negotiated`] is what comes back.

use std::time::Duration;

use melodia_audio::player::source::audio::{SampleRate, Shape, SourceFormat};

/// Whether the output goes through the system mixer or takes the device for itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputMode {
    #[default]
    Shared,
    Exclusive,
}

/// What an open asks the device for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRequest {
    /// The named device, or the system's default where none is, at `rate` where one is given and
    /// the device's own config otherwise.
    Shared {
        rate: Option<SampleRate>,
        /// One of [`super::devices`]' ids, only where [`super::SHARED_DEVICE_SUPPORTED`].
        device: Option<String>,
    },
    Exclusive(ExclusiveRequest),
}

impl Default for OutputRequest {
    fn default() -> Self {
        Self::Shared { rate: None, device: None }
    }
}

/// A claim on one device, asked for at the source's shape and a format that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExclusiveRequest {
    /// The device's id, or `None` for the first the system lists.
    pub device: Option<String>,
    pub shape: Shape,
    pub format: SourceFormat,
    pub tuning: ExclusiveTuning,
    /// Carry the volume on the device's own control where it has one in hardware, leaving the
    /// samples untouched. Part of the request so a change reopens the claim: the gain moving
    /// between the voices and the device mid-stream can play a period at the wrong level.
    pub hardware_volume: bool,
    /// Part of the request so a change reopens the claim: one that fell back to shared can take
    /// the device at another rate, and one converting gives it back.
    pub rate_fallback: RateFallback,
}

/// How an exclusive writer paces the device. Part of the request, so changing it reopens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExclusiveTuning {
    /// What the writer hands the device at a time. A device that can't run it gets the nearest
    /// period it can, which [`super::Negotiated::period`] reports.
    pub period: Duration,
    pub drive: Drive,
}

impl ExclusiveTuning {
    /// Short enough that a stop lands quickly, long enough that the writer wakes rarely.
    pub const DEFAULT_PERIOD: Duration = Duration::from_millis(20);
    /// Below a couple of milliseconds no device keeps up, and every one rounds it up anyway.
    pub const MIN_PERIOD: Duration = Duration::from_millis(2);
    /// A stop waits out a period, and a voice's command waits for a fill, so a longer one would
    /// start to read as a hang.
    pub const MAX_PERIOD: Duration = Duration::from_millis(100);

    /// `period` held inside the range a claim asks for.
    pub fn new(period: Duration, drive: Drive) -> Self {
        Self { period: period.clamp(Self::MIN_PERIOD, Self::MAX_PERIOD), drive }
    }
}

impl Default for ExclusiveTuning {
    fn default() -> Self {
        Self { period: Self::DEFAULT_PERIOD, drive: Drive::default() }
    }
}

/// What wakes the exclusive writer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Drive {
    /// The device signals each time it wants a period, and gets exactly one.
    #[default]
    Events,
    /// The writer wakes on a timer and tops the device's buffer up. Some USB drivers stutter in
    /// event mode and play cleanly in this one. Only WASAPI tells the two apart; ALSA's writer
    /// always blocks on the card.
    Polling,
}

impl Drive {
    /// The drive a polling toggle stands for.
    pub fn from_polling(polling: bool) -> Self {
        if polling { Self::Polling } else { Self::Events }
    }
}

/// What a claim does where the device lacks the source's rate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RateFallback {
    /// Give the claim up and play shared, as a refused channel count or format does.
    #[default]
    Shared,
    /// Keep the claim at another of the device's rates, which the voices convert to.
    Resample,
}

/// A device an exclusive claim can be aimed at, and where [`super::SHARED_DEVICE_SUPPORTED`] a
/// shared stream too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputDevice {
    /// Stable across reboots and replugs, which is what makes it the persisted choice.
    pub id: String,
    pub name: String,
}
