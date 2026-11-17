//! Crossfeed's setter.

use crate::services;
use melodia_core::config::Paths;
use melodia_core::entities::crossfeed::CrossfeedSetting;
use melodia_core::entities::device_map::DeviceMap;
use melodia_core::error::AppError;

/// Every device's crossfeed at once, the map being one key of `settings.json`.
pub fn set_crossfeed(
    paths: &Paths,
    crossfeed: DeviceMap<CrossfeedSetting>,
) -> Result<(), AppError> {
    services::settings::mutate_settings(paths, move |settings| settings.crossfeed = crossfeed)
}
