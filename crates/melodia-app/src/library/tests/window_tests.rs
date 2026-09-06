//! The two window setters that write more, or less, than the field they are named for.
//!
//! Both are invisible when wrong. The titlebar toggle turns a second setting on that the user
//! never touched, and always-on-top persists only if the desktop actually honoured the pin — the
//! caller reverts its own optimistic toggle off that error, so a write that happened anyway leaves
//! the UI and the file disagreeing until the next launch.

use std::sync::Arc;

use crate::services;
use crate::state::fixtures::{seeded_root, seeded_root_with};
use melodia_core::error::AppError;
use melodia_platform::services::platform::always_on_top::AlwaysOnTopMethod;

use super::{apply_then_persist, write_use_native_titlebar};

/// KDE's unfocused tint mirrors its own window-decoration fade, so it ships on the moment the
/// native titlebar does. It is still a write the user did not ask for, and both fields have to
/// land together — the respawned process reads the pair, not one and then the other.
#[test]
fn enabling_the_native_titlebar_under_kde_turns_the_unfocused_tint_on_too() -> Result<(), AppError>
{
    let (_tmp, paths) = seeded_root()?;

    write_use_native_titlebar(&paths, true, true)?;

    let settings = services::settings::read_settings(&paths)?;
    assert!(settings.window.use_native_titlebar);
    assert!(settings.layout.match_unfocused_to_system_bg);
    Ok(())
}

#[test]
fn enabling_it_anywhere_else_writes_only_the_titlebar() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|s| s.layout.match_unfocused_to_system_bg = false)?;

    write_use_native_titlebar(&paths, true, false)?;

    let settings = services::settings::read_settings(&paths)?;
    assert!(settings.window.use_native_titlebar);
    assert!(!settings.layout.match_unfocused_to_system_bg);
    Ok(())
}

/// Going back to the custom titlebar leaves the tint alone rather than clearing it. The Slint
/// binding sites already suppress it in that mode, so clearing would silently discard a choice the
/// user gets back the moment they re-enable the native titlebar.
#[test]
fn disabling_the_native_titlebar_leaves_the_tint_where_it_was() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|s| s.layout.match_unfocused_to_system_bg = true)?;

    write_use_native_titlebar(&paths, false, true)?;

    let settings = services::settings::read_settings(&paths)?;
    assert!(!settings.window.use_native_titlebar);
    assert!(settings.layout.match_unfocused_to_system_bg, "the persisted tint survives the trip");
    Ok(())
}

/// A desktop with no supported method has to fail before the write. The caller reverts its
/// optimistic UI toggle off this error, so a file that recorded the pin anyway would restore a
/// window state the desktop never applied.
#[tokio::test]
async fn a_desktop_that_cannot_pin_reports_it_and_persists_nothing() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|s| s.window.always_on_top = false)?;
    let paths = Arc::new(paths);

    let refused = apply_then_persist(&paths, AlwaysOnTopMethod::Unsupported, true).await;

    assert!(matches!(refused, Err(AppError::Window(_))));
    assert!(!services::settings::read_settings(&paths)?.window.always_on_top);
    Ok(())
}

#[tokio::test]
async fn a_pin_the_desktop_accepts_is_persisted() -> Result<(), AppError> {
    let (_tmp, paths) = seeded_root_with(|s| s.window.always_on_top = false)?;
    let paths = Arc::new(paths);

    apply_then_persist(&paths, AlwaysOnTopMethod::Native, true).await?;

    assert!(services::settings::read_settings(&paths)?.window.always_on_top);
    Ok(())
}
