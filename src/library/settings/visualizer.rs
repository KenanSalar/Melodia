//! Visualizer-section setter. Unlike its audio siblings there is no runtime
//! half to pair with: the flag only decides whether the Now-Playing strip
//! mounts, and the Slint two-way binding has already applied that by the time
//! this runs. Arming the Rodio sample tap follows from the strip being on
//! screen — see [`crate::ui::visualizer`].

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
