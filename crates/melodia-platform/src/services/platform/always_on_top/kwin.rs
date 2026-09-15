//! `KWin` backend for always-on-top. `KWin` has no direct D-Bus method to
//! set `keepAbove` on a window, so both directions go through the
//! `/Scripting` interface. [`set_always_on_top`] ships a one-shot script that
//! walks `workspace.stackingOrder`, matches the client by PID, and flips its
//! `keepAbove` flag. [`watch_keep_above`] leaves one running that reports
//! every change to that flag back, whoever made it.
//!
//! We use `zbus::blocking::*` exclusively. Enabling zbus's `tokio`
//! feature would unify Slint's accesskit-transitive zbus chain and
//! panic the a11y thread at startup (see CLAUDE.md "zbus footgun" +
//! `MEMORY.md/zbus_slint_tokio_conflict.md`). To avoid blocking the
//! tokio runtime, the whole body runs inside `tokio::task::spawn_blocking`.

use std::path::Path;

use tokio::sync::watch;
use zbus::interface;
use zbus::object_server::Interface;
use zbus::zvariant::ObjectPath;

use super::session_connection;
use melodia_core::error::AppError;

const KWIN_SERVICE: &str = "org.kde.KWin";
const SCRIPTING_PATH: ObjectPath<'static> = ObjectPath::from_static_str_unchecked("/Scripting");
const SCRIPTING_INTERFACE: &str = "org.kde.kwin.Scripting";
const SCRIPT_INTERFACE: &str = "org.kde.kwin.Script";
const PIN_PLUGIN: &str = "melodia_pin";

/// Where the watcher script calls back, on the connection that loaded it.
const REPORT_PATH: &str = "/com/github/kenansalar/Melodia/KeepAbove";
const REPORT_METHOD: &str = "Changed";

pub async fn set_always_on_top(data_dir: &Path, pinned: bool) -> Result<(), AppError> {
    let pid = std::process::id();
    let script_path = data_dir.join("kwin_pin.js");

    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let script_content = format!(
            r"var clients = workspace.stackingOrder;
for (var i = 0; i < clients.length; i++) {{
    var w = clients[i];
    if (w.pid === {pid}) {{
        w.keepAbove = {pinned};
    }}
}}",
        );

        std::fs::write(&script_path, &script_content)
            .map_err(|e| AppError::Window(format!("Failed to write KWin script: {e}")))?;

        let conn = session_connection()?;
        let script_obj_path = load_and_run(&conn, &script_path, PIN_PLUGIN)?;

        // Stopped and unloaded straight after `run`. `KWin` reads the file on a worker thread once
        // `run` returns and a stop deletes the script with it, so this leans on a one-line read
        // beating the next two round trips.
        let _ = conn.call_method(
            Some(KWIN_SERVICE),
            &script_obj_path,
            Some(SCRIPT_INTERFACE),
            "stop",
            &(),
        );
        unload(&conn, PIN_PLUGIN);

        let _ = std::fs::remove_file(&script_path);

        Ok(())
    })
    .await
    .map_err(|e| AppError::Window(format!("KWin pin task panicked: {e}")))?
}

/// Leave a script running in `KWin` that sends every change to our window's `keepAbove` to
/// `reports`. Nothing else tells a client when `KWin`'s own keep-above titlebar button moves it.
pub async fn watch_keep_above(
    data_dir: &Path,
    reports: watch::Sender<bool>,
) -> Result<(), AppError> {
    let script_path = data_dir.join("kwin_keep_above.js");
    let plugin = keep_above_plugin(data_dir);

    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let conn = session_connection()?;
        let bus_name = conn
            .unique_name()
            .ok_or_else(|| AppError::Window("D-Bus connection has no unique name".to_owned()))?
            .to_string();

        conn.object_server()
            .at(REPORT_PATH, KeepAboveReports { reports })
            .map_err(|e| AppError::Window(format!("Failed to serve keep-above reports: {e}")))?;

        // The file stays: `KWin` reads it on a worker thread after `run` returns, and this script
        // is never stopped to make the wait worth it.
        std::fs::write(&script_path, keep_above_script(std::process::id(), &bus_name))
            .map_err(|e| AppError::Window(format!("Failed to write KWin script: {e}")))?;

        load_and_run(&conn, &script_path, &plugin).map(drop)
    })
    .await
    .map_err(|e| AppError::Window(format!("KWin keep-above watch task panicked: {e}")))?
}

/// The receiving end of the watcher script's `callDBus`.
struct KeepAboveReports {
    reports: watch::Sender<bool>,
}

// `spawn = false` because zbus otherwise hands each call its own task, and two quick toggles can
// then land in the wrong order and leave the button on the state `KWin` left behind.
#[interface(name = "com.github.kenansalar.Melodia.KeepAbove", spawn = false)]
impl KeepAboveReports {
    /// Spelled out rather than derived, [`REPORT_METHOD`] being the script's copy.
    #[zbus(name = "Changed")]
    fn changed(&self, keep_above: bool) {
        self.reports.send_replace(keep_above);
    }
}

/// `windowAdded` is Plasma 6's name for Plasma 5's `clientAdded`. Tracking new windows matters
/// beyond startup: a hide to the tray unmaps ours, and showing it again adds a fresh one.
fn keep_above_script(pid: u32, bus_name: &str) -> String {
    let interface = KeepAboveReports::name();
    format!(
        r#"function track(window) {{
    if (window.pid !== {pid}) {{
        return;
    }}
    window.keepAboveChanged.connect(function () {{
        callDBus("{bus_name}", "{REPORT_PATH}", "{interface}", "{REPORT_METHOD}", window.keepAbove);
    }});
}}
var windows = workspace.stackingOrder;
for (var i = 0; i < windows.length; i++) {{
    track(windows[i]);
}}
(workspace.windowAdded || workspace.clientAdded).connect(track);"#
    )
}

/// Named per data root, so a relaunch replaces the script its previous run left loaded while a
/// second Melodia on another root keeps its own.
fn keep_above_plugin(data_dir: &Path) -> String {
    let digest = blake3::hash(data_dir.as_os_str().as_encoded_bytes());
    format!("melodia_keep_above_{}", &digest.to_hex()[..16])
}

/// Load `script_path` as `plugin`, replacing a script already loaded under that name, and start
/// it. Returns the script's object path.
fn load_and_run(
    conn: &zbus::blocking::Connection,
    script_path: &Path,
    plugin: &str,
) -> Result<ObjectPath<'static>, AppError> {
    unload(conn, plugin);

    let script_path_str = script_path.to_string_lossy().into_owned();
    let reply = conn
        .call_method(
            Some(KWIN_SERVICE),
            &SCRIPTING_PATH,
            Some(SCRIPTING_INTERFACE),
            "loadScript",
            &(&script_path_str, plugin),
        )
        .map_err(|e| AppError::Window(format!("Failed to load KWin script: {e}")))?;

    let script_id: i32 = reply
        .body()
        .deserialize()
        .map_err(|e| AppError::Window(format!("Invalid script ID from KWin: {e}")))?;

    let script_obj_path = ObjectPath::try_from(format!("/Scripting/Script{script_id}"))
        .map_err(|e| AppError::Window(format!("Invalid script path: {e}")))?;

    conn.call_method(Some(KWIN_SERVICE), &script_obj_path, Some(SCRIPT_INTERFACE), "run", &())
        .map_err(|e| AppError::Window(format!("Failed to run KWin script: {e}")))?;

    Ok(script_obj_path)
}

/// Unload the script loaded as `plugin`, if there is one.
fn unload(conn: &zbus::blocking::Connection, plugin: &str) {
    let _ = conn.call_method(
        Some(KWIN_SERVICE),
        &SCRIPTING_PATH,
        Some(SCRIPTING_INTERFACE),
        "unloadScript",
        &(plugin,),
    );
}

#[cfg(test)]
#[path = "tests/kwin_tests.rs"]
mod tests;
