use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::player::stream_health::{self, AudioStreamHealth};
use crate::services::{
    always_on_top::{self, AlwaysOnTopCapability},
    discord::DiscordPresenceService,
    media_controls::{self, MediaControlsHandle},
    scrobble::ScrobbleService,
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
    /// Fault counters the output device's error callback writes into, on the
    /// audio thread. Drained by `tasks::audio_health` and read nowhere else.
    pub audio_health: Arc<AudioStreamHealth>,
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
    /// Bumped by `tasks::audio_health` when the output device goes away. A
    /// UI-thread subscriber pushes a sticky warning toast — nothing else
    /// notices, so playback runs on with the position ticking and no sound.
    /// Coalesces like `rescan_notice_tx`: a burst paints one toast.
    pub audio_device_lost_tx: watch::Sender<u64>,
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
    /// Scrobbling service: credential/enabled shadow + durable offline queue.
    /// Loaded at boot; the detector/submitter tasks that drive it wire in later.
    pub scrobble: Arc<ScrobbleService>,
    /// Discord Rich Presence: enable-flags shadow + connection-status watch,
    /// lazily spawning a blocking IPC worker thread. Stateless (no on-disk
    /// state); the detector task in `tasks::discord_presence` drives it.
    pub discord: Arc<DiscordPresenceService>,
    /// The Radio master switch, mirrored off `settings.json` at boot. A shadow
    /// rather than a read at the decision point because every reader is on a
    /// path a file read has no business being on: `library::radio`'s guard runs
    /// on a tokio worker per directory call, and `boot::ui_setup` asks before
    /// the window is shown. [`AppState::set_radio_enabled`] owns when it moves.
    radio_enabled: Arc<AtomicBool>,
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

/// Receivers handed back from `AppState::init` for `boot::tasks` to consume.
/// Holding them on `AppState` would force a `Mutex<Option<…>>` shape nothing
/// needs; returning them keeps the struct stable.
pub struct StartupChannels {
    pub media_control_rx: Option<mpsc::Receiver<MediaControlEvent>>,
    pub file_event_rx: mpsc::Receiver<FileEvent>,
}

impl AppState {
    pub async fn init(paths: Paths, runtime: Handle) -> AppResult<(Self, StartupChannels)> {
        // Our own error callback rather than rodio's, which `eprintln!`s to
        // stderr unless its `tracing` feature is on. It records into counters
        // instead of logging because cpal calls it on the output worker thread;
        // `player::stream_health` argues that.
        //
        // `open_sink_or_fallback` rather than `open_stream`: the latter turns
        // any config the device rejects into a boot that stops with no audio at
        // all, where this first retries every config the device supports.
        let audio_health = Arc::new(AudioStreamHealth::default());
        let builder = rodio::DeviceSinkBuilder::from_default_device()
            .map_err(|e| AppError::Player(format!("Failed to open audio output device: {e}")))?
            .with_error_callback(stream_health::error_callback(audio_health.clone()));
        let mut speakers = builder
            .open_sink_or_fallback()
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
            s.playback_speed = settings
                .playback
                .playback_speed
                .clamp(crate::player::state::MIN_SPEED, crate::player::state::MAX_SPEED);
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
        let (audio_device_lost_tx, _) = watch::channel::<u64>(0);
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
        // Shared lazy client: one `OnceLock` seeds both `http_client()` and the
        // scrobble service, so the app has a single connection pool built on
        // first actual request.
        let http_client = Arc::new(OnceLock::new());
        let scrobble =
            Arc::new(ScrobbleService::init(&paths, &settings.scrobble, http_client.clone()));
        // Persists nothing (the application id is a compile-time constant, not a
        // secret), so unlike scrobble it needs no `&paths` — but it shares the
        // one `http_client` pool for the album-cover lookup.
        let discord =
            Arc::new(DiscordPresenceService::init(&settings.discord, http_client.clone()));

        let state = Self {
            paths: Arc::new(paths),
            runtime,
            db,
            cover_cache,
            player_state,
            rodio,
            audio_health,
            sinks,
            position_tx,
            library_changed_tx,
            stats_changed_tx,
            rescan_notice_tx,
            audio_device_lost_tx,
            scan_progress_tx,
            watcher,
            self_writes: Arc::new(SelfWrites::default()),
            always_on_top: always_on_top_capability,
            search_history,
            scrobble,
            discord,
            radio_enabled: Arc::new(AtomicBool::new(settings.radio.radio_enabled)),
            media_controls: Some(mc_handle),
            http_client,
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
        // `label` already names the setting, so one line here covers every
        // caller — including the three settings-row helpers that pass one
        // through. On the way in, so a write that hangs still says what it was.
        log::debug!("settings: {label}");
        let s = self.clone();
        self.runtime.spawn_blocking(move || {
            if let Err(e) = f(&s) {
                log::warn!("{label}: {e}");
            }
        });
    }

    /// The shared `reqwest::Client`, built on first call and reused for the
    /// process lifetime. Construction is deferred out of `init` (see
    /// [`crate::services::build_http_client`]) so the rustls TLS stack and
    /// connection pool never load at boot/idle — only the updater, the post-scan
    /// Deezer artist-image fetch, and scrobbling pull them in. reqwest's
    /// connection pool lives on the client, so callers reuse this accessor (and
    /// `ScrobbleService`, which shares the same `OnceLock`) rather than
    /// constructing a new client per request.
    pub fn http_client(&self) -> &reqwest::Client {
        self.http_client.get_or_init(crate::services::build_http_client)
    }

    /// The still-unbuilt cell behind [`Self::http_client`], for the one consumer that has to carry
    /// the client somewhere `AppState` doesn't reach: `PlaybackContext`, whose transport commands
    /// re-open a paused radio station. Handing over the `OnceLock` rather than the client keeps
    /// the construction lazy — a player that never tunes to a station still never loads rustls.
    pub fn http_client_cell(&self) -> Arc<OnceLock<reqwest::Client>> {
        self.http_client.clone()
    }

    /// Whether the Radio section is switched on. The cheap synchronous gate
    /// `library::radio` gives every outbound call, and what `boot::ui_setup`
    /// folds a persisted nav index against.
    pub fn radio_enabled(&self) -> bool {
        self.radio_enabled.load(Ordering::Relaxed)
    }

    /// Mirror a flipped Radio toggle into the shadow, ahead of the disk write —
    /// a directory call racing the persist must see the new answer, not the one
    /// still on disk.
    pub fn set_radio_enabled(&self, enabled: bool) {
        self.radio_enabled.store(enabled, Ordering::Relaxed);
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

    // The visualizer is deliberately absent: its tap is armed by the
    // Now-Playing view being on screen, not by a persisted flag, so it must
    // stay disarmed until `Visualizer.set-active` fires. See
    // `crate::ui::visualizer`.
}
