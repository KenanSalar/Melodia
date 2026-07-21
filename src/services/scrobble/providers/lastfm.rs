//! Last.fm provider. Phase 0 only exposes the compile-time app credentials and
//! the gate that keys the whole Last.fm surface off them; the signed API calls
//! (`get_token` / `get_session` / `update_now_playing` / `scrobble_batch` /
//! `love`) land in Phase 1.
//!
//! The API key + shared secret identify the *application*, not a listening
//! account — every user scrobbles to their own account via their own session
//! key. They're read at compile time from the environment so nothing secret is
//! committed: official releases inject them as CI secrets, while a keyless local
//! build leaves `ListenBrainz` fully working and the Last.fm surface inert.

pub const LASTFM_API_KEY: Option<&str> = option_env!("LASTFM_API_KEY");
pub const LASTFM_SHARED_SECRET: Option<&str> = option_env!("LASTFM_SHARED_SECRET");

/// Whether this build shipped Last.fm app credentials (both key and secret).
/// Gates the Last.fm setter / UI / detector so a keyless build ships
/// ListenBrainz-only.
pub fn is_configured() -> bool {
    LASTFM_API_KEY.is_some() && LASTFM_SHARED_SECRET.is_some()
}
