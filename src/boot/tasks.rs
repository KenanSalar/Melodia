//! Background-task spawning + queue restore + resume-on-startup.

use std::sync::Arc;

use melodia::{
    library,
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
    tasks::heap_trim::spawn(spawner);
    // Batches `play_count` / `skip_count` UPDATEs every 2 s so a fast skip
    // burst becomes one write instead of N. Must be spawned before any
    // playback can fire `UpdatePlayCount` actions.
    tasks::play_count_flusher::spawn(
        spawner,
        state.db.clone(),
        state.stats_changed_tx.clone(),
    );
    // Scrobble detector + submitter: watch the player view-model/position seam,
    // enqueue qualifying plays, and drain the durable queue to Last.fm /
    // ListenBrainz. Inert until a provider is connected + enabled.
    tasks::scrobble::spawn(spawner, state);
    // Auto-tag scanned tracks with their MusicBrainz Recording ID via
    // ListenBrainz so loves work on untagged libraries. Inert until the user
    // enables it + ListenBrainz is connected.
    tasks::mbid_backfill::spawn(spawner, state);

    // OS media controls → SlintEventSink: souvlaki events drive the same
    // library::* paths the UI does, so MPRIS / SMTC stays in lockstep.
    if let Some(rx) = channels.media_control_rx.take() {
        let sink: Arc<dyn EventSink> = Arc::new(ui::event_sink::SlintEventSink {
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

/// Restore the persisted queue from disk (best-effort; missing file is OK).
pub fn restore_persisted_queue(runtime: &tokio::runtime::Runtime, state: &AppState) {
    if let Err(e) = runtime.block_on(library::queue::restore_persisted_queue(state)) {
        log::warn!("Failed to restore persisted queue: {e}");
    }
}

/// If the user enabled "Resume on Startup" AND the restored queue has a
/// current track, kick `player_play()` here so the very first frame paints
/// with `status=Playing` and the rodio thread is already decoding.
pub fn maybe_resume_on_startup(
    state: &AppState,
    startup_settings: Option<&services::settings::SettingsData>,
) {
    let resume_enabled = startup_settings.is_some_and(|s| s.playback.resume_on_startup);
    if !resume_enabled {
        return;
    }
    // Bind the bool out and drop the `std::sync::Mutex` guard
    // *before* calling `player_play()` — that fn re-enters the same
    // lock via `with_state_emit`, so holding the guard across the
    // call would deadlock.
    let has_track = melodia::player::state::lock_state(&state.player_state)
        .current_track
        .is_some();
    if has_track
        && let Err(e) = library::playback::player_play(&state.playback_ctx())
    {
        log::warn!("resume_on_startup: player_play failed: {e}");
    }
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
