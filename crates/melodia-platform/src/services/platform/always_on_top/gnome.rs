//! GNOME backend for always-on-top. GNOME Shell exposes no native
//! always-on-top D-Bus interface; the `window-calls` extension fills the
//! gap with `org.gnome.Shell.Extensions.Windows`. We `List`, pick the
//! window matching our PID (preferring the one whose `wm_class` contains
//! "melodia" so a child dialog isn't pinned by accident), then call
//! `MakeAbove` / `UnmakeAbove`. Capability detection has already
//! confirmed the extension is present.
//!
//! Same threading rules as `kwin.rs`: `zbus::blocking::*` only,
//! everything inside `tokio::task::spawn_blocking`.

use melodia_core::error::AppError;

use super::session_connection;

pub async fn set_always_on_top(pinned: bool) -> Result<(), AppError> {
    let pid = std::process::id();

    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let conn = session_connection()?;

        let reply = conn
            .call_method(
                Some("org.gnome.Shell"),
                "/org/gnome/Shell/Extensions/Windows",
                Some("org.gnome.Shell.Extensions.Windows"),
                "List",
                &(),
            )
            .map_err(|e| AppError::Window(format!("Failed to list windows: {e}")))?;

        let windows_json: String = reply
            .body()
            .deserialize()
            .map_err(|e| AppError::Window(format!("Invalid windows list: {e}")))?;

        let windows: Vec<serde_json::Value> = serde_json::from_str(&windows_json)
            .map_err(|e| AppError::Window(format!("Failed to parse windows JSON: {e}")))?;

        let pid_matches: Vec<&serde_json::Value> = windows
            .iter()
            .filter(|w| w.get("pid").and_then(serde_json::Value::as_u64) == Some(u64::from(pid)))
            .collect();

        // Prefer the window whose `wm_class` contains "melodia"
        // (case-insensitive) — a process can host multiple windows and we
        // must not pin a transient dialog.
        let window = pid_matches
            .iter()
            .find(|w| {
                w.get("wm_class")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.to_lowercase().contains("melodia"))
            })
            .or(pid_matches.first())
            .ok_or_else(|| AppError::Window("Could not find Melodia window in GNOME".to_owned()))?;

        let window_id: u32 = window
            .get("id")
            .and_then(serde_json::Value::as_u64)
            .and_then(|id| u32::try_from(id).ok())
            .ok_or_else(|| AppError::Window("Melodia window has no valid id field".to_owned()))?;

        let method = if pinned { "MakeAbove" } else { "UnmakeAbove" };

        conn.call_method(
            Some("org.gnome.Shell"),
            "/org/gnome/Shell/Extensions/Windows",
            Some("org.gnome.Shell.Extensions.Windows"),
            method,
            &(window_id,),
        )
        .map_err(|e| AppError::Window(format!("Failed to {method} window: {e}")))?;

        Ok(())
    })
    .await
    .map_err(|e| AppError::Window(format!("GNOME pin task panicked: {e}")))?
}
