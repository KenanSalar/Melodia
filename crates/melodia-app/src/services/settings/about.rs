//! The flag structs behind the Settings ▸ About tab: updates, diagnostics, and what the welcome card
//! and the support prompt remember.

use serde::{Deserialize, Serialize};

/// Auto-updater state persisted between launches.
///
/// `last_known_release` and `last_manifest_etag` describe one manifest, the last
/// one read in full, and are only ever replaced together. `last_check_unix` drives
/// the daily loop's elapsed gate and `consecutive_failures` lengthens its re-arm
/// against flaky-network thrash. `skipped_release` is set *only* by the "Skip this
/// version" affordance. Dismissing the toast deliberately doesn't, since a dismissed
/// update returns at the next daily check while a skipped one stays quiet until a
/// strictly newer release lands.
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
    /// A manifest read in full. A response without a tag clears the old one rather than keeping
    /// it, since a tag left beside another manifest's version would let a `304` vouch for a
    /// manifest this file no longer describes.
    pub fn record_fetch(&mut self, now_unix: i64, version: String, etag: Option<String>) {
        self.record_reachable(now_unix);
        self.last_known_release = version;
        self.last_manifest_etag = etag.unwrap_or_default();
    }

    /// A `304`: the stored manifest is still the published one.
    pub fn record_not_modified(&mut self, now_unix: i64) {
        self.record_reachable(now_unix);
    }

    fn record_reachable(&mut self, now_unix: i64) {
        self.last_check_unix = now_unix;
        self.consecutive_failures = 0;
    }

    /// Clears a skip a check judged spent, unless the user has picked another since that check
    /// read it.
    pub fn forget_skip(&mut self, spent: &str) {
        if self.skipped_release == spent {
            self.skipped_release.clear();
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
