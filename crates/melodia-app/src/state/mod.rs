use std::sync::{Arc, OnceLock};

use souvlaki::MediaControlEvent;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub mod contexts;
#[cfg(test)]
pub(crate) mod fixtures;
pub mod signal;
pub use contexts::PlaybackContext;
pub use signal::{SharedFlag, Signal};

use crate::services::search_history::SearchHistoryState;
use crate::services::settings;
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::config::Paths;
use melodia_core::error::{AppError, AppResult};
use melodia_core::utils::self_writes::SelfWrites;
use melodia_engine::player::engine::backend::PlaybackEngine;
use melodia_engine::player::engine::event_sink::{MediaControlsSync, PlayerSinks};
use melodia_engine::player::engine::state::{
    PlayerStateHandle, PlayerViewModelLight, PositionTick, QueueViewModel, lock_state,
};
use melodia_integrations::services::integrations::discord::DiscordPresenceService;
use melodia_integrations::services::integrations::media_controls::{self, MediaControlsHandle};
use melodia_integrations::services::integrations::scrobble::ScrobbleService;
use melodia_platform::services::platform::always_on_top::{self, AlwaysOnTopCapability};
use melodia_playback::player::playback::decks::DECK_COUNT;
use melodia_playback::player::playback::output::AudioOutput;
use melodia_playback::player::playback::stream_health::{self, AudioStreamHealth};
use melodia_store::database::{self, DbPool};
use melodia_store::media::ingest::watcher::{FileEvent, FolderWatcher};

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
    pub engine: Arc<PlaybackEngine>,
    /// The open device. Held because dropping it stops the stream and releases the card, which is
    /// what a device picker will need.
    pub audio_output: Arc<AudioOutput>,
    /// Fault counters the output device's error callback writes into, on the
    /// audio thread. Drained by `tasks::audio_health` and read nowhere else.
    pub audio_health: Arc<AudioStreamHealth>,
    pub sinks: Arc<PlayerSinks>,
    pub position_tx: watch::Sender<Option<PositionTick>>,
    /// Bumped whenever the track library is mutated by a scan or watcher
    /// event. UI subscribers re-fetch the Tracks model on each tick.
    pub library_changed: Signal,
    /// Bumped after every play-count flush. Split from `library_changed`
    /// so the per-song flush (every track completion) doesn't imply
    /// "library structure changed" — the only views that depend on
    /// play-driven data subscribe here: Favorites (hero mosaic + Most Played
    /// strip, ranked by `play_count`) and Recently-Played (ordered by
    /// `last_played`, written on the same flush). Everything else
    /// (Tracks/Browse refreshers, `queue_prune`, the folder list) stays on
    /// `library_changed` and no longer fires per played song.
    pub stats_changed: Signal,
    /// Bumped after the UI language changes. A locale switch re-resolves every live `@tr`
    /// binding on its own, so this exists only for the strings Rust renders through a
    /// trampoline and stores in a model, which nothing re-reads. One subscriber: the
    /// notification stack, whose rows outlive every navigation. Everything else so stored
    /// sits behind a section that hands its models back on the way to Settings.
    pub locale_changed: Signal,
    /// Bumped whenever the watcher reports a kernel-queue overflow (notify
    /// `Flag::Rescan`). A UI-thread subscriber pushes a transient
    /// "library re-syncing" toast through the notifications stack so the
    /// user knows why Tracks/Browse paused refreshing while the
    /// reconcile runs. Coalescing semantics: a burst of overflows that
    /// land in the same `watch` slot only paints one toast — which is
    /// also what `RECONCILE_IN_FLIGHT` does to the reconcile spawn
    /// itself.
    pub rescan_notice: Signal,
    /// Bumped by `tasks::audio_health` when the output device goes away. A
    /// UI-thread subscriber pushes a sticky warning toast — nothing else
    /// notices, so playback runs on with the position ticking and no sound.
    /// Coalesces like `rescan_notice`: a burst paints one toast.
    pub audio_device_lost: Signal,
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
    /// the window is shown. Every writer moves it *before* spawning the persist — the rule
    /// [`SharedFlag`] carries — so a directory call racing the disk write sees the new answer.
    pub radio_enabled: SharedFlag,
    /// Whether directory results drop segmented stations, on the same terms as
    /// [`Self::radio_enabled`]: `library::radio::search` reads it per page, on a
    /// worker.
    pub radio_hide_segmented: SharedFlag,
    /// Whether playing a station reports a click back to the directory. Read on
    /// the play path, which is already on a worker.
    pub radio_send_clicks: SharedFlag,
    /// Whether a star rating is also written into the file's own tag, on the same
    /// terms as [`Self::radio_enabled`]: `tasks::rating_writeback` asks once per
    /// coalesced burst, and a `settings.json` read there would be a file read on
    /// the path a rating click already paid for.
    pub write_ratings_to_tags: SharedFlag,
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
        // The error callback records into counters rather than logging, because cpal calls it on
        // the output worker thread; `player::playback::stream_health` argues that.
        let audio_health = Arc::new(AudioStreamHealth::default());
        let audio_output =
            AudioOutput::open(DECK_COUNT, stream_health::error_callback(audio_health.clone()))?;
        log::info!("Audio output: {:?}", audio_output.negotiated());
        // The runtime handle is only used to schedule the deferred half of a
        // faded pause / stop (arm the ramp now, pause the decks once it lands).
        let engine = Arc::new(PlaybackEngine::new(audio_output.mixer(), runtime.clone())?);

        let db = database::init_database(&paths).await?;

        let settings = settings::read_settings(&paths).unwrap_or_else(|e| {
            log::warn!("Failed to read settings on startup: {e}; using defaults");
            settings::SettingsData::default()
        });

        let player_state = Arc::new(PlayerStateHandle::default());
        {
            let mut s = lock_state(&player_state);
            s.volume = settings.volume.min(melodia_engine::player::engine::state::MAX_VOLUME);
            s.is_muted = settings.playback.is_muted;
            s.pre_mute_volume = s.volume;
            s.gapless_enabled = settings.playback.gapless_playback;
            s.playback_speed = settings.playback.playback_speed.clamp(
                melodia_engine::player::engine::state::MIN_SPEED,
                melodia_engine::player::engine::state::MAX_SPEED,
            );
            let vol = s.effective_volume();
            let speed = s.playback_speed;
            drop(s);
            engine.set_volume(vol);
            engine.set_speed(speed);
        }

        hydrate_audio_dsp(&engine, &settings);

        let cover_cache: CoverCache = melodia_artwork::media::image::artwork::new_cover_cache();

        let (vm_tx, _) = watch::channel::<Option<PlayerViewModelLight>>(None);
        let (q_tx, _) = watch::channel::<Option<QueueViewModel>>(None);
        let (position_tx, _) = watch::channel::<Option<PositionTick>>(None);
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
            engine,
            audio_output: Arc::new(audio_output),
            audio_health,
            sinks,
            position_tx,
            library_changed: Signal::new(),
            stats_changed: Signal::new(),
            locale_changed: Signal::new(),
            rescan_notice: Signal::new(),
            audio_device_lost: Signal::new(),
            scan_progress_tx,
            watcher,
            self_writes: Arc::new(SelfWrites::default()),
            always_on_top: always_on_top_capability,
            search_history,
            scrobble,
            discord,
            radio_enabled: SharedFlag::new(settings.radio.radio_enabled),
            radio_hide_segmented: SharedFlag::new(settings.radio.radio_hide_segmented),
            radio_send_clicks: SharedFlag::new(settings.radio.radio_send_clicks),
            write_ratings_to_tags: SharedFlag::new(settings.library.write_ratings_to_tags),
            media_controls: Some(mc_handle),
            http_client,
            task_tracker: TaskTracker::new(),
            shutdown_token: CancellationToken::new(),
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
    /// [`melodia_net::services::net::build_http_client`]) so the rustls TLS stack and
    /// connection pool never load at boot/idle — only the updater, the post-scan
    /// Deezer artist-image fetch, and scrobbling pull them in. reqwest's
    /// connection pool lives on the client, so callers reuse this accessor (and
    /// `ScrobbleService`, which shares the same `OnceLock`) rather than
    /// constructing a new client per request.
    pub fn http_client(&self) -> &reqwest::Client {
        self.http_client.get_or_init(melodia_net::services::net::build_http_client)
    }

    /// The still-unbuilt cell behind [`Self::http_client`], for the one consumer that has to carry
    /// the client somewhere `AppState` doesn't reach: `PlaybackContext`, whose transport commands
    /// re-open a paused radio station. Handing over the `OnceLock` rather than the client keeps
    /// the construction lazy — a player that never tunes to a station still never loads rustls.
    pub fn http_client_cell(&self) -> Arc<OnceLock<reqwest::Client>> {
        self.http_client.clone()
    }
}

/// Seed the playback engine's lock-free cells (graphic EQ, `ReplayGain`,
/// crossfade) from persisted settings before playback starts, so the first
/// track is already processed when any of them is enabled. All three live on
/// the engine (not `PlayerState`). Ordering is deliberate: values first,
/// `enabled` last, so the enable's generation bump publishes a fully-seeded
/// state to the audio thread.
fn hydrate_audio_dsp(engine: &PlaybackEngine, settings: &settings::SettingsData) {
    // `set_eq_gains` clamps and length-normalises the (possibly hand-edited)
    // gain list; the EQ ships off by default.
    engine.set_eq_gains(&settings.equalizer.eq_band_gains);
    engine.set_eq_preamp(settings.equalizer.eq_preamp);
    engine.set_eq_enabled(settings.equalizer.eq_enabled);

    // ReplayGain master state seeds the same way — per-track gain is baked per
    // source at play time. Ships off by default; the mode string falls back to
    // Album on an unknown value.
    engine.set_replaygain_preamp(settings.replaygain.rg_preamp);
    engine.set_replaygain_mode(
        melodia_playback::player::playback::replaygain::RgMode::from_settings_str(
            &settings.replaygain.rg_mode,
        ),
    );
    engine.set_replaygain_prevent_clipping(settings.replaygain.rg_prevent_clipping);
    engine.set_replaygain_enabled(settings.replaygain.rg_enabled);

    // Crossfade ships off; `set_crossfade_duration_ms` clamps a hand-edited value.
    engine.set_crossfade_duration_ms(settings.crossfade.crossfade_duration_ms);
    engine.set_crossfade_manual(settings.crossfade.crossfade_manual);
    engine.set_crossfade_skip_same_album(settings.crossfade.crossfade_skip_same_album);
    engine.set_crossfade_fade_on_pause(settings.crossfade.crossfade_fade_on_pause);
    engine.set_crossfade_enabled(settings.crossfade.crossfade_enabled);

    // The visualizer is deliberately absent: its tap is armed by the
    // Now-Playing view being on screen, not by a persisted flag, so it must
    // stay disarmed until `Visualizer.set-active` fires. See
    // `melodia-views`' `ui/visualizer/`.
}
