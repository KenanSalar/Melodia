//! The scrobbling and Discord toggles, which live one layer below everything that reads them.
//!
//! Declared here rather than beside the rest of `settings.json`'s data model because their
//! readers are the integration services and the Settings page, and the file they are part of
//! belongs to neither. Plain serde data shared across tiers is what this directory is for; the
//! on-disk shape is unaffected, the settings root being what flattens them.

use serde::{Deserialize, Serialize};

/// Scrobbling toggles, all off by default. The Last.fm / `ListenBrainz`
/// *credentials* live in a separate `scrobble_credentials.json`, never here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent scrobble toggles, serde-flattened into settings.json — nesting would change the on-disk shape and break existing installs"
)]
pub struct ScrobbleFlags {
    pub lastfm_enabled: bool,
    pub listenbrainz_enabled: bool,
    /// Mirror favorites to Last.fm Loved Tracks. Independent of
    /// `lastfm_enabled` — loving isn't scrobbling — and of its sibling below.
    pub lastfm_love_enabled: bool,
    /// Mirror favorites to `ListenBrainz` recording feedback.
    pub listenbrainz_love_enabled: bool,
    /// Auto-tag scanned tracks with their `MusicBrainz` Recording ID, into both
    /// the DB and the file, so loves work on untagged libraries.
    pub mbid_auto_tag: bool,
}

/// Discord Rich Presence toggles. Nothing lives outside `settings.json` here —
/// the application id is a compile-time constant, not a credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordFlags {
    pub discord_rpc_enabled: bool,
    /// Show album artwork on the card. Its own toggle because it drives an
    /// outbound cover lookup; inert until the parent is enabled.
    pub discord_rpc_artwork: bool,
    /// Hide the card entirely while paused instead of showing a paused marker.
    pub discord_rpc_hide_when_paused: bool,
}

impl Default for DiscordFlags {
    fn default() -> Self {
        Self {
            discord_rpc_enabled: false,
            discord_rpc_artwork: true,
            discord_rpc_hide_when_paused: false,
        }
    }
}
