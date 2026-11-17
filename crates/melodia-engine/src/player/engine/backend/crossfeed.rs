//! Which crossfeed every source glides to: the heard device's, off where it has none.

use std::sync::Arc;

use melodia_core::entities::crossfeed::CrossfeedSetting;
use melodia_core::entities::device_map::DeviceMap;
use melodia_playback::player::playback::crossfeed::CrossfeedShared;

use super::PlaybackEngine;

pub(super) struct CrossfeedControl {
    shared: Arc<CrossfeedShared>,
    settings: DeviceMap<CrossfeedSetting>,
}

impl Default for CrossfeedControl {
    fn default() -> Self {
        Self { shared: CrossfeedShared::new(), settings: DeviceMap::default() }
    }
}

impl CrossfeedControl {
    pub(super) fn shared(&self) -> Arc<CrossfeedShared> {
        Arc::clone(&self.shared)
    }

    /// Whether `heard`'s crossfeed is switched on; a source plays it only in stereo.
    pub(super) fn enabled(&self, heard: Option<&str>) -> bool {
        self.setting_for(heard).enabled
    }

    pub(super) fn set_settings(
        &mut self,
        settings: DeviceMap<CrossfeedSetting>,
        heard: Option<&str>,
    ) {
        self.settings = settings;
        self.select(heard);
    }

    /// Publishing an unchanged setting is free: a source ramps only to a target that moved.
    pub(super) fn select(&self, heard: Option<&str>) {
        self.shared.set(self.setting_for(heard));
    }

    fn setting_for(&self, heard: Option<&str>) -> CrossfeedSetting {
        self.settings.for_device(heard).copied().unwrap_or_default()
    }
}

impl PlaybackEngine {
    /// Every device's crossfeed; a change to the heard device's glides in.
    pub fn set_crossfeed(&self, settings: DeviceMap<CrossfeedSetting>) {
        self.headphones.lock().set_crossfeed(settings);
    }
}
