//! What the settings UI shows of scrobbling, projected from the service's shadow.

use super::credentials::ScrobbleCredentials;
use melodia_core::entities::integrations::ScrobbleFlags;

#[derive(Debug, Clone, Default)]
pub struct ProviderStatus {
    pub connected: bool,
    pub username: Option<String>,
    pub enabled: bool,
    /// Whether favorites are mirrored to this provider's loved tracks.
    pub love_enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ScrobbleStatus {
    pub lastfm: ProviderStatus,
    pub listenbrainz: ProviderStatus,
}

/// A backfill names one provider, so connecting one doesn't re-love the whole library on the
/// other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoveTarget {
    Lastfm,
    ListenBrainz,
}

pub(super) fn build_status(
    credentials: &ScrobbleCredentials,
    flags: &ScrobbleFlags,
) -> ScrobbleStatus {
    ScrobbleStatus {
        lastfm: ProviderStatus {
            connected: credentials.lastfm.is_some(),
            username: credentials.lastfm.as_ref().map(|c| c.username.clone()),
            enabled: flags.lastfm_enabled,
            love_enabled: flags.lastfm_love_enabled,
        },
        listenbrainz: ProviderStatus {
            connected: credentials.listenbrainz.is_some(),
            username: credentials.listenbrainz.as_ref().map(|c| c.username.clone()),
            enabled: flags.listenbrainz_enabled,
            love_enabled: flags.listenbrainz_love_enabled,
        },
    }
}
