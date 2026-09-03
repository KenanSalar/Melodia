//! `KWin` backend for always-on-top. `KWin` has no direct D-Bus method to
//! set `keepAbove` on a window, so we ship a tiny script through the
//! `/Scripting` interface that walks `workspace.stackingOrder`, matches
//! the client by PID, and flips its `keepAbove` flag. The Tauri version
//! did exactly this and the sequence — write tmp script → loadScript →
//! run → stop → unloadScript → delete tmp — is preserved verbatim.
//!
//! We use `zbus::blocking::*` exclusively. Enabling zbus's `tokio`
//! feature would unify Slint's accesskit-transitive zbus chain and
//! panic the a11y thread at startup (see CLAUDE.md "zbus footgun" +
//! `MEMORY.md/zbus_slint_tokio_conflict.md`). To avoid blocking the
//! tokio runtime, the whole body runs inside `tokio::task::spawn_blocking`.

use zbus::zvariant::ObjectPath;

use super::session_connection;
use crate::error::AppError;

pub async fn set_always_on_top(data_dir: &std::path::Path, pinned: bool) -> Result<(), AppError> {
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
        let script_path_str = script_path.to_string_lossy().into_owned();

        let conn = session_connection()?;

        let scripting_path = ObjectPath::try_from("/Scripting")
            .map_err(|e| AppError::Window(format!("Invalid object path: {e}")))?;

        let _ = conn.call_method(
            Some("org.kde.KWin"),
            &scripting_path,
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &("melodia_pin",),
        );

        let reply = conn
            .call_method(
                Some("org.kde.KWin"),
                &scripting_path,
                Some("org.kde.kwin.Scripting"),
                "loadScript",
                &(&script_path_str, "melodia_pin"),
            )
            .map_err(|e| AppError::Window(format!("Failed to load KWin script: {e}")))?;

        let script_id: i32 = reply
            .body()
            .deserialize()
            .map_err(|e| AppError::Window(format!("Invalid script ID from KWin: {e}")))?;

        let script_obj_path_str = format!("/Scripting/Script{script_id}");
        let script_obj_path = ObjectPath::try_from(script_obj_path_str.as_str())
            .map_err(|e| AppError::Window(format!("Invalid script path: {e}")))?;

        // KWin runs trivial scripts synchronously; matches kdotool's pattern
        // of calling stop() right after run().
        conn.call_method(
            Some("org.kde.KWin"),
            &script_obj_path,
            Some("org.kde.kwin.Script"),
            "run",
            &(),
        )
        .map_err(|e| AppError::Window(format!("Failed to run KWin script: {e}")))?;

        let _ = conn.call_method(
            Some("org.kde.KWin"),
            &script_obj_path,
            Some("org.kde.kwin.Script"),
            "stop",
            &(),
        );

        let _ = conn.call_method(
            Some("org.kde.KWin"),
            &scripting_path,
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &("melodia_pin",),
        );

        let _ = std::fs::remove_file(&script_path);

        Ok(())
    })
    .await
    .map_err(|e| AppError::Window(format!("KWin pin task panicked: {e}")))?
}
