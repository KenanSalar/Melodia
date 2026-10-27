//! Discord Rich Presence: the flag shadow, the connection-status `watch`, and the [`ipc`]
//! worker, a `std::thread` over a blocking transport, alive only while the feature is on. Unlike
//! scrobbling it persists nothing, the application id being public and compiled in.

mod artwork;
pub mod ipc;
pub mod model;
pub mod payload;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread::JoinHandle;

use parking_lot::{Mutex, RwLock};
use tokio::sync::watch;

use ipc::Command;
use melodia_core::entities::integrations::DiscordFlags;
use melodia_core::error::describe;
use model::Presence;

/// **Public**, shipping in every payload, so unlike the Last.fm keys it needs no secret. A fork
/// overrides it at build time for its own application.
const DISCORD_APP_ID: &str = match non_empty_env(option_env!("MELODIA_DISCORD_APP_ID")) {
    Some(id) => id,
    // The registered "Melodia" application.
    None => "1530168119954374666",
};

/// An empty override would ship a client id Discord refuses; as `lastfm::non_empty_env`.
const fn non_empty_env(value: Option<&str>) -> Option<&str> {
    match value {
        Some(s) if s.is_empty() => None,
        other => other,
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DiscordStatus {
    pub enabled: bool,
    pub connected: bool,
}

/// Apart from the service, so the worker publishes `connected` without an `Arc` cycle through it.
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

    pub(crate) fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Relaxed);
        self.publish();
    }
}

/// The IPC worker's command channel and the thread it runs on.
#[derive(Default)]
struct WorkerSlot {
    /// `None` while the feature is off, which costs no thread and no socket probing.
    commands: Option<mpsc::Sender<Command>>,
    /// The running worker, or the last one turned off, which may still be closing its socket.
    thread: Option<JoinHandle<()>>,
}

/// What a worker opens its socket to Discord with, `None` where no client is listening.
type Connector = Arc<dyn Fn() -> Option<ipc::Connection> + Send + Sync>;

/// Held as `Arc<DiscordPresenceService>` on the app's `AppState`.
pub struct DiscordPresenceService {
    runtime: RwLock<DiscordFlags>,
    status: Arc<StatusCell>,
    worker: Mutex<WorkerSlot>,
    /// [`ipc::connect`] in the app, a stand-in Discord in the worker's suite.
    connect: Connector,
    /// `AppState`'s lazy client, for the cover lookup.
    http: Arc<OnceLock<reqwest::Client>>,
    artwork_cache: artwork::ArtworkCache,
}

impl DiscordPresenceService {
    pub fn init(flags: &DiscordFlags, http: Arc<OnceLock<reqwest::Client>>) -> Self {
        let (tx, _) =
            watch::channel(DiscordStatus { enabled: flags.discord_rpc_enabled, connected: false });
        let status = Arc::new(StatusCell {
            enabled: AtomicBool::new(flags.discord_rpc_enabled),
            connected: AtomicBool::new(false),
            tx,
        });
        Self {
            runtime: RwLock::new(flags.clone()),
            status,
            worker: Mutex::new(WorkerSlot::default()),
            connect: Arc::new(ipc::connect),
            http,
            artwork_cache: artwork::new_cache(),
        }
    }

    /// Opens every worker's socket with `connect` rather than probing for the client. Consuming,
    /// so a seated `Arc` can't reach it.
    #[cfg(all(test, unix))]
    #[must_use]
    pub(crate) fn with_connector(mut self, connect: Connector) -> Self {
        self.connect = connect;
        self
    }

    /// The cheap gate the task checks before touching the model.
    pub fn armed(&self) -> bool {
        self.runtime.read().discord_rpc_enabled
    }

    pub fn flags(&self) -> DiscordFlags {
        self.runtime.read().clone()
    }

    /// Cache first. The task asks only on a track change; a pause or a seek reuses its last
    /// answer.
    pub async fn resolve_artwork(&self, artist: &str, album: &str) -> Option<String> {
        let client = self.http.get_or_init(melodia_net::services::net::build_http_client).clone();
        artwork::resolve_album_cover(&client, &self.artwork_cache, artist, album).await
    }

    pub fn status(&self) -> DiscordStatus {
        self.status.snapshot()
    }

    pub fn subscribe_status(&self) -> watch::Receiver<DiscordStatus> {
        self.status.tx.subscribe()
    }

    /// Mirrors flags a setter has already persisted, starting or stopping the worker with them.
    pub fn set_flags(&self, flags: DiscordFlags) {
        let enabled = flags.discord_rpc_enabled;
        *self.runtime.write() = flags;
        self.status.set_enabled(enabled);
        if enabled {
            self.arm();
        } else {
            self.disarm();
        }
    }

    /// Called at boot, so a saved "on" connects and shows its status before anything plays.
    pub fn start_if_enabled(&self) {
        if self.armed() {
            self.arm();
        }
    }

    /// A no-op when disabled.
    pub fn apply(&self, presence: Presence) {
        self.ensure_worker();
        self.send(Command::Apply(presence));
    }

    /// Clears the card while staying connected.
    pub fn clear(&self) {
        self.send(Command::Clear);
    }

    fn arm(&self) {
        self.ensure_worker();
        self.send(Command::Enable);
    }

    /// Clear the card and close the socket, then drop the channel, which ends the worker once it
    /// has done both. A later enable spawns a fresh one.
    fn disarm(&self) {
        if let Some(commands) = self.worker.lock().commands.take() {
            let _ = commands.send(Command::Disable);
        }
    }

    /// Spawn the worker if the feature is on and none is running. Never joined at shutdown: at
    /// quit the socket closes under `process::exit(0)`, which makes Discord drop the card.
    fn ensure_worker(&self) {
        let mut worker = self.worker.lock();
        // Read under the slot's lock, so an `apply` racing a disable can't spawn a worker
        // after `disarm` has let the last one go.
        if worker.commands.is_some() || !self.armed() {
            return;
        }
        let (tx, rx) = mpsc::channel::<Command>();
        let status = Arc::clone(&self.status);
        let connect = Arc::clone(&self.connect);
        let previous = worker.thread.take();
        let spawned = std::thread::Builder::new().name("melodia-discord".into()).spawn(move || {
            // One turned off just before may still be closing its socket. Waiting for it keeps
            // one connection to Discord, and one writer of `connected`, at a time.
            if let Some(previous) = previous {
                let _ = previous.join();
            }
            ipc::run_worker(&rx, &status, &*connect);
        });
        match spawned {
            Ok(thread) => {
                worker.commands = Some(tx);
                worker.thread = Some(thread);
            }
            Err(e) => log::warn!("discord: could not spawn worker thread: {}", describe(&e)),
        }
    }

    fn send(&self, cmd: Command) {
        if let Some(commands) = self.worker.lock().commands.as_ref() {
            let _ = commands.send(cmd);
        }
    }
}

#[cfg(test)]
#[path = "tests/mod_tests.rs"]
mod tests;
