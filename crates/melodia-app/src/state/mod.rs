use std::sync::{Arc, OnceLock};

use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub mod contexts;
#[cfg(test)]
pub(crate) mod fixtures;
pub mod scan;
pub mod signal;
pub use contexts::PlaybackContext;
pub use scan::{ScanControl, ScanPhase, ScanProgressTick};
pub use signal::{SharedFlag, Signal};

use crate::services::search_history::SearchHistoryState;
use crate::services::settings;
use melodia_artwork::media::image::artwork::CoverCache;
use melodia_core::config::Paths;
use melodia_core::error::{AppError, AppResult, describe};
use melodia_core::utils::self_writes::SelfWrites;
use melodia_engine::player::engine::backend::PlaybackEngine;
use melodia_engine::player::engine::event_sink::{MediaControlsSync, PlayerEvent, PlayerSinks};
use melodia_engine::player::engine::signal_path::SignalPath;
use melodia_engine::player::engine::state::{
    PlayerStateHandle, PlayerViewModelLight, PositionTick, QueueViewModel, lock_state,
};
use melodia_integrations::services::integrations::discord::DiscordPresenceService;
use melodia_integrations::services::integrations::media_controls::{self, MediaControlsHandle};
use melodia_integrations::services::integrations::scrobble::ScrobbleService;
use melodia_net::services::net::pacer::RequestPacer;
use melodia_platform::services::platform::always_on_top::{self, AlwaysOnTopCapability};
use melodia_playback::player::playback::decks::DECK_COUNT;
use melodia_playback::player::playback::output::{AudioOutput, OutputMode};
use melodia_playback::player::playback::stream_health::AudioStreamHealth;
use melodia_store::database::{self, DbPool};
use melodia_store::media::ingest::watcher::{FileEvent, FolderWatcher};

#[derive(Clone)]
pub struct AppState {
    pub paths: Arc<Paths>,
    pub runtime: Handle,
    pub db: DbPool,
    pub cover_cache: CoverCache,
    pub player_state: Arc<PlayerStateHandle>,
    pub engine: Arc<PlaybackEngine>,
    /// Fault counters the output's error callback writes on the audio thread, drained by
    /// `tasks::audio_health` and read by the output's reopen.
    pub audio_health: Arc<AudioStreamHealth>,
    pub sinks: Arc<PlayerSinks>,
    pub position_tx: watch::Sender<Option<PositionTick>>,
    /// What the playing source goes through on its way to the device, `None` while stopped.
    pub signal_path_tx: watch::Sender<Option<SignalPath>>,
    /// Bumped after a write that changes what a library list shows. Each subscriber re-fetches
    /// what it shows, so a write that changes nothing on screen doesn't bump it.
    pub library_changed: Signal,
    /// Bumped after every play-count flush, apart from `library_changed` so a played song wakes
    /// only the views that read play stats.
    pub stats_changed: Signal,
    /// Bumped after the UI language changes, for the strings Rust renders through a trampoline into
    /// a model, which no `@tr` re-resolves. One subscriber, the notification stack, whose rows
    /// outlive every navigation.
    pub locale_changed: Signal,
    /// Bumped when the watcher reports a kernel-queue overflow (notify `Flag::Rescan`), for the
    /// "library re-syncing" toast. A burst coalesces into one.
    pub rescan_notice: Signal,
    /// Bumped after the auto-update switch reaches disk, waking `tasks::updater_daily` from a sleep
    /// of hours or a week's backoff. Only a wake: what it then does is still the file's, so a
    /// hand-edited `settings.json` works.
    pub auto_check_changed: Signal,
    /// Bumped by `tasks::audio_health` when the output device goes away, for the sticky toast that
    /// is the only notice: playback otherwise runs on in silence.
    pub audio_device_lost: Signal,
    /// How many scans are restoring cover art whose stored file went missing, for the notice. A
    /// count rather than a [`Signal`], since the boot scan can start before the notice subscribes.
    pub artwork_restoring: watch::Sender<u32>,
    /// The folder scan in flight: its progress for the Library settings bar, and its cancel.
    pub scan: Arc<ScanControl>,
    pub watcher: Arc<parking_lot::Mutex<FolderWatcher>>,
    /// Files this process just wrote (tag edits), so the watcher drops the `Modified` events of
    /// its own writes. See [`SelfWrites`].
    pub self_writes: Arc<SelfWrites>,
    pub always_on_top: AlwaysOnTopCapability,
    pub search_history: Arc<SearchHistoryState>,
    /// Scrobbling's credential and enabled shadow, and its durable offline queue.
    pub scrobble: Arc<ScrobbleService>,
    /// Discord Rich Presence, whose blocking IPC worker is spawned lazily;
    /// `tasks::discord_presence` drives it.
    pub discord: Arc<DiscordPresenceService>,
    /// The Radio master switch, mirrored off `settings.json` at boot because no reader may read a
    /// file: `library::radio`'s guard runs per directory call on a worker, and `boot::ui_setup`
    /// asks before the window shows.
    pub radio_enabled: SharedFlag,
    /// Whether directory results drop segmented stations, read per page by
    /// `library::radio::search`.
    pub radio_hide_segmented: SharedFlag,
    /// Whether playing a station reports a click back to the directory.
    pub radio_send_clicks: SharedFlag,
    /// Whether a song heard on a station is scrobbled, read by `tasks::scrobble`'s detector on
    /// every title change.
    pub radio_scrobble: SharedFlag,
    /// Whether lyrics run at all: `library::lyrics` refuses on it from a worker, and the Now
    /// Playing switch and the Settings card both write it.
    pub lyrics_enabled: SharedFlag,
    /// Whether the lyrics panel may look a sheet up online, read once per track change.
    pub lyrics_online_enabled: SharedFlag,
    /// Whether the panel draws the romanization it holds. No worker reads it: it is mirrored
    /// because the Settings card and the Now Playing menu both write it, and two writers through
    /// disk race.
    pub lyrics_romanization_shown: SharedFlag,
    /// How often the lyrics directory may be asked, and the stop it can impose. State rather than a
    /// `static`, so a test builds one fresh; shared by every clone, the floor being per host.
    pub lyrics_pacer: Arc<RequestPacer>,
    /// Whether a star rating is also written into the file's own tag, asked by
    /// `tasks::rating_writeback` once per coalesced burst.
    pub write_ratings_to_tags: SharedFlag,
    pub media_controls: Option<Arc<MediaControlsHandle>>,
    /// Built on first use by [`AppState::http_client`], one client for every clone.
    http_client: Arc<OnceLock<reqwest::Client>>,
    /// Every spawned background task, so shutdown can wait for their current writes.
    pub task_tracker: TaskTracker,
    pub shutdown_token: CancellationToken,
}

/// Receivers handed back from `AppState::init` for `boot::tasks` to consume, rather than held on
/// the state in a `Mutex<Option<…>>` nothing needs.
pub struct StartupChannels {
    pub media_control_rx: Option<mpsc::Receiver<PlayerEvent>>,
    pub file_event_rx: mpsc::Receiver<FileEvent>,
}

impl AppState {
    pub async fn init(paths: Paths, runtime: Handle) -> AppResult<(Self, StartupChannels)> {
        // Ahead of the output, which opens differently under exclusive output.
        let settings = settings::read_settings(&paths).unwrap_or_else(|e| {
            log::warn!("Failed to read settings on startup: {}; using defaults", describe(&e));
            settings::SettingsData::default()
        });

        let audio_health = Arc::new(AudioStreamHealth::default());
        let audio_output = open_output(&settings, audio_health.clone())?;
        let engine = Arc::new(PlaybackEngine::with_output(audio_output, runtime.clone())?);

        let db = database::init_database(&paths).await?;

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
        let (signal_path_tx, _) = watch::channel::<Option<SignalPath>>(None);
        let shutdown_token = CancellationToken::new();
        let scan = Arc::new(ScanControl::new(&shutdown_token));

        let (mc_handle, mc_rx) = media_controls::init_media_controls();
        let mc_handle = Arc::new(mc_handle);
        let media_controls_sync: Arc<dyn MediaControlsSync> = mc_handle.clone();

        let sinks = Arc::new(PlayerSinks {
            view_model: vm_tx,
            queue: q_tx,
            media_controls: Some(media_controls_sync),
            awaited_start: watch::channel(None).0,
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
        // One cell behind `http_client()`, scrobbling and Discord, so the app has one pool.
        let http_client = Arc::new(OnceLock::new());
        let scrobble =
            Arc::new(ScrobbleService::init(&paths, &settings.scrobble, http_client.clone()));
        let discord =
            Arc::new(DiscordPresenceService::init(&settings.discord, http_client.clone()));

        let state = Self {
            paths: Arc::new(paths),
            runtime,
            db,
            cover_cache,
            player_state,
            engine,
            audio_health,
            sinks,
            position_tx,
            signal_path_tx,
            library_changed: Signal::new(),
            stats_changed: Signal::new(),
            locale_changed: Signal::new(),
            rescan_notice: Signal::new(),
            auto_check_changed: Signal::new(),
            audio_device_lost: Signal::new(),
            artwork_restoring: watch::Sender::new(0),
            scan,
            watcher,
            self_writes: Arc::new(SelfWrites::default()),
            always_on_top: always_on_top_capability,
            search_history,
            scrobble,
            discord,
            radio_enabled: SharedFlag::new(settings.radio.radio_enabled),
            radio_hide_segmented: SharedFlag::new(settings.radio.radio_hide_segmented),
            radio_send_clicks: SharedFlag::new(settings.radio.radio_send_clicks),
            radio_scrobble: SharedFlag::new(settings.radio.radio_scrobble),
            lyrics_enabled: SharedFlag::new(settings.lyrics.lyrics_enabled),
            lyrics_online_enabled: SharedFlag::new(settings.lyrics.lyrics_online_enabled),
            lyrics_romanization_shown: SharedFlag::new(settings.lyrics.lyrics_romanization_shown),
            lyrics_pacer: Arc::new(crate::library::lyrics::pacer()),
            write_ratings_to_tags: SharedFlag::new(settings.library.write_ratings_to_tags),
            media_controls: Some(mc_handle),
            http_client,
            task_tracker: TaskTracker::new(),
            shutdown_token,
        };

        let channels = StartupChannels { media_control_rx: Some(mc_rx), file_event_rx: file_rx };

        Ok((state, channels))
    }

    /// Persist a settings mutation on the blocking pool, logging a failure under `label`.
    ///
    /// The write half of "apply live, then persist": the caller has already applied the value to
    /// the running player, so a failed write must not undo it and the warn is the only report.
    pub fn persist_blocking(
        &self,
        label: &'static str,
        f: impl FnOnce(&Paths) -> Result<(), AppError> + Send + 'static,
    ) {
        // On the way in, so a write that hangs still says what it was.
        log::debug!("settings: {label}");
        let paths = Arc::clone(&self.paths);
        self.runtime.spawn_blocking(move || {
            if let Err(e) = f(&paths) {
                log::warn!("{label}: {}", describe(&e));
            }
        });
    }

    /// The shared `reqwest::Client`, built on first call so rustls and the connection pool never
    /// load at boot or idle. The pool lives on the client, so every remote call reuses this rather
    /// than building one.
    pub fn http_client(&self) -> &reqwest::Client {
        self.http_client.get_or_init(melodia_net::services::net::build_http_client)
    }

    /// The still-unbuilt cell behind [`Self::http_client`], for `PlaybackContext`, whose transport
    /// commands re-open a paused station. The cell rather than the client keeps construction lazy:
    /// a player that never tunes in never loads rustls.
    pub fn http_client_cell(&self) -> Arc<OnceLock<reqwest::Client>> {
        self.http_client.clone()
    }
}

/// Open the output the saved choice will play through. Parked under exclusive output, which claims
/// the card at the first play, and for a chosen shared device, which [`hydrate_audio_dsp`] opens:
/// opening the default device shared would only close it again.
fn open_output(
    settings: &settings::SettingsData,
    health: Arc<AudioStreamHealth>,
) -> AppResult<AudioOutput> {
    let choice = settings.output.output_choice();
    if choice.mode == OutputMode::Exclusive {
        log::info!("Audio output: parked until the first play claims the device");
        return AudioOutput::open_parked(DECK_COUNT, health);
    }
    if choice.shared_device().is_some() {
        log::info!("Audio output: parked until the chosen device opens");
        return AudioOutput::open_parked(DECK_COUNT, health);
    }
    let output = AudioOutput::open(DECK_COUNT, health)?;
    log::info!("Audio output: {:?}", output.negotiated());
    Ok(output)
}

/// Seed the engine's lock-free DSP cells from persisted settings before playback starts, so the
/// first track is already processed. Values first and each `enabled` last, the enable's generation
/// bump publishing a fully seeded state to the audio thread.
fn hydrate_audio_dsp(engine: &PlaybackEngine, settings: &settings::SettingsData) {
    engine.set_eq_gains(&settings.equalizer.eq_band_gains);
    engine.set_eq_preamp(settings.equalizer.eq_preamp);
    engine.set_eq_enabled(settings.equalizer.eq_enabled);

    engine.set_replaygain_preamp(settings.replaygain.rg_preamp);
    engine.set_replaygain_mode(
        melodia_playback::player::playback::replaygain::RgMode::from_settings_str(
            &settings.replaygain.rg_mode,
        ),
    );
    engine.set_replaygain_prevent_clipping(settings.replaygain.rg_prevent_clipping);
    engine.set_replaygain_enabled(settings.replaygain.rg_enabled);

    engine.set_crossfade_duration_ms(settings.crossfade.crossfade_duration_ms);
    engine.set_crossfade_manual(settings.crossfade.crossfade_manual);
    engine.set_crossfade_skip_same_album(settings.crossfade.crossfade_skip_same_album);
    engine.set_crossfade_fade_on_pause(settings.crossfade.crossfade_fade_on_pause);
    engine.set_crossfade_enabled(settings.crossfade.crossfade_enabled);
    engine.set_follow_rate(settings.output.output_follow_rate);
    engine.set_resampler(settings.output.output_resampler);
    engine.set_release_when_paused(settings.output.output_paused_device.releases());
    engine.set_resync_hold(settings.output.resync_hold());
    // Nothing is loaded yet, so an exclusive choice leaves the output parked as `open_output` left
    // it, and claims the card at the first play. A shared one opens its chosen device here.
    engine.set_output_choice(settings.output.output_choice());

    // No visualizer: its tap is armed by Now Playing being on screen (`melodia-views`'
    // `ui/visualizer/`), never by a persisted flag.
}
