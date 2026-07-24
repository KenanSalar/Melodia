//! Discord Rich Presence service: owns the enable-flags shadow and the
//! connection-status watch, and lazily spawns the blocking IPC worker thread.
//!
//! Mirrors `services::scrobble` in spirit — an enabled shadow read synchronously,
//! a `watch<DiscordStatus>` the settings UI subscribes to — but owns **no**
//! on-disk state (the application id is a compile-time constant, not a secret to
//! persist) and drives a blocking transport rather than HTTP, so its worker is a
//! detached `std::thread` + `std::sync::mpsc` channel rather than a tokio task.
//! See [`ipc`] for the transport and [`model`] for the pure projection.

pub mod ipc;
pub mod model;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

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
    // TODO(phase 0): replace with the registered "Melodia" application id. A
    // placeholder still compiles/tests; only a real handshake needs the true id.
    None => "0000000000000000000",
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
}

impl DiscordPresenceService {
    /// Seed the shadow from `settings.json`. Infallible — nothing to load.
    pub fn init(flags: &DiscordFlags) -> Self {
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
        }
    }

    /// Whether presence pushing is on — the cheap synchronous gate the task
    /// checks before touching the model.
    pub fn armed(&self) -> bool {
        self.runtime.read().discord_rpc_enabled
    }

    /// A snapshot of the flags for the task (hide-while-paused, and later artwork).
    pub fn flags(&self) -> DiscordFlags {
        self.runtime.read().clone()
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
            self.ensure_worker();
            self.send(Command::Enable);
        } else {
            self.send(Command::Disable);
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
