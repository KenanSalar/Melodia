//! Visualizer-section setters. Unlike their audio siblings there is no runtime
//! half to pair with: the flags only decide whether the Now-Playing strip
//! mounts and which style it draws, and the Slint bindings have already applied
//! that by the time these run. Arming the sample tap follows from the
//! strip being on screen — see [`crate::ui::visualizer`].

use crate::error::AppError;
use crate::services;
use crate::state::AppState;

/// Persist the visualizer on/off toggle. Unlike the other audio features this
/// one defaults to *on* (`VisualizerFlags::default()`) — see its doc for why —
/// so this write is what records a user turning it off.
pub fn set_visualizer_enabled(state: &AppState, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.visualizer.viz_enabled = enabled;
    })
}

/// Persist the chosen visualizer style, by key. The caller resolves the key
/// from its style table (`crate::ui::visualizer`), so nothing unrecognized
/// reaches the file.
pub fn set_visualizer_style(state: &AppState, style: String) -> Result<(), AppError> {
    services::settings::mutate_settings(&state.paths, move |settings| {
        settings.visualizer.viz_style = style;
    })
}
