//! What `main()` runs once the event loop has returned.

use melodia_app::services;
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_core::utils;
use melodia_platform::services::platform::single_instance::RESPAWN_ENV;
use melodia_ui::{AppWindow, Nav, Queue};
use melodia_views::ui;
use slint::ComponentHandle;

/// Persists what only lives in memory during a session. Synchronous writes only: the runtime is
/// about to go, and a spawned task could be dropped mid-write.
pub fn save_state_on_exit(app: &AppWindow, state: &AppState, runtime: &tokio::runtime::Runtime) {
    use melodia_engine::player::engine::state::lock_state;
    use melodia_store::database::queries;
    use std::sync::atomic::{AtomicBool, Ordering};

    static SAVED: AtomicBool = AtomicBool::new(false);
    if SAVED.swap(true, Ordering::AcqRel) {
        return;
    }

    let (track_data, persistable, volume, is_muted) = {
        let s = lock_state(&state.player_state);
        (s.resume_point(), s.to_persisted(), s.volume, s.is_muted)
    };

    // The expanded width, which the rail never writes, and the user's own fold rather than the
    // rail a narrow window forces.
    let sidebar_width = f64::from(app.global::<Nav>().get_sidebar_width());
    let sidebar_collapsed = app.global::<Nav>().get_sidebar_collapsed();

    // `open` rather than `shown`, so a panel stepped aside comes back with the room. A width of 0
    // is the default, which stays `Theme`'s to change.
    let queue = app.global::<Queue>();
    let queue_panel_open = queue.get_open();
    let panel_width = f64::from(queue.get_panel_width());
    let queue_panel_width = (panel_width > 0.0).then_some(panel_width);

    // Under the settings lock, so a toggle still persisting as the window closed isn't written
    // back over.
    let flushed = services::settings::mutate_settings(&state.paths, |settings| {
        settings.volume = volume;
        settings.playback.is_muted = is_muted;
        settings.sidebar_width = sidebar_width;
        settings.layout.sidebar_collapsed = sidebar_collapsed;
        settings.queue_panel.queue_panel_open = queue_panel_open;
        settings.queue_panel.queue_panel_width = queue_panel_width;
        ui::window_chrome::geometry::snapshot_into(settings);
    });
    if let Err(e) = flushed {
        log::warn!("save_state_on_exit: settings.json: {}", describe(&e));
    }

    // Every other `views.json` field is written by its own callback as it changes.
    match services::view_state::read_view_state(&state.paths) {
        Ok(mut vs) => {
            ui::track_list_view::snapshot_all(app, &mut vs);
            // Recently Played has no sort, and an older build's key would sit there unread.
            vs.view_sort.remove(ui::track_list_view::view_id::RECENTLY_PLAYED);
            if let Err(e) = services::view_state::write_view_state(&state.paths, &vs) {
                log::warn!("save_state_on_exit: write views.json: {}", describe(&e));
            }
        }
        Err(e) => log::warn!("save_state_on_exit: read views.json: {}", describe(&e)),
    }

    if let Some((track_id, position_ms)) = track_data
        && let Err(e) = runtime.block_on(queries::track::update_last_position(
            &state.db,
            track_id,
            i64::try_from(position_ms).unwrap_or(i64::MAX),
        ))
    {
        log::warn!("save_state_on_exit: update_last_position {track_id}: {}", describe(&e));
    }

    if let Err(e) = utils::atomic_file::write_json_sync(&state.paths.queue_path, &persistable) {
        log::warn!("save_state_on_exit: write queue.json: {}", describe(&e));
    }
}

/// Cancels every task and waits up to 3 s for them and the pool to settle, returning whether they
/// did. Past that the caller exits anyway, `save_state_on_exit` having written what matters.
pub fn flush_tasks_and_db(runtime: &tokio::runtime::Runtime, state: AppState) -> bool {
    state.shutdown_token.cancel();
    state.task_tracker.close();

    let tracker = state.task_tracker.clone();
    let db = state.db.clone();
    drop(state);

    // One budget over both: `spawn_blocking` work can't be cancelled from outside (a scan stops
    // only at its own checkpoints), and `db.close()` waits for every connection to come back,
    // which a task holding one across an await can pin indefinitely.
    runtime.block_on(async move {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tracker.wait().await;
            db.close().await;
        })
        .await
        .is_ok()
    })
}

/// Drop the runtime off the exit path: its `Drop` waits on worker-thread join,
/// which can block on `Drop`s of audio / D-Bus / accesskit threads we don't own.
pub fn drop_runtime_in_background(runtime: tokio::runtime::Runtime) {
    let res = std::thread::Builder::new().name("melodia-rt-drop".into()).spawn(move || {
        drop(runtime);
    });
    if let Err(e) = res {
        log::warn!("could not spawn runtime-drop thread: {}", describe(&e));
    }
}

/// Replaces this process with the binary `window_chrome::respawn_target` names, when
/// `window_chrome` left the respawn flag set.
///
/// On Unix through `exec`, which keeps the PID, fds and process group, so the old window goes as
/// the new one arrives. A spawn and exit raced the child's first paint under a launcher that pipes
/// stdio to journald and keeps its own process group. Elsewhere, or if `exec` fails,
/// [`spawn_detached`].
pub fn respawn_if_requested() {
    if !ui::window_chrome::should_respawn_after_exit() {
        return;
    }
    // `respawn_target` logs the reason; there is nothing to fall back to.
    let Some(exe) = ui::window_chrome::respawn_target() else {
        return;
    };

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Returns only on failure.
        let err = std::process::Command::new(&exe).exec();
        log::warn!(
            "respawn exec failed for {}: {}; falling back to detached spawn",
            exe.display(),
            describe(&err)
        );
    }

    if let Err(e) = spawn_detached(&exe) {
        log::warn!("respawn spawn failed for {}: {}", exe.display(), describe(&e));
    }
}

/// Spawns `exe` detached, in its own process group on Unix so a signal to the parent's can't take
/// it down.
fn spawn_detached(exe: &std::path::Path) -> std::io::Result<std::process::Child> {
    detached_command(exe).spawn()
}

/// [`spawn_detached`]'s command, built rather than spawned so the mark below can be asserted.
fn detached_command(exe: &std::path::Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe);
    // A restart waits for the single-instance name rather than forwarding to a parent about to
    // exit. `exec` frees the name at the image replace, so that arm is left unmarked.
    cmd.env(RESPAWN_ENV, "1");
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

#[cfg(test)]
#[path = "tests/shutdown_tests.rs"]
mod tests;
