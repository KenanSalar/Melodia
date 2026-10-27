//! The backend half of boot: background tasks, the restored queue, and what a launch plays.

use std::sync::Arc;

use slint::ComponentHandle;

use melodia_app::state::{AppState, StartupChannels};
use melodia_app::{library, services, tasks};
use melodia_core::error::describe;
use melodia_core::utils;
use melodia_engine::player::engine::event_sink::EventSink;
use melodia_integrations::services::integrations;
use melodia_platform::services::platform;
use melodia_ui::AppWindow;
use melodia_views::ui;

/// Spawns every always-running background task and the OS media-control event receiver.
pub fn spawn_background_tasks(
    spawner: &tasks::TaskSpawner,
    state: &AppState,
    mut channels: StartupChannels,
) {
    tasks::playback_monitor::spawn(spawner, state);
    tasks::file_event_processor::spawn(spawner, state, channels.file_event_rx);
    tasks::queue_prune::spawn(spawner, state);
    // Before the restore, whose first window change it has to see.
    tasks::queue_window::spawn(spawner, state);
    tasks::retroactive_hash::spawn(spawner, state);
    tasks::search_text_backfill::spawn(spawner, state);
    tasks::artwork_restore::spawn(spawner, state);
    tasks::artwork_sweep::spawn(spawner, state);
    // The three once-per-install passes, each gated on its own settings marker.
    tasks::artwork_renormalize::spawn(spawner, state);
    tasks::rating_import::spawn(spawner, state);
    tasks::tag_backfill::spawn(spawner, state);
    // Before any callback can set a rating.
    tasks::rating_writeback::spawn(spawner, state);
    tasks::heap_trim::spawn(spawner);
    tasks::audio_health::spawn(spawner, state);
    tasks::device_volume::spawn(spawner, state);
    // Before any playback can fire an `UpdatePlayCount`.
    tasks::play_count_flusher::spawn(spawner, state.db.clone(), state.stats_changed.clone());
    tasks::scrobble::spawn(spawner, state);
    tasks::discord_presence::spawn(spawner, state);
    // Started here when already enabled, so the card connects while idle rather than on the
    // first track.
    state.discord.start_if_enabled();

    if let Some(rx) = channels.media_control_rx.take() {
        let sink: Arc<dyn EventSink> =
            Arc::new(ui::shell::event_sink::SlintEventSink { state: state.clone() });
        integrations::media_controls::spawn_event_receiver(
            &state.task_tracker,
            state.shutdown_token.clone(),
            rx,
            sink,
        );
        log::info!("Media controls event receiver started");
    }
}

/// Restore the persisted queue and the station over it from disk (best-effort; missing file is OK).
pub fn restore_persisted_playback(runtime: &tokio::runtime::Runtime, state: &AppState) {
    if let Err(e) = runtime.block_on(library::queue::restore_persisted_playback(state)) {
        log::warn!("Failed to restore persisted playback: {}", describe(&e));
    }
}

/// Plays a restored source when "Resume on Startup" is on, here so the first frame paints
/// `Playing`. A station re-opens through the same call and paints `Connecting…` instead.
pub fn maybe_resume_on_startup(
    state: &AppState,
    startup_settings: Option<&services::settings::SettingsData>,
) {
    let resume_enabled = startup_settings.is_some_and(|s| s.playback.resume_on_startup);
    if !resume_enabled {
        return;
    }
    // The guard drops before `player_play`, which re-enters this lock.
    let has_source = {
        let s = melodia_engine::player::engine::state::lock_state(&state.player_state);
        s.source.is_some()
    };
    if has_source && let Err(e) = library::playback::player_play(&state.playback_ctx()) {
        log::warn!("resume_on_startup: player_play failed: {}", describe(&e));
    }
}

/// Plays the files this launch was handed, after [`restore_persisted_playback`] so they win over
/// what the last session left. Synchronous, so the first frame paints what was opened.
pub fn open_startup_files(
    runtime: &tokio::runtime::Runtime,
    state: &AppState,
    files: &[std::path::PathBuf],
) {
    let paths: Vec<String> = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    // Too early to toast: the bridge installs with the UI and drops what arrives before it.
    if let Err(e) = runtime.block_on(library::queue::open_files(state, paths)) {
        log::warn!("Failed to open the files given on the command line: {}", describe(&e));
    }
}

/// Starts accepting forwarded launches, including those queued since the boot's claim.
pub fn serve_file_opens(
    state: &AppState,
    app: &AppWindow,
    listener: platform::single_instance::Listener,
) {
    let state = state.clone();
    let weak = app.as_weak();

    platform::single_instance::serve(listener, move |paths| {
        // Raised either way: an empty forward is a launch to get at the window.
        let _ = weak.upgrade_in_event_loop(|ui| ui::shell::tray_bridge::raise_window(&ui));

        if paths.is_empty() {
            return;
        }
        let state = state.clone();
        state.runtime.clone().spawn(async move {
            if let Err(e) = library::queue::open_files(&state, paths).await {
                log::warn!("Failed to open forwarded files: {}", describe(&e));
                utils::toast::notify(utils::toast::ToastKind::OperationFailed, describe(&e));
            }
        });
    });
}

/// Restarts folder watching and runs the launch reconcile, tracked so shutdown awaits its writes.
pub fn spawn_resume_watching(spawner: &tasks::TaskSpawner, state: &AppState) {
    let state_for_init = state.clone();
    spawner.spawn(async move {
        if let Err(e) = tasks::resume_watching::run(&state_for_init).await {
            log::warn!("Resuming folder watching failed: {}", describe(&e));
        }
    });
}
