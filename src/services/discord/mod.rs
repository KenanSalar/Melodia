//! Discord Rich Presence service: owns the enable-flags shadow and the
//! connection-status watch, and lazily spawns the blocking IPC worker thread.
//!
//! Mirrors `services::scrobble` in spirit — an enabled shadow read synchronously,
//! a `watch<DiscordStatus>` the settings UI subscribes to — but owns **no**
//! on-disk state (the application id is a compile-time constant, not a secret to
//! persist) and drives a blocking transport rather than HTTP, so its worker is a
//! detached `std::thread` + `std::sync::mpsc` channel rather than a tokio task.
//! See [`ipc`] for the transport and [`model`] for the pure projection.

mod artwork;
pub mod ipc;
pub mod model;
pub mod payload;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};

use parking_lot::{Mutex, RwLock};
use tokio::sync::watch;

use crate::services::settings::DiscordFlags;
use ipc::Command;
use model::Presence;

/// The Discord application id — **public** (it ships in every presence payload),
/// so unlike the Last.fm keys it needs no CI secret. Hardcoded, with an
/// `option_env!` override for a fork building against its own application.
const DISCORD_APP_ID: &str = match non_empty_env(option_env!("MELODIA_DISCORD_APP_ID")) {
    Some(id) => id,
    // The registered "Melodia" application id — public, ships in every payload.
    None => "1530168119954374666",
};

/// Treat a present-but-empty compile-time env var as absent — a CI env that
/// substitutes `""` would otherwise ship an empty client id no Discord client
/// accepts. Mirrors `scrobble::providers::lastfm::non_empty_env`.
const fn non_empty_env(value: Option<&str>) -> Option<&str> {
    match value {
        Some(s) if s.is_empty() => None,
        other => other,
    }
}

/// Cheap connection + enable snapshot for the settings UI — the payload of the
/// service's `watch<DiscordStatus>`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiscordStatus {
    pub enabled: bool,
    pub connected: bool,
}

/// Shared status cell held by both the service (writes `enabled`) and the worker
/// thread (writes `connected`). Split out of the service so the worker can
/// publish connection changes without an `Arc<DiscordPresenceService>` cycle.
pub(crate) struct StatusCell {
    enabled: AtomicBool,
    connected: AtomicBool,
    tx: watch::Sender<DiscordStatus>,
}

impl StatusCell {
    fn snapshot(&self) -> DiscordStatus {
        DiscordStatus {
            enabled: self.enabled.load(Ordering::Relaxed),
            connected: self.connected.load(Ordering::Relaxed),
        }
    }

    fn publish(&self) {
        self.tx.send_replace(self.snapshot());
    }

    fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.publish();
    }

    /// Called from the worker thread on (dis)connect.
    pub(crate) fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Relaxed);
        self.publish();
    }
}

/// Held as `Arc<DiscordPresenceService>` on [`crate::state::AppState`].
pub struct DiscordPresenceService {
    /// The enable flags, read synchronously by the detector task (`armed`).
    runtime: RwLock<DiscordFlags>,
    status: Arc<StatusCell>,
    /// Command channel to the worker — `None` until the feature is first enabled
    /// (a user who never turns it on pays for no thread, no socket probing).
    worker: Mutex<Option<mpsc::Sender<Command>>>,
    /// Shared lazy HTTP client (same `OnceLock` as `AppState`/scrobble) for the
    /// album-cover lookup — one connection pool built on first actual request.
    http: Arc<OnceLock<reqwest::Client>>,
    /// Cover-URL cache, keyed case-insensitively on `(artist, album)`.
    artwork_cache: artwork::ArtworkCache,
}

impl DiscordPresenceService {
    /// Seed the shadow from `settings.json`. Infallible — nothing to load.
    pub fn init(flags: &DiscordFlags, http: Arc<OnceLock<reqwest::Client>>) -> Self {
        let (tx, _) = watch::channel(DiscordStatus {
            enabled: flags.discord_rpc_enabled,
            connected: false,
        });
        let status = Arc::new(StatusCell {
            enabled: AtomicBool::new(flags.discord_rpc_enabled),
            connected: AtomicBool::new(false),
            tx,
        });
        Self {
            runtime: RwLock::new(flags.clone()),
            status,
            worker: Mutex::new(None),
            http,
            artwork_cache: artwork::new_cache(),
        }
    }

    /// Whether presence pushing is on — the cheap synchronous gate the task
    /// checks before touching the model.
    pub fn armed(&self) -> bool {
        self.runtime.read().discord_rpc_enabled
    }

    /// A snapshot of the flags for the task (hide-while-paused, artwork).
    pub fn flags(&self) -> DiscordFlags {
        self.runtime.read().clone()
    }

    /// Resolve an album cover URL for the presence card's `large_image`, cache
    /// first. Driven by the detector task on a track change only.
    pub async fn resolve_artwork(&self, artist: &str, album: &str) -> Option<String> {
        let client = self
            .http
            .get_or_init(crate::services::build_http_client)
            .clone();
        artwork::resolve_album_cover(&client, &self.artwork_cache, artist, album).await
    }

    pub fn status(&self) -> DiscordStatus {
        self.status.snapshot()
    }

    pub fn subscribe_status(&self) -> watch::Receiver<DiscordStatus> {
        self.status.tx.subscribe()
    }

    /// Mirror the enable flags into the shadow after a setter has persisted them,
    /// then start or stop the worker so the toggle takes effect immediately.
    pub fn set_flags(&self, flags: DiscordFlags) {
        let enabled = flags.discord_rpc_enabled;
        *self.runtime.write() = flags;
        self.status.set_enabled(enabled);
        if enabled {
            self.arm();
        } else {
            self.send(Command::Disable);
        }
    }

    /// Start the IPC worker if the feature is already enabled — called once at
    /// boot so a persisted `discord_rpc_enabled: true` connects (and shows its
    /// status) while idle, instead of waiting for the first `apply` (which only
    /// fires once a track is playing) or a toggle. Idempotent: a no-op when
    /// disabled, and `ensure_worker` skips an already-spawned worker.
    pub fn start_if_enabled(&self) {
        if self.armed() {
            self.arm();
        }
    }

    /// Push a presence card. No-op when disabled.
    pub fn apply(&self, presence: Presence) {
        if !self.armed() {
            return;
        }
        self.ensure_worker();
        self.send(Command::Apply(presence));
    }

    /// Clear the card (playback stopped) while staying connected.
    pub fn clear(&self) {
        self.send(Command::Clear);
    }

    /// Spawn the worker if needed and tell it the feature is on — the shared tail
    /// of `set_flags` (toggle) and `start_if_enabled` (boot).
    fn arm(&self) {
        self.ensure_worker();
        self.send(Command::Enable);
    }

    /// Lazily spawn the detached worker thread on first use, storing its sender.
    /// Not tracked for shutdown — at quit the socket closes under
    /// `process::exit(0)`, which makes Discord drop the card.
    fn ensure_worker(&self) {
        let mut worker = self.worker.lock();
        if worker.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel::<Command>();
        let status = Arc::clone(&self.status);
        match std::thread::Builder::new()
            .name("melodia-discord".into())
            .spawn(move || ipc::run_worker(&rx, &status))
        {
            Ok(_handle) => *worker = Some(tx),
            Err(e) => log::warn!("discord: could not spawn worker thread: {e}"),
        }
    }

    /// Best-effort send to the worker; a never-spawned or gone worker just means
    /// nothing to push.
    fn send(&self, cmd: Command) {
        if let Some(tx) = self.worker.lock().as_ref() {
            let _ = tx.send(cmd);
        }
    }
}
