//! Visualizer-section setters, with no engine half: arming the tap follows from the strip being on
//! screen (`ui::visualizer`).

use crate::services;
use melodia_core::config::Paths;
use melodia_core::error::AppError;

pub fn set_visualizer_enabled(paths: &Paths, enabled: bool) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.visualizer.viz_enabled = enabled;
    })
}

/// By key, which the caller resolves from its style table, so nothing unrecognised reaches the
/// file.
pub fn set_visualizer_style(paths: &Paths, style: String) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| {
        settings.visualizer.viz_style = style;
    })
}
