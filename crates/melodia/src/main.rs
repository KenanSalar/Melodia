// Otherwise Windows opens a console beside the window and ties the app's lifetime to it. Debug
// builds keep one for `RUST_LOG`. A parent's `Stdio::piped()` still captures stdout, which the
// updater's smoke test relies on.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod boot;
mod shutdown;

use melodia_app::services::settings::SettingsData;
use melodia_app::state::AppState;
use melodia_app::{library, services, tasks};
use melodia_core::config::Paths;
use melodia_core::error::{AppError, AppResult, describe};
use melodia_core::utils;
use melodia_platform::services::platform;
use melodia_ui::AppWindow;
use melodia_views::ui;
use slint::ComponentHandle;

/// Rayon's global pool, where jpeg-decoder runs every JPEG decoded outside a pool. Two keeps a
/// cover decode parallel without a worker per core idling until quit; a scan brings its own pool.
const GLOBAL_RAYON_THREADS: usize = 2;

fn main() -> AppResult<()> {
    // The updater's smoke test runs the swapped-in binary with this and wants exit 0 and a
    // `Melodia ` prefix, so dropping either breaks in-place updates for every older client. Ahead
    // of `mallopt`, which shapes a long-lived process and would only slow the check.
    if std::env::args().nth(1).as_deref() == Some("--version") {
        use std::io::Write;
        // Not `println!`, which `clippy::print_stdout` denies. A failed write fails the
        // verifier's prefix check anyway.
        let _ = writeln!(std::io::stdout().lock(), "Melodia {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // Ahead of anything that can fail: the rest of diagnostics sits behind Settings, out of reach
    // when Melodia won't open. Linux and macOS only, a Windows release build having no console,
    // so `README.md` and the issue template point Windows at `%APPDATA%\Melodia\logs\`.
    if std::env::args().nth(1).as_deref() == Some("--logs") {
        use std::io::Write;
        let paths = Paths::resolve()?;
        let _ = writeln!(std::io::stdout().lock(), "{}", paths.logs_dir.display());
        return Ok(());
    }

    prepare_process();

    // Ahead of the logger, which writes under it, so a failure here has nowhere to be logged.
    let paths = Paths::resolve()?;

    // Ahead of the logger, so a forwarding launch never opens the shared file, and of everything
    // expensive, so forwarding costs a socket write and a return.
    let startup_files = platform::single_instance::audio_files_from_argv();
    let mut unenforced_reason = None;
    let file_open_listener = match platform::single_instance::claim(&paths.data_dir, &startup_files)
    {
        platform::single_instance::Claim::Secondary => return Ok(()),
        platform::single_instance::Claim::Primary(listener) => Some(listener),
        platform::single_instance::Claim::Unenforced(e) => {
            unenforced_reason = Some(e);
            None
        }
    };

    install_diagnostics(&paths, unenforced_reason);
    let runtime = build_runtime()?;

    // Held while the event loop runs: Slint's a11y thread finds its tokio reactor through it.
    let runtime_guard = runtime.enter();

    let (state, channels) = runtime.block_on(AppState::init(paths, runtime.handle().clone()))?;

    log::info!(
        "AppState initialized: db ok, watcher built, media controls = {}",
        if state.media_controls.is_some() { "ready" } else { "no-op" }
    );

    let spawner = tasks::TaskSpawner::from_state(&state);
    boot::tasks::spawn_background_tasks(&spawner, &state, channels);
    boot::tasks::restore_persisted_playback(&runtime, &state);

    let startup_settings: Option<services::settings::SettingsData> =
        library::settings::get_settings(&state.paths).ok();
    let startup_view_state: Option<services::view_state::ViewStateData> =
        library::settings::get_view_state(&state.paths).ok();

    // Files on the command line replace the restored queue and play, so resume
    // would only be visible for the moment it takes them to land.
    if startup_files.is_empty() {
        boot::tasks::maybe_resume_on_startup(&state, startup_settings.as_ref());
    } else {
        boot::tasks::open_startup_files(&runtime, &state, &startup_files);
    }

    boot::tasks::spawn_resume_watching(&spawner, &state);

    let app = open_window(startup_settings.as_ref())?;
    boot::ui_setup::install_ui(
        &app,
        &state,
        &spawner,
        startup_settings.as_ref(),
        startup_view_state.as_ref(),
        file_open_listener,
    )?;

    // Not `app.run()`, whose loop ends once the last window is hidden, which close-to-tray does.
    app.show().map_err(|e| AppError::io("show window", e))?;
    slint::run_event_loop_until_quit().map_err(|e| AppError::io("event loop", e))?;

    log::info!("Melodia shutting down — flushing player state");
    shutdown::save_state_on_exit(&app, &state, &runtime);

    // Before `process::exit(0)` skips destructors: an exclusive claim hands the device back with
    // the volume it had, rather than leaving it at Melodia's.
    state.engine.close_output();

    // Likewise, or `tray-icon` ghosts in the Windows notification area. On the main thread, the
    // Win/mac handle being `!Send`; Linux's drops with its subscriber task.
    ui::shell::tray_bridge::shutdown();

    log::info!("Melodia shutting down — signalling tasks");
    let shutdown_completed = shutdown::flush_tasks_and_db(&runtime, state);
    if shutdown_completed {
        log::info!("All background tasks completed; exiting");
    } else {
        log::warn!(
            "Background shutdown did not finish within 3s — forcing exit. \
             Pending blocking work (scan, retroactive hash, …) is abandoned; \
             persisted state is already flushed by save_state_on_exit."
        );
    }

    // `!Send`, so it can't ride into the background drop thread.
    drop(runtime_guard);

    shutdown::drop_runtime_in_background(runtime);

    // Neither the respawn's `exec` nor the `process::exit` below runs a destructor.
    platform::logging::flush();

    shutdown::respawn_if_requested();

    // A normal return waits on threads that never exit: accesskit's a11y thread and any tokio
    // worker parked on a blocking call.
    std::process::exit(0);
}

/// What has to happen before the first thread starts or the first arena is created.
fn prepare_process() {
    // The rollback copy `swap_in_place` keeps, which reaching this launch proves unneeded.
    // msiexec never leaves one.
    #[cfg(target_os = "linux")]
    {
        if let Ok(stale) = services::updater::install_target_old() {
            let _ = std::fs::remove_file(stale);
        }
    }
    // Ahead of the logger and the runtime, both of which allocate on threads of their own.
    platform::allocator::pin_arenas_and_thresholds();

    // Names the PipeWire node the default ALSA PCM becomes, which mixers otherwise list as
    // `alsa_playback.<prgname>`. Ignored on bare ALSA.
    #[cfg(target_os = "linux")]
    #[allow(unsafe_code, reason = "env::set_var is unsafe in Rust 2024")]
    // SAFETY: `set_var` requires that no other thread reads or writes the environment, and none
    // exists yet: the logger, the runtime and Slint all come later.
    unsafe {
        std::env::set_var(
            "PIPEWIRE_ALSA",
            "{ application.name = \"Melodia\" node.name = \"Melodia\" }",
        );
    }
}

/// The rolling log and the crash hook, then the startup facts only this launch knows.
fn install_diagnostics(paths: &Paths, unenforced_reason: Option<std::io::Error>) {
    // An unreadable settings file starts quiet; `AppState::init`'s own read reports it.
    let verbose_logging = services::settings::read_settings(paths)
        .is_ok_and(|settings| settings.diagnostics.verbose_logging);
    platform::logging::install(paths, verbose_logging);
    // Before the runtime and before Slint, so boot panics are covered too.
    platform::crash_report::install_hook(&paths.logs_dir);
    log::info!("Melodia starting");
    // A dev build and `MELODIA_DATA_DIR` both move the root, and nothing downstream says which
    // this boot took. The diagnostics bundle carries it too; this is a live tail's copy.
    log::info!("data directory: {}", utils::redact::redact_home(&paths.data_dir.to_string_lossy()));
    // The claim happened before there was anywhere to say this.
    if let Some(e) = unenforced_reason {
        log::warn!(
            "single_instance: not enforced ({}); a second launch will open a second window",
            describe(&e)
        );
    }
}

/// The tokio runtime, and rayon's global pool beside it.
fn build_runtime() -> AppResult<tokio::runtime::Runtime> {
    // The async work is event-driven, and CPU-bound work goes to rayon or `spawn_blocking`,
    // neither of which draws on these. One per core would idle, each holding a 2 MB stack.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        // A burst ceiling: blocking threads spawn on demand and are reaped. The one permanent
        // tenant is `system_theme::spawn_color_watcher`.
        .max_blocking_threads(32)
        .enable_all()
        .thread_name("melodia-bg")
        .build()
        .map_err(|e| AppError::io("tokio runtime", e))?;

    // Ahead of the first decode: rayon fixes the global pool's shape at first use.
    if let Err(e) = rayon::ThreadPoolBuilder::new()
        .num_threads(GLOBAL_RAYON_THREADS)
        .thread_name(|i| format!("rayon-{i}"))
        .build_global()
    {
        log::warn!("rayon global pool: {}; rayon will size its own", describe(&e));
    }
    Ok(runtime)
}

/// The main window on the winit backend, at the persisted geometry and not yet shown.
fn open_window(settings: Option<&SettingsData>) -> AppResult<AppWindow> {
    // Maximized rides the attributes hook, Slint having no API for it, so the window is created
    // maximized with no flash.
    let geometry = settings.map_or_else(
        ui::window_chrome::geometry::PersistedGeometry::fallback,
        ui::window_chrome::geometry::PersistedGeometry::from_settings,
    );
    select_backend(geometry.maximized)?;

    let app = AppWindow::new().map_err(|e| AppError::io("main window", e))?;

    // Before `app.show()`: `set_size` sets winit's `has_explicit_size`, which stops Slint snapping
    // the window to its preferred size on first show.
    ui::window_chrome::geometry::restore(
        &app,
        geometry,
        settings.and_then(|s| s.full_player_geometry),
    );
    Ok(app)
}

fn select_backend(restore_maximized: bool) -> AppResult<()> {
    let backend = slint::BackendSelector::new()
        .backend_name("winit".into())
        .with_winit_window_attributes_hook(move |attrs| {
            let attrs = if restore_maximized { attrs.with_maximized(true) } else { attrs };
            // How the compositor finds our icon and label. X11 matches `WM_CLASS` against the
            // desktop file's `StartupWMClass`; Wayland matches `app_id` to a desktop file of the
            // same basename, so it has to be the reverse-DNS id we install under.
            #[cfg(target_os = "linux")]
            let attrs = {
                use slint::winit_030::winit::platform::wayland::WindowAttributesExtWayland;
                use slint::winit_030::winit::platform::x11::WindowAttributesExtX11;
                let attrs = WindowAttributesExtX11::with_name(attrs, "Melodia", "Melodia");
                WindowAttributesExtWayland::with_name(
                    attrs,
                    "com.github.kenansalar.melodia",
                    "com.github.kenansalar.melodia",
                )
            };
            attrs
        });
    // Counts the loop's `NewEvents`, which is how the pump tells a Win32 drag's modal loop apart.
    #[cfg(target_os = "windows")]
    let backend =
        backend.with_winit_custom_application_handler(ui::window_chrome::parked_loop::LoopTicks);
    backend.select().map_err(|e| AppError::io("backend selector", e))
}

#[cfg(test)]
#[path = "tests/main_order_tests.rs"]
mod tests;
