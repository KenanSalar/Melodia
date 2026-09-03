use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[cfg(target_os = "linux")]
pub mod gnome;
#[cfg(target_os = "linux")]
pub mod kwin;

/// `Copy` because every variant is a unit one: it is a detected capability passed by value to
/// whatever acts on it, and a `&AlwaysOnTopCapability` in hand should not force a clone to read
/// which arm it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlwaysOnTopMethod {
    Native,
    #[cfg(target_os = "linux")]
    KwinDbus,
    #[cfg(target_os = "linux")]
    GnomeExtension,
    #[serde(other)]
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlwaysOnTopCapability {
    pub supported: bool,
    pub reason: Option<String>,
    pub method: AlwaysOnTopMethod,
    pub use_native_decorations: bool,
}

impl AlwaysOnTopCapability {
    fn native() -> Self {
        Self::supported(AlwaysOnTopMethod::Native)
    }

    fn supported(method: AlwaysOnTopMethod) -> Self {
        Self {
            supported: true,
            reason: None,
            method,
            use_native_decorations: false,
        }
    }

    #[cfg(target_os = "linux")]
    fn unsupported(reason: &str) -> Self {
        Self {
            supported: false,
            reason: Some(reason.to_owned()),
            method: AlwaysOnTopMethod::Unsupported,
            use_native_decorations: false,
        }
    }
}

/// Detect always-on-top capability for the current platform.
/// Called once at startup and cached on `AppState`. The matching `apply`
/// dispatcher uses the cached method to route to the right backend.
#[cfg(not(target_os = "linux"))]
pub fn detect_capability() -> AlwaysOnTopCapability {
    AlwaysOnTopCapability::native()
}

#[cfg(target_os = "linux")]
pub fn detect_capability() -> AlwaysOnTopCapability {
    detect_capability_linux()
}

#[cfg(target_os = "linux")]
fn detect_capability_linux() -> AlwaysOnTopCapability {
    let session_type = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();

    if session_type != "wayland" && std::env::var("WAYLAND_DISPLAY").is_err() {
        return AlwaysOnTopCapability::native();
    }

    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let desktop_upper = desktop.to_uppercase();

    // Single blocking D-Bus connection shared across startup probes AND
    // every later apply call. Per CLAUDE.md, zbus's tokio feature must stay
    // off — the blocking API is fine here because detection runs once on
    // the main thread before the runtime is up, and the apply path drops
    // to `spawn_blocking` before reusing this same pooled connection.
    let Ok(conn) = session_connection() else {
        return AlwaysOnTopCapability::unsupported("D-Bus session bus not available");
    };

    if desktop_upper.contains("KDE") {
        if probe_dbus_service(&conn, "org.kde.KWin") {
            return AlwaysOnTopCapability::supported(AlwaysOnTopMethod::KwinDbus);
        }
        return AlwaysOnTopCapability::unsupported("KWin D-Bus service not available");
    }

    if desktop_upper.contains("GNOME") {
        if probe_gnome_window_calls(&conn) {
            return AlwaysOnTopCapability::supported(AlwaysOnTopMethod::GnomeExtension);
        }
        // No extension available — fall back to native decorations
        // so the user can right-click the title bar for "Always on Top"
        return AlwaysOnTopCapability {
            supported: false,
            reason: None,
            method: AlwaysOnTopMethod::Unsupported,
            use_native_decorations: true,
        };
    }

    AlwaysOnTopCapability::unsupported("Always-on-top is not supported on this Wayland compositor")
}

/// Apply the user's pinned choice. The `Native` branch is intentionally a
/// no-op here — winit's `Window::set_window_level` must run on the UI
/// thread, so callbacks.rs handles that synchronously before invoking this.
/// The Linux branches do D-Bus work via the per-backend modules (each one
/// already drops to `spawn_blocking`, so calling this from the runtime is
/// fine). `Unsupported` returns an error so the UI can revert its
/// optimistic toggle.
///
/// Takes the two answers it needs rather than an `AppState`: the detected `method` and, for the
/// `KwinDbus` arm, the directory its script is written into. Both callers hold one and can spell
/// `state.always_on_top.method` themselves, and a platform adapter that names the app's whole
/// state is the edge this directory exists on the far side of.
#[cfg_attr(
    not(target_os = "linux"),
    allow(
        unused_variables,
        clippy::unused_async,
        reason = "`pinned` is consumed only by the KwinDbus + GnomeExtension Linux match arms; the same arms are the only ones that await — non-Linux falls through to the synchronous Native/Unsupported branches"
    )
)]
pub async fn apply(
    method: AlwaysOnTopMethod,
    data_dir: &std::path::Path,
    pinned: bool,
) -> Result<(), AppError> {
    match method {
        AlwaysOnTopMethod::Native => Ok(()),
        #[cfg(target_os = "linux")]
        AlwaysOnTopMethod::KwinDbus => kwin::set_always_on_top(data_dir, pinned).await,
        #[cfg(target_os = "linux")]
        AlwaysOnTopMethod::GnomeExtension => gnome::set_always_on_top(pinned).await,
        AlwaysOnTopMethod::Unsupported => {
            Err(AppError::Window("Always-on-top is not supported on this desktop".to_owned()))
        }
    }
}

/// Lazily-initialized, shared D-Bus session connection used by the Linux
/// backends. `zbus::blocking::Connection` is internally Arc-backed so the
/// clone is cheap. Held behind a `parking_lot::Mutex` rather than an
/// async lock because every caller is already inside
/// `tokio::task::spawn_blocking` (per the CLAUDE.md zbus footgun: the
/// async `zbus::Connection` would pull in zbus's tokio feature and
/// collide with Slint's accesskit-transitive zbus chain).
#[cfg(target_os = "linux")]
static SESSION_CONN: parking_lot::Mutex<Option<zbus::blocking::Connection>> =
    parking_lot::Mutex::new(None);

#[cfg(target_os = "linux")]
pub(crate) fn session_connection() -> Result<zbus::blocking::Connection, AppError> {
    let mut guard = SESSION_CONN.lock();
    if let Some(c) = guard.as_ref() {
        return Ok(c.clone());
    }
    let conn = zbus::blocking::Connection::session()
        .map_err(|e| AppError::Window(format!("D-Bus connection failed: {e}")))?;
    *guard = Some(conn.clone());
    Ok(conn)
}

#[cfg(target_os = "linux")]
fn probe_dbus_service(conn: &zbus::blocking::Connection, name: &str) -> bool {
    let result = (|| {
        let proxy = zbus::blocking::fdo::DBusProxy::new(conn)?;
        let names = proxy.list_names()?;
        Ok::<bool, zbus::Error>(names.iter().any(|n| n.as_str() == name))
    })();
    result.unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn probe_gnome_window_calls(conn: &zbus::blocking::Connection) -> bool {
    conn.call_method(
        Some("org.gnome.Shell"),
        "/org/gnome/Shell/Extensions/Windows",
        Some("org.gnome.Shell.Extensions.Windows"),
        "List",
        &(),
    )
    .is_ok()
}
