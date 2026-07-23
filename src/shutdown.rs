//! Shutdown sequence: state flush, task cancellation + bounded wait,
//! runtime drop in background, optional respawn.

use melodia::{AppWindow, Nav, services, state::AppState, ui};
use slint::ComponentHandle;

/// Persist the current track's playback position and the queue to disk
/// before shutdown. Modelled on the Tauri app's `save_state_on_exit`.
/// Synchronous writes only — the runtime is about to be torn down, so
/// spawned tasks could be dropped mid-write. A function-local `AtomicBool`
/// makes this safe to call from multiple shutdown paths (window close +
/// run-loop exit).
pub fn save_state_on_exit(
    app: &AppWindow,
    state: &AppState,
    runtime: &tokio::runtime::Runtime,
) {
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

    // Persist volume / is_muted / sidebar_width / window geometry into
    // settings.json (mirrors Tauri's save_state_on_exit). These live only
    // in UI/PlayerState during a session; we flush them here so they
    // survive a relaunch.
    match services::settings::read_settings(&state.paths) {
        Ok(mut settings) => {
            settings.volume = volume;
            settings.playback.is_muted = is_muted;
            settings.sidebar_width = sidebar_width;
            settings.layout.sidebar_collapsed = sidebar_collapsed;
            // Snapshot window geometry (size / position / maximized) so
            // the next launch's `BackendSelector` attributes-hook can
            // restore it. Skips size/position when maximized so we
            // preserve the user's real restore geometry.
            ui::window_chrome::geometry::snapshot_into(&mut settings);
            if let Err(e) = services::settings::write_settings(&state.paths, &settings) {
                log::warn!("save_state_on_exit: write settings.json: {e}");
            }
        }
        Err(e) => log::warn!("save_state_on_exit: read settings.json: {e}"),
    }

    // Per-view column widths + visibility flush into views.json. The
    // column-width drag clamps to per-column min/max in
    // `track-list-header.slint`, so the persisted values are always in
    // range. Other view-state fields (sort, browse path, nav index,
    // detail ids, collapse toggles) are written eagerly by their own
    // callbacks during the session; only the column state needs a
    // shutdown snapshot.
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

    // Bound the entire shutdown wait. Most tasks honour `shutdown_token`
    // and exit on the next select! tick (≤ 500 ms), but a few do
    // non-cancellable blocking work behind `spawn_blocking` —
    // `first_launch::run` can be deep in a parallel scan / DB ingest when
    // the user closes the window, and `material_you` may be in a
    // `spawn_blocking` quantize pass. Worse, `db.close()` itself waits for
    // outstanding connections to be returned to the pool, so a
    // still-running task holding a connection across an await pins the
    // close indefinitely. Both operations therefore live inside a single
    // 3 s budget — past that we force-exit regardless of what tokio thinks
    // is pending.
    runtime.block_on(async move {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tracker.wait().await;
            db.close().await;
        })
        .await
        .is_ok()
    })
}

/// Drop the runtime in a background thread so it can't extend our exit
/// path. The runtime's `Drop` waits for worker-thread join, which can
/// itself block on `Drop`s of audio / D-Bus / accesskit threads we don't
/// control.
pub fn drop_runtime_in_background(runtime: tokio::runtime::Runtime) {
    let res = std::thread::Builder::new()
        .name("melodia-runtime-drop".into())
        .spawn(move || {
            drop(runtime);
        });
    if let Err(e) = res {
        log::warn!("could not spawn runtime-drop thread: {e}");
    }
}

/// If the user confirmed a titlebar-mode restart from Settings, or the
/// auto-updater's "Restart Now" was clicked, the dialog / callback handler
/// in `ui::window_chrome` left a respawn flag set. Replace the current
/// process with a fresh copy of the binary so the old window goes away
/// the same instant the new one comes up — `execvp` semantics, no
/// fork-then-exit race on the shared screen.
///
/// The auto-updater path also records an explicit binary path
/// (`respawn_exe`) captured before the install swapped the binary on disk.
/// We prefer it over `current_exe()`, which by this point resolves to the
/// stale `<target>.old` (atomic swap) or a `(deleted)` path (RPM/DEB) —
/// respawning from that relaunches the old binary or fails outright. The
/// titlebar restart records no path (no install happened), so its
/// `current_exe()` fallback stays correct.
///
/// **Unix** — uses `CommandExt::exec` (`execvp`). This atomically
/// replaces the running process with the new binary: same PID, same
/// fds, same env, same cwd, same process group / session. None of the
/// "parent exits → child gets SIGHUP / broken pipe / orphan reap"
/// failure modes apply, which were what made `spawn` + `process::exit`
/// flaky under KDE/Plasma's `kio-launcher` — the launcher wires the
/// app's stdio to a journald pipe and keeps the child in its own
/// process group, so the parent's exit was racing the freshly-installed
/// child's first paint. `exec` only returns on failure; on success
/// control never comes back to this caller.
///
/// **Windows / exec failure** — falls through to `spawn_detached`,
/// which sets `Stdio::null` for all three streams and (on Unix) puts
/// the child in its own process group via `process_group(0)`. Even if
/// the parent's stdio pipe goes EOF or its process group receives a
/// signal, the detached child stays alive.
pub fn respawn_if_requested() {
    if !ui::window_chrome::should_respawn_after_exit() {
        return;
    }
    let exe = match ui::window_chrome::respawn_exe()
        .map_or_else(std::env::current_exe, Ok)
    {
        Ok(exe) => exe,
        Err(e) => {
            log::warn!("current_exe lookup failed during respawn: {e}");
            return;
        }
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

    // Reached only on exec failure (Unix) or non-Unix targets — see
    // the `Windows / exec failure` paragraph in the function docs.
    if let Err(e) = spawn_detached(&exe) {
        log::warn!("respawn spawn failed for {}: {e}", exe.display());
    }
}

/// Spawn a detached child running `exe`, with all three stdio streams
/// redirected to `/dev/null` (or the platform equivalent). On Unix the
/// child is placed in its own process group via `process_group(0)` so
/// any signal sent to the parent's process group after `process::exit`
/// can't take it down. Windows inherits no such handoff problems — no
/// special detachment needed beyond what `Command::spawn` already gives.
fn spawn_detached(exe: &std::path::Path) -> std::io::Result<std::process::Child> {
    let mut cmd = std::process::Command::new(exe);
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
