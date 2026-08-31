//! Background-task spawning, queue restore, and what the two ways of starting
//! resolve to: resume-on-startup, or the files this launch was handed.

use std::sync::Arc;

use slint::ComponentHandle;

use melodia::{
    AppWindow, library,
    player::event_sink::EventSink,
    services,
    state::{AppState, StartupChannels},
    tasks, ui,
};

/// Spawn every always-running background task and the souvlaki event
/// receiver. Consumes `channels`.
pub fn spawn_background_tasks(
    spawner: &tasks::TaskSpawner,
    state: &AppState,
    mut channels: StartupChannels,
) {
    tasks::playback_monitor::spawn(spawner, state);
    tasks::file_event_processor::spawn(spawner, state, channels.file_event_rx);
    tasks::queue_prune::spawn(spawner, state);
    tasks::retroactive_hash::spawn(spawner, state);
    // One-shot, and gated on its own settings marker rather than on anything here.
    tasks::artwork_renormalize::spawn(spawner, state);
    tasks::heap_trim::spawn(spawner);
    // Folds the output device's fault counters into one line per window, and is
    // the only thing watching for a device that goes away mid-session.
    tasks::audio_health::spawn(spawner, state);
    // Batches `play_count` / `skip_count` UPDATEs so a fast skip burst is one
    // write. Before any playback can fire an `UpdatePlayCount`.
    tasks::play_count_flusher::spawn(spawner, state.db.clone(), state.stats_changed_tx.clone());
    // Watches the view-model/position seam, enqueues qualifying plays and
    // drains the durable queue. Inert until a provider is connected.
    tasks::scrobble::spawn(spawner, state);
    // Auto-tags Recording IDs so loves work on an untagged library. Inert until
    // enabled and ListenBrainz is connected.
    tasks::mbid_backfill::spawn(spawner, state);
    // Projects the view-model into a Discord activity card. Inert until enabled;
    // started here when it already is, so the card connects while idle rather
    // than on the first track.
    tasks::discord_presence::spawn(spawner, state);
    state.discord.start_if_enabled();

    // souvlaki events drive the same `library::*` paths the UI does, keeping
    // MPRIS / SMTC in lockstep with it.
    if let Some(rx) = channels.media_control_rx.take() {
        let sink: Arc<dyn EventSink> = Arc::new(ui::shell::event_sink::SlintEventSink {
            state: state.clone(),
        });
        services::media_controls::spawn_event_receiver(
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
        log::warn!("Failed to restore persisted playback: {e}");
    }
}

/// With "Resume on Startup" on and a restored source, play here so the
/// first frame paints `Playing` and rodio is already decoding.
///
/// A restored station goes through the same call and comes back the way it went away — as a
/// re-open, since `player_play` routes a paused station to `resume_station`. That one is a
/// network round trip rather than a decode, so the first frame paints `Connecting…` instead.
pub fn maybe_resume_on_startup(
    state: &AppState,
    startup_settings: Option<&services::settings::SettingsData>,
) {
    let resume_enabled = startup_settings.is_some_and(|s| s.playback.resume_on_startup);
    if !resume_enabled {
        return;
    }
    // Bind the bool out so the guard drops *before* `player_play()`, which
    // re-enters the same lock through `with_state_emit`.
    let has_source = {
        let s = melodia::player::state::lock_state(&state.player_state);
        s.source.is_some()
    };
    if has_source && let Err(e) = library::playback::player_play(&state.playback_ctx()) {
        log::warn!("resume_on_startup: player_play failed: {e}");
    }
}

/// Play the files this launch was handed on the command line.
///
/// After [`restore_persisted_playback`] so an opened track wins over what the
/// last session left; `main()` skips [`maybe_resume_on_startup`] when there are
/// files, resuming being visible for the moment it takes this to replace it.
/// Synchronous like the restore — the first frame should paint what was
/// double-clicked.
pub fn open_startup_files(
    runtime: &tokio::runtime::Runtime,
    state: &AppState,
    files: &[std::path::PathBuf],
) {
    let paths: Vec<String> = files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    // Too early to toast: that bridge installs with the UI and drops rather
    // than queues what arrives before it.
    if let Err(e) = runtime.block_on(library::queue::open_files(state, paths)) {
        log::warn!("Failed to open the files given on the command line: {e}");
    }
}

/// Start accepting forwarded launches. The listener has been bound since before
/// the logger opened, so what waited in the backlog is delivered from here.
pub fn serve_file_opens(
    state: &AppState,
    app: &AppWindow,
    listener: services::single_instance::Listener,
) {
    let state = state.clone();
    let weak = app.as_weak();

    services::single_instance::serve(listener, move |paths| {
        // Raise either way — an empty forward is someone launching Melodia
        // again to get at the window.
        let _ = weak.upgrade_in_event_loop(|ui| ui::shell::tray_bridge::raise_window(&ui));

        if paths.is_empty() {
            return;
        }
        let state = state.clone();
        state.runtime.clone().spawn(async move {
            if let Err(e) = library::queue::open_files(&state, paths).await {
                log::warn!("Failed to open forwarded files: {e}");
                services::toast::notify(services::toast::ToastKind::OperationFailed, e.to_string());
            }
        });
    });
}

/// First-launch auto-add + folder-watcher restart (off-thread, tracked so
/// shutdown can await its DB writes before dropping the runtime).
pub fn spawn_first_launch(spawner: &tasks::TaskSpawner, state: &AppState) {
    let state_for_init = state.clone();
    spawner.spawn(async move {
        if let Err(e) = tasks::first_launch::run(&state_for_init).await {
            log::warn!("First-launch init failed: {e}");
        }
    });
}
