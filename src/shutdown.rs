//! Shutdown sequence: state flush, task cancellation + bounded wait,
//! runtime drop in background, optional respawn.

use melodia::services::single_instance::RESPAWN_ENV;
use melodia::{AppWindow, Nav, services, state::AppState, ui};
use slint::ComponentHandle;

/// Persist playback position and queue before shutdown. Synchronous writes only:
/// the runtime is about to be torn down, so a spawned task could be dropped
/// mid-write. The function-local `AtomicBool` makes it safe to call from both
/// shutdown paths (window close and run-loop exit).
pub fn save_state_on_exit(app: &AppWindow, state: &AppState, runtime: &tokio::runtime::Runtime) {
    use melodia::database::queries;
    use melodia::player::state::lock_state;
    use std::sync::atomic::{AtomicBool, Ordering};

    static SAVED: AtomicBool = AtomicBool::new(false);
    if SAVED.swap(true, Ordering::AcqRel) {
        return;
    }

    let (track_data, persistable, volume, is_muted) = {
        let s = lock_state(&state.player_state);
        let td = s.current_track.as_ref().map(|t| (t.id, s.position_ms));
        (td, s.queue.to_persistable(), s.volume, s.is_muted)
    };

    // `Nav.sidebar-width` always holds the expanded-mode width; collapsed
    // mode renders `Theme.sidebar-collapsed-w` separately and does not
    // mutate this property (drag handle is gated on `!sidebar-collapsed`).
    let sidebar_width = f64::from(app.global::<Nav>().get_sidebar_width());
    let sidebar_collapsed = app.global::<Nav>().get_sidebar_collapsed();

    // Volume, mute, sidebar width and window geometry live only in
    // UI/PlayerState during a session, so they need flushing to survive one.
    match services::settings::read_settings(&state.paths) {
        Ok(mut settings) => {
            settings.volume = volume;
            settings.playback.is_muted = is_muted;
            settings.sidebar_width = sidebar_width;
            settings.layout.sidebar_collapsed = sidebar_collapsed;
            // For the next launch's `BackendSelector` attributes hook. Size and
            // position are skipped while maximized, so the user's real restore
            // geometry survives.
            ui::window_chrome::geometry::snapshot_into(&mut settings);
            if let Err(e) = services::settings::write_settings(&state.paths, &settings) {
                log::warn!("save_state_on_exit: write settings.json: {e}");
            }
        }
        Err(e) => log::warn!("save_state_on_exit: read settings.json: {e}"),
    }

    // Column widths and visibility into views.json. The drag clamps to
    // per-column min/max in `track-list-header.slint`, so persisted values are
    // always in range. Every other view-state field is written eagerly by its
    // own callback; only column state needs a shutdown snapshot — plus the one
    // retired key below.
    match services::view_state::read_view_state(&state.paths) {
        Ok(mut vs) => {
            ui::track_list_view::snapshot_tracks_view(app, &mut vs);
            ui::track_list_view::snapshot_browse_view(app, &mut vs);
            ui::track_list_view::snapshot_album_detail_view(app, &mut vs);
            ui::track_list_view::snapshot_artist_detail_view(app, &mut vs);
            ui::track_list_view::snapshot_genre_detail_view(app, &mut vs);
            ui::track_list_view::snapshot_playlist_detail_view(app, &mut vs);
            ui::track_list_view::snapshot_favorites_view(app, &mut vs);
            ui::track_list_view::snapshot_recently_played_view(app, &mut vs);
            ui::track_list_view::snapshot_search_view(app, &mut vs);
            // Recently Played stopped being sortable; drop the key builds
            // before that wrote so an upgraded views.json doesn't carry
            // state nothing reads.
            vs.view_sort.remove(ui::track_list_view::view_id::RECENTLY_PLAYED);
            if let Err(e) = services::view_state::write_view_state(&state.paths, &vs) {
                log::warn!("save_state_on_exit: write views.json: {e}");
            }
        }
        Err(e) => log::warn!("save_state_on_exit: read views.json: {e}"),
    }

    if let Some((track_id, position_ms)) = track_data
        && let Err(e) = runtime.block_on(queries::track::update_last_position(
            &state.db,
            track_id,
            i64::try_from(position_ms).unwrap_or(i64::MAX),
        ))
    {
        log::warn!("save_state_on_exit: update_last_position {track_id}: {e}");
    }

    if let Err(e) = services::write_json_atomic_sync(&state.paths.queue_path, &persistable) {
        log::warn!("save_state_on_exit: write queue.json: {e}");
    }
}

/// Cancel the shutdown token, close the task tracker, and wait up to 3 s
/// for both `tracker.wait()` and `db.close()` to settle. Returns `true` if
/// the wait completed within the budget. Past the budget the caller forces
/// exit — persisted state was already written by `save_state_on_exit`.
pub fn flush_tasks_and_db(runtime: &tokio::runtime::Runtime, state: AppState) -> bool {
    state.shutdown_token.cancel();
    state.task_tracker.close();

    let tracker = state.task_tracker.clone();
    let db = state.db.clone();
    drop(state);

    // One budget over both operations. Most tasks honour `shutdown_token` and
    // exit within a `select!` tick, but `spawn_blocking` work can't be
    // cancelled — `first_launch::run` may be deep in a scan, `material_you` in a
    // quantize pass — and `db.close()` waits for every connection to come back
    // to the pool, which a task holding one across an await pins indefinitely.
    // Past the budget we force-exit regardless of what tokio thinks is pending.
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
    let res = std::thread::Builder::new().name("melodia-runtime-drop".into()).spawn(move || {
        drop(runtime);
    });
    if let Err(e) = res {
        log::warn!("could not spawn runtime-drop thread: {e}");
    }
}

/// Replace this process with a fresh copy of the binary when `ui::window_chrome`
/// left the respawn flag set (a titlebar-mode restart, or the updater's "Restart
/// Now"). `execvp` semantics, so the old window goes the instant the new one
/// arrives with no fork-then-exit race on screen.
///
/// Which binary is `window_chrome::respawn_target`'s answer: the path the
/// updater recorded before its install swapped the file, else the running one
/// through `services::current_exe` — so a package upgrade mid-session can't hand
/// back the `<path> (deleted)` the kernel appends to an unlinked executable.
///
/// **Unix** — `CommandExt::exec` (`execvp`) replaces the process atomically:
/// same PID, fds, env, cwd, process group. None of the "parent exits → child
/// gets SIGHUP / broken pipe / orphan reap" modes apply, which is what made
/// `spawn` + `process::exit` flaky under KDE's `kio-launcher` — it wires stdio
/// to a journald pipe and keeps the child in its own process group, so the
/// parent's exit raced the new child's first paint. Only returns on failure.
///
/// **Windows / exec failure** — falls through to `spawn_detached`.
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
        // `exec` only returns on failure — successful invocation
        // replaces this process in place and never resumes here.
        let err = std::process::Command::new(&exe).exec();
        log::warn!(
            "respawn exec failed for {}: {err}; falling back to detached spawn",
            exe.display()
        );
    }

    // Only on exec failure, or on a non-Unix target.
    if let Err(e) = spawn_detached(&exe) {
        log::warn!("respawn spawn failed for {}: {e}", exe.display());
    }
}

/// Spawn `exe` detached, stdio to `/dev/null`. On Unix its own process group,
/// so a signal to the parent's after `process::exit` can't take it down;
/// Windows has no such handoff problem and `Command::spawn` suffices.
fn spawn_detached(exe: &std::path::Path) -> std::io::Result<std::process::Child> {
    let mut cmd = std::process::Command::new(exe);
    // Marks the child a restart, so it waits for the single-instance name
    // rather than forwarding to a parent that is about to exit — both being
    // alive at once is what separates this path from the `exec` above, which
    // frees the name at the image replace and so is deliberately left unmarked.
    cmd.env(RESPAWN_ENV, "1");
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
}
