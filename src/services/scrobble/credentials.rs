//! Persisted scrobble credentials, stored in their own `scrobble_credentials.json`
//! (never in `settings.json`). Tokens are low-sensitivity, revocable and scoped,
//! so plaintext JSON is acceptable — on Unix the file is chmod `0o600`; on Windows
//! the per-user `AppData` ACL already restricts it.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::services;

/// Last.fm session credentials. The session key is long-lived (no expiry) and
/// scoped to scrobbling on the user's own account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastfmCredentials {
    pub session_key: String,
    pub username: String,
}

/// `ListenBrainz` user token, validated once at connect time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenBrainzCredentials {
    pub token: String,
    pub username: String,
}

/// The on-disk credential file. Either side is `None` until that provider is
/// connected; the whole struct defaults to empty for a first launch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrobbleCredentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lastfm: Option<LastfmCredentials>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listenbrainz: Option<ListenBrainzCredentials>,
}

/// Read the credential file, defaulting to empty on a missing or unparseable file.
pub fn load(path: &Path) -> AppResult<ScrobbleCredentials> {
    services::load_json_or_default_sync(path)
}

/// Atomically write the credential file, then tighten it to owner-only on Unix.
///
/// The atomic writer streams through a `NamedTempFile` in the same directory and
/// renames on success; the `0o600` chmod afterward is net-new (no existing
/// secure-write helper) and is a no-op on Windows.
pub fn save(path: &Path, credentials: &ScrobbleCredentials) -> AppResult<()> {
    services::write_json_atomic_sync(path, credentials)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
