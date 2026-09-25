//! Why an exclusive claim on a device did not go through.
//!
//! Two types because two readers want different things. [`ClaimError`] keeps the typed cause for
//! the log line the fallback writes; [`FallbackReason`] is what [`super::Negotiated`] carries to
//! the panel, which only ever needs to name the cause, and which has to compare equal across
//! ticks for the watch it rides on to stay quiet.

/// What a backend's own error is carried as. Boxed rather than named because the backends are
/// per-platform and this type is not.
pub type ClaimSource = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error("the device is in use by another client")]
    Busy(#[source] ClaimSource),
    #[error("{by} holds the device's reservation")]
    Reserved { by: String },
    #[error("the device cannot run at {rate} Hz")]
    RateRefused { rate: u32 },
    #[error("the device cannot run {channels} channels or more")]
    ChannelsRefused { channels: u16 },
    #[error("the device takes none of the formats a {bits}-bit source can be written in")]
    FormatRefused { bits: u8 },
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

/// The cause of a fallback, as the panel names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    Busy,
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
