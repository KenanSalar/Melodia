//! Cheap connection/enabled snapshots for the settings UI — the payload of the
//! service's `watch<ScrobbleStatus>`. Pure data + the projection from the shadow
//! parts; no locking or I/O (that lives on [`super::ScrobbleService`]).

use super::credentials::ScrobbleCredentials;
use melodia_core::entities::integrations::ScrobbleFlags;

/// Connection + enable state for one provider.
#[derive(Debug, Clone, Default)]
pub struct ProviderStatus {
    pub connected: bool,
    pub username: Option<String>,
    pub enabled: bool,
    /// Whether favorites are mirrored to this provider's loved tracks.
    pub love_enabled: bool,
}

/// A cheap snapshot of scrobble state for seeding the settings UI.
#[derive(Debug, Clone, Default)]
pub struct ScrobbleStatus {
    pub lastfm: ProviderStatus,
    pub listenbrainz: ProviderStatus,
    pub mbid_auto_tag: bool,
}

/// Which provider a retroactive love backfill targets. Provider-scoped so
/// enabling/connecting one service doesn't re-love the whole library on the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoveTarget {
    Lastfm,
    ListenBrainz,
}

/// Build a [`ScrobbleStatus`] snapshot from the raw shadow parts. Shared by
/// `ScrobbleService::status` and `init` (which needs it before `Self` exists) —
/// `pub(super)` so both sites in the parent module can reach it.
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
        mbid_auto_tag: flags.mbid_auto_tag,
    }
}
