//! Why an exclusive claim on a device did not go through.
//!
//! Two types because two readers want different things. [`ClaimError`] keeps the typed cause for
//! the log line the fallback writes; [`FallbackReason`] is what [`super::Negotiated`] carries to
//! the panel, which only ever needs to name the cause, and which has to compare equal across
//! ticks for the watch it rides on to stay quiet.

use melodia_audio::player::source::audio::SourceFormat;

/// What a backend's own error is carried as. Boxed rather than named because the backends are
/// per-platform and this type is not.
pub type ClaimSource = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error("the device is in use by another client")]
    Busy(#[source] ClaimSource),
    #[error("the system does not allow exclusive control of the device")]
    NotAllowed(#[source] ClaimSource),
    #[error("{by} holds the device's reservation")]
    Reserved { by: String },
    #[error("the device cannot run at {rate} Hz")]
    RateRefused { rate: u32 },
    #[error("the device cannot run {channels} channels or more")]
    ChannelsRefused { channels: u16 },
    #[error("the device takes none of the formats a {format} source can be written in")]
    FormatRefused { format: SourceFormat },
    #[error("the device {id} is not connected")]
    NotConnected { id: String },
    #[error("exclusive output is not available on this platform")]
    Unsupported,
    #[error("{context}")]
    Io {
        context: &'static str,
        #[source]
        source: ClaimSource,
    },
}

impl ClaimError {
    pub fn io(context: &'static str, source: impl Into<ClaimSource>) -> Self {
        Self::Io { context, source: source.into() }
    }

    pub fn reason(&self) -> FallbackReason {
        match self {
            Self::Busy(_) => FallbackReason::Busy,
            Self::NotAllowed(_) => FallbackReason::NotAllowed,
            Self::Reserved { by } => FallbackReason::Reserved { by: by.clone() },
            Self::RateRefused { .. } => FallbackReason::RateRefused,
            Self::ChannelsRefused { .. } => FallbackReason::ChannelsRefused,
            Self::FormatRefused { .. } => FallbackReason::FormatRefused,
            Self::NotConnected { .. } => FallbackReason::NotConnected,
            Self::Unsupported => FallbackReason::Unsupported,
            Self::Io { .. } => FallbackReason::Io,
        }
    }
}

/// An exclusive claim that was refused, as the shared stream opened in its place reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    pub reason: FallbackReason,
    /// The device that refused, which is not the one the audio went to. `None` where it could not
    /// be found, as a disconnected one can't.
    pub device: Option<String>,
}

/// The cause of a fallback, as the panel names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    Busy,
    /// The user has switched exclusive control off for the device, which only Windows offers.
    NotAllowed,
    /// Another application holds the card's reservation at a priority above ours.
    Reserved {
        by: String,
    },
    RateRefused,
    ChannelsRefused,
    FormatRefused,
    NotConnected,
    Unsupported,
    Io,
}

impl FallbackReason {
    /// Whether a track start asks for the same claim again: the holder may have let go, the user
    /// allowed exclusive control, or a fault cleared. A rate, channel count or format the device
    /// lacks is refused the same way every time, and a device that wasn't connected is the reclaim
    /// poll's, which asks only once it is listed again.
    pub fn retry_at_track_start(&self) -> bool {
        match self {
            Self::Busy | Self::NotAllowed | Self::Reserved { .. } | Self::Io => true,
            Self::NotConnected
            | Self::RateRefused
            | Self::ChannelsRefused
            | Self::FormatRefused
            | Self::Unsupported => false,
        }
    }
}

#[cfg(test)]
#[path = "tests/claim_tests.rs"]
mod tests;
