//! Visualizer-section setter. The runtime effect (arming the Rodio sample tap)
//! is done synchronously by the UI callback through
//! `library::playback::player_set_visualizer_enabled` *before* this async disk
//! write, so the bars react immediately and the `settings.json` commit only
//! persists the choice. Mirrors the shape of [`super::equalizer`].

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
