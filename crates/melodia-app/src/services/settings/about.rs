//! The flag structs behind the Settings ▸ About tab: updates, diagnostics, and what the welcome card
//! and the support prompt remember.

use serde::{Deserialize, Serialize};

/// Auto-updater state persisted between launches.
///
/// `last_check_unix` and `last_manifest_etag` drive the daily-check loop's
/// elapsed gate and `If-None-Match` short-circuit; `consecutive_failures` is
/// what lengthens its re-arm against flaky-network thrash. `skipped_release` is
/// set *only* by the "Skip this version" affordance — dismissing the toast
/// deliberately doesn't, since a dismissed toast returns next launch while a
/// skipped version stays suppressed until a strictly-newer one lands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateFlags {
    pub auto_check_enabled: bool,
    pub last_check_unix: i64,
    pub last_known_release: String,
    pub skipped_release: String,
    pub last_manifest_etag: String,
    pub consecutive_failures: u8,
}

impl UpdateFlags {
    /// A manifest fetch that came back. `latest_version` is `None` on a `304`, where the cached
    /// value is still the most recent thing seen and overwriting it would lose it.
    pub fn record_success(
        &mut self,
        now_unix: i64,
        latest_version: Option<String>,
        etag: Option<String>,
    ) {
        self.last_check_unix = now_unix;
        self.consecutive_failures = 0;
        if let Some(version) = latest_version {
            self.last_known_release = version;
        }
        if let Some(tag) = etag {
            self.last_manifest_etag = tag;
        }
    }

    /// A fetch that didn't. Advancing `last_check_unix` is what the counter depends on: the daily
    /// loop's 24h gate reads it, and a failure that left it alone would re-fire every iteration
    /// rather than backing off.
    pub fn record_failure(&mut self, now_unix: i64) {
        self.last_check_unix = now_unix;
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
    }
}

impl Default for UpdateFlags {
    fn default() -> Self {
        Self {
            auto_check_enabled: true,
            last_check_unix: 0,
            last_known_release: String::new(),
            skipped_release: String::new(),
            last_manifest_etag: String::new(),
            consecutive_failures: 0,
        }
    }
}

/// What the diagnostics surfaces record.
///
/// `verbose_logging` is a debugging mode, and against a fixed rotation budget
/// leaving it on costs a reporter the older history. Persisted rather than
/// session-scoped so `logging::install` can start a boot at it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiagnosticsFlags {
    pub verbose_logging: bool,
}

/// What the one-time Ko-fi prompt remembers. `launch_count` stops advancing
/// once `support_prompt_seen` is set, so a settled install stops rewriting
/// `settings.json` at boot rather than counting forever.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SupportFlags {
    pub launch_count: u32,
    pub support_prompt_seen: bool,
}

/// The onboarding revision this install has been shown, `0` meaning never.
///
/// A revision rather than a bool so a later feature that belongs in the welcome card can bump
/// [`ONBOARDING_VERSION`] and reach installs that already ran the flow, instead of owing a
/// separate what's-new surface. While the constant never moves this behaves exactly as a bool
/// would have.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OnboardingFlags {
    pub onboarding_version: u32,
}

/// The revision [`OnboardingFlags::onboarding_version`] lands on once the card has been seen.
pub const ONBOARDING_VERSION: u32 = 1;

impl OnboardingFlags {
    /// Whether the welcome card is owed on this launch.
    pub fn needs_onboarding(&self) -> bool {
        self.onboarding_version < ONBOARDING_VERSION
    }
}
