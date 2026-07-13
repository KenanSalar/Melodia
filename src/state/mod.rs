use std::sync::{Arc, OnceLock};

use souvlaki::MediaControlEvent;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub mod contexts;
pub use contexts::PlaybackContext;

use crate::config::Paths;
use crate::database::{self, DbPool};
use crate::error::{AppError, AppResult};
use crate::media::{
    artwork::CoverCache,
    self_writes::SelfWrites,
    watcher::{FileEvent, FolderWatcher},
};
use crate::player::event_sink::{MediaControlsSync, PlayerSinks};
use crate::player::rodio_backend::RodioPlayer;
use crate::player::state::{
    PlayerStateHandle, PlayerViewModelLight, PositionTick, QueueViewModel, lock_state,
};
use crate::services::{
    always_on_top::{self, AlwaysOnTopCapability},
    media_controls::{self, MediaControlsHandle},
    search_history::SearchHistoryState,
    settings,
};

/// One scan-progress sample published while a folder scan is running. `None`
/// on the channel means "no scan in progress" — the UI uses that to clear the
/// progress bar.
#[derive(Debug, Clone)]
pub struct ScanProgressTick {
    pub folder_id: i64,
    pub scanned: u32,
    pub total: u32,
    pub current_file: String,
}

#[derive(Clone)]
pub struct AppState {
    pub paths: Arc<Paths>,
    pub runtime: Handle,
    pub db: DbPool,
    pub cover_cache: CoverCache,
    pub player_state: Arc<PlayerStateHandle>,
    pub rodio: Arc<RodioPlayer>,
    pub sinks: Arc<PlayerSinks>,
    pub position_tx: watch::Sender<Option<PositionTick>>,
    /// Bumped whenever the track library is mutated by a scan or watcher
    /// event. UI subscribers re-fetch the Tracks model on each tick.
    pub library_changed_tx: watch::Sender<u64>,
    /// Bumped after every play-count flush. Split from `library_changed_tx`
    /// so the per-song flush (every track completion) doesn't imply
    /// "library structure changed" — the only views that depend on
    /// play-driven data subscribe here: Favorites (hero mosaic + Most Played
    /// strip, ranked by `play_count`) and Recently-Played (ordered by
    /// `last_played`, written on the same flush). Everything else
    /// (Tracks/Browse refreshers, `queue_prune`, the folder list) stays on
    /// `library_changed_tx` and no longer fires per played song.
    pub stats_changed_tx: watch::Sender<u64>,
    /// Bumped whenever the watcher reports a kernel-queue overflow (notify
    /// `Flag::Rescan`). A UI-thread subscriber pushes a transient
    /// "library re-syncing" toast through the notifications stack so the
    /// user knows why Tracks/Browse paused refreshing while the
    /// reconcile runs. Coalescing semantics: a burst of overflows that
    /// land in the same `watch` slot only paints one toast — which is
    /// also what `RECONCILE_IN_FLIGHT` does to the reconcile spawn
    /// itself.
    pub rescan_notice_tx: watch::Sender<u64>,
    /// Live progress for the folder scan in flight. `None` means idle; the UI
    /// uses that to hide the progress bar in the Library settings section.
    pub scan_progress_tx: watch::Sender<Option<ScanProgressTick>>,
    pub watcher: Arc<parking_lot::Mutex<FolderWatcher>>,
    /// Files this process wrote itself (tag edits), so the watcher can drop the
    /// `Modified` events its own writes generate instead of paying for a full
    /// re-ingest of a file it just retagged. See [`SelfWrites`].
    pub self_writes: Arc<SelfWrites>,
    pub always_on_top: AlwaysOnTopCapability,
    pub search_history: Arc<SearchHistoryState>,
    pub media_controls: Option<Arc<MediaControlsHandle>>,
    /// Shared `reqwest::Client`, built lazily on first use via
    /// [`AppState::http_client`]. Only the updater and the post-scan Deezer
    /// artist-image fetch ever need it, so deferring construction keeps the
    /// rustls TLS stack and connection pool off the boot/idle footprint.
    /// `Arc<OnceLock<…>>` so every cloned `AppState` shares one client and one
    /// initialization (`reqwest::Client` itself wraps an internal pool in
    /// `Arc`). Future remote calls should reuse the accessor rather than
    /// constructing a new client per call.
    http_client: Arc<OnceLock<reqwest::Client>>,
    /// Tracks every spawned background task so shutdown can wait for them
    /// to finish their current write before the runtime is dropped.
    pub task_tracker: TaskTracker,
    /// Broadcast cancel signal — long-running loops listen via `cancelled().await`.
    pub shutdown_token: CancellationToken,
    /// Browser-style back/forward navigation history. In-memory only —
    /// reset on each launch. See `src/ui/nav_history.rs`.
    pub nav_history: Arc<parking_lot::Mutex<crate::ui::nav_history::NavHistory>>,
    /// Per-section `*Ui` handle registry the nav-history replay reads
    /// when it needs to invoke an `open_*` future. Populated by each
    /// `wire_*` once its `Arc<*Ui>` exists. See `src/ui/nav_history.rs`.
    pub ui_handles: Arc<crate::ui::nav_history::UiHandles>,
}

/// Receivers handed back from `AppState::init` for sub-phase I to consume.
/// Holding them on `AppState` would force a `Mutex<Option<...>>` shape that
/// Phase 1 doesn't need; returning them keeps the struct stable.
pub struct StartupChannels {
    pub media_control_rx: Option<mpsc::Receiver<MediaControlEvent>>,
    pub file_event_rx: mpsc::Receiver<FileEvent>,
}

impl AppState {
    pub async fn init(paths: Paths, runtime: Handle) -> AppResult<(Self, StartupChannels)> {
        // Open the output device with our own error callback instead of
        // `open_default_sink()`'s default — rodio's default handler `eprintln!`s
        // straight to stderr (it only routes through a logger when its
        // `tracing` feature is on, which we don't enable). A transient ALSA
        // xrun at device-open time under a CPU-heavy first-launch scan is
        // benign and self-recovering; routing it through `log` keeps it
        // filterable and consistent, while a genuine device failure mid-session
        // still surfaces as a `warn`.
        let mut speakers = rodio::DeviceSinkBuilder::from_default_device()
            .map_err(|e| AppError::Player(format!("Failed to open audio output device: {e}")))?
            .with_error_callback(|err| log::warn!("audio stream error: {err}"))
            .open_stream()
            .map_err(|e| AppError::Player(format!("Failed to open audio output device: {e}")))?;
        speakers.log_on_drop(false);
        let speakers: &'static rodio::MixerDeviceSink = Box::leak(Box::new(speakers));
        // The runtime handle is only used to schedule the deferred half of a
        // faded pause / stop (arm the ramp now, pause the decks once it lands).
        let rodio = Arc::new(RodioPlayer::new(speakers.mixer(), runtime.clone()));

        let db = database::init_database(&paths).await?;

        let settings = settings::read_settings(&paths).unwrap_or_else(|e| {
            log::warn!("Failed to read settings on startup: {e}; using defaults");
            settings::SettingsData::default()
        });

        let player_state = Arc::new(PlayerStateHandle::default());
        {
            let mut s = lock_state(&player_state);
            s.volume = settings.volume.min(crate::player::state::MAX_VOLUME);
            s.is_muted = settings.playback.is_muted;
            s.pre_mute_volume = s.volume;
            s.gapless_enabled = settings.playback.gapless_playback;
            s.playback_speed = settings.playback.playback_speed.clamp(
                crate::player::state::MIN_SPEED,
                crate::player::state::MAX_SPEED,
            );
            let vol = s.effective_volume();
            let speed = s.playback_speed;
            drop(s);
            rodio.set_volume(vol);
            rodio.set_speed(speed);
        }

        hydrate_audio_dsp(&rodio, &settings);

        let cover_cache: CoverCache = crate::media::artwork::new_cover_cache();

        let (vm_tx, _) = watch::channel::<Option<PlayerViewModelLight>>(None);
        let (q_tx, _) = watch::channel::<Option<QueueViewModel>>(None);
        let (position_tx, _) = watch::channel::<Option<PositionTick>>(None);
        let (library_changed_tx, _) = watch::channel::<u64>(0);
        let (stats_changed_tx, _) = watch::channel::<u64>(0);
        let (rescan_notice_tx, _) = watch::channel::<u64>(0);
        let (scan_progress_tx, _) = watch::channel::<Option<ScanProgressTick>>(None);

        let (mc_handle, mc_rx) = media_controls::init_media_controls();
        let mc_handle = Arc::new(mc_handle);
        let media_controls_sync: Arc<dyn MediaControlsSync> = mc_handle.clone();

        let sinks = Arc::new(PlayerSinks {
            view_model: vm_tx,
            queue: q_tx,
            media_controls: Some(media_controls_sync),
        });

        let (file_tx, file_rx) = mpsc::channel::<FileEvent>(256);
        let watcher = Arc::new(parking_lot::Mutex::new(FolderWatcher::new(file_tx)));

        let always_on_top_capability = always_on_top::detect_capability();
        log::info!(
            "Always-on-top: method={:?}, supported={}, native_decorations={}",
            always_on_top_capability.method,
            always_on_top_capability.supported,
            always_on_top_capability.use_native_decorations
        );

        let search_history = Arc::new(SearchHistoryState::init(&paths).await);

        let state = Self {
            paths: Arc::new(paths),
            runtime,
            db,
            cover_cache,
            player_state,
            rodio,
            sinks,
            position_tx,
            library_changed_tx,
            stats_changed_tx,
            rescan_notice_tx,
            scan_progress_tx,
            watcher,
            self_writes: Arc::new(SelfWrites::default()),
            always_on_top: always_on_top_capability,
            search_history,
            media_controls: Some(mc_handle),
            http_client: Arc::new(OnceLock::new()),
            task_tracker: TaskTracker::new(),
            shutdown_token: CancellationToken::new(),
            nav_history: Arc::new(parking_lot::Mutex::new(
                crate::ui::nav_history::NavHistory::new(),
            )),
            ui_handles: Arc::new(crate::ui::nav_history::UiHandles::default()),
        };

        let channels = StartupChannels {
            media_control_rx: Some(mc_rx),
            file_event_rx: file_rx,
        };

        Ok((state, channels))
    }

    /// Persist a settings mutation on the blocking pool, logging (never
    /// surfacing) a failure under `label`. This is the write half of the
    /// two-phase "apply live, then persist" shape used by the EQ / `ReplayGain`
    /// / playback-settings installers: the caller has already applied the value
    /// to the running player, so a failed disk write must not undo it — the
    /// warn is the only report.
    pub fn persist_blocking(
        &self,
        label: &'static str,
        f: impl FnOnce(&AppState) -> Result<(), AppError> + Send + 'static,
    ) {
        let s = self.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(e) = f(&s) {
                log::warn!("{label}: {e}");
            }
        });
    }

    /// The shared `reqwest::Client`, built on first call and reused for the
    /// process lifetime. Construction is deferred out of `init` so the rustls
    /// TLS stack and connection pool never load at boot/idle — only the
    /// updater and the post-scan Deezer artist-image fetch pull them in.
    /// reqwest's connection pool lives on the client, so callers reuse this
    /// accessor rather than constructing a new client per request.
    ///
    /// Timeouts: reqwest's default is "no timeout", which means a wedged CDN
    /// socket parks the streaming download future in `bytes_stream().next()`
    /// indefinitely (the cancellation token never fires because the future is
    /// parked in foreign code). The fix is a per-read deadline rather than a
    /// whole-body deadline: a legitimately-slow 200 MB download must be allowed
    /// to take minutes, but no individual read should sit silent for 60 s.
    /// `read_timeout` resets on every byte received, so it only trips when the
    /// socket is genuinely dead.
    ///
    /// User-Agent: GitHub's API guidance asks every consumer to set a
    /// descriptive UA. Default `reqwest/0.13` is tolerated but ours is more
    /// useful in server logs when something goes wrong.
    ///
    /// Pool cap: the updater only talks to api.github.com +
    /// objects.githubusercontent.com; 4 idle conns per host is more than enough
    /// and bounds memory on a long-running process. Default is unbounded.
    ///
    /// Build is documented infallible for these options; the fallback is
    /// paranoia. If it ever fires we'd lose the timeouts, which is why it's
    /// logged.
    pub fn http_client(&self) -> &reqwest::Client {
        self.http_client.get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .read_timeout(std::time::Duration::from_mins(1))
                .pool_max_idle_per_host(4)
                .user_agent(concat!("Melodia/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_else(|e| {
                    log::warn!(
                        "reqwest::Client::builder().build() failed unexpectedly ({e}); falling \
                         back to default client without timeouts — downloads may hang on a wedged \
                         socket"
                    );
                    reqwest::Client::new()
                })
        })
    }
}

/// Seed the Rodio backend's lock-free cells (graphic EQ, `ReplayGain`,
/// crossfade) from persisted settings before playback starts, so the first
/// track is already processed when any of them is enabled. All three live on
/// the Rodio backend (not `PlayerState`). Ordering is deliberate: values first,
/// `enabled` last, so the enable's generation bump publishes a fully-seeded
/// state to the audio thread.
fn hydrate_audio_dsp(rodio: &RodioPlayer, settings: &settings::SettingsData) {
    // `set_eq_gains` clamps and length-normalises the (possibly hand-edited)
    // gain list; the EQ ships off by default.
    rodio.set_eq_gains(&settings.equalizer.eq_band_gains);
    rodio.set_eq_preamp(settings.equalizer.eq_preamp);
    rodio.set_eq_enabled(settings.equalizer.eq_enabled);

    // ReplayGain master state seeds the same way — per-track gain is baked per
    // source at play time. Ships off by default; the mode string falls back to
    // Album on an unknown value.
    rodio.set_replaygain_preamp(settings.replaygain.rg_preamp);
    rodio.set_replaygain_mode(crate::player::replaygain::RgMode::from_settings_str(
        &settings.replaygain.rg_mode,
    ));
    rodio.set_replaygain_prevent_clipping(settings.replaygain.rg_prevent_clipping);
    rodio.set_replaygain_enabled(settings.replaygain.rg_enabled);

    // Crossfade ships off; `set_crossfade_duration_ms` clamps a hand-edited value.
    rodio.set_crossfade_duration_ms(settings.crossfade.crossfade_duration_ms);
    rodio.set_crossfade_manual(settings.crossfade.crossfade_manual);
    rodio.set_crossfade_skip_same_album(settings.crossfade.crossfade_skip_same_album);
    rodio.set_crossfade_fade_on_pause(settings.crossfade.crossfade_fade_on_pause);
    rodio.set_crossfade_enabled(settings.crossfade.crossfade_enabled);
}
