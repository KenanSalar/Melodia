//! Scrobble credentials, in a plaintext file of their own rather than `settings.json`
//! (`docs/adr/0023-scrobbling-is-a-read-only-tap.md`).
//!
//! `Debug` leaves each secret out: users attach the log tail to public issues.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use melodia_core::error::AppResult;

/// The session key doesn't expire.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastfmCredentials {
    pub session_key: String,
    pub username: String,
}

impl fmt::Debug for LastfmCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LastfmCredentials")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

/// Validated once, at connect.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenBrainzCredentials {
    pub token: String,
    pub username: String,
}

impl fmt::Debug for ListenBrainzCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListenBrainzCredentials")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrobbleCredentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lastfm: Option<LastfmCredentials>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listenbrainz: Option<ListenBrainzCredentials>,
}

pub fn load(path: &Path) -> AppResult<ScrobbleCredentials> {
    melodia_core::utils::atomic_file::load_json_or_default_sync(path)
}

/// Owner-only on Unix; on Windows the per-user `AppData` ACL already restricts it.
pub fn save(path: &Path, credentials: &ScrobbleCredentials) -> AppResult<()> {
    melodia_core::utils::atomic_file::write_json_sync(path, credentials)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
