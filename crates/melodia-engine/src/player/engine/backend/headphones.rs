//! The device the engine plays through, and the settings each device keeps, so one aim moves
//! every one of them to that device's.

use std::sync::Arc;

use melodia_core::entities::correction::Correction;
use melodia_core::entities::device_map::DeviceMap;
use melodia_playback::player::playback::correction::shared::CorrectionShared;

use super::PlaybackEngine;
use super::correction::CorrectionControl;

#[derive(Default)]
pub(super) struct HeadphoneControls {
    /// `None` is System default.
    heard: Option<String>,
    correction: CorrectionControl,
}

impl HeadphoneControls {
    pub(super) fn correction_shared(&self) -> Arc<CorrectionShared> {
        self.correction.shared()
    }

    pub(super) fn aim(&mut self, device: Option<&str>) {
        self.heard = device.map(str::to_owned);
        self.correction.select(device);
    }

    pub(super) fn correction_engaged(&self) -> bool {
        self.correction.engaged()
    }

    pub(super) fn set_corrections(&mut self, corrections: DeviceMap<Correction>) {
        self.correction.set_corrections(corrections, self.heard.as_deref());
    }

    pub(super) fn set_correction_enabled(&mut self, enabled: bool) {
        self.correction.set_enabled(enabled, self.heard.as_deref());
    }
}

impl PlaybackEngine {
    /// The device whose settings play, `None` naming System default; it moves once a reopen has
    /// returned.
    pub fn heard_device(&self) -> Option<String> {
        self.headphones.lock().heard.clone()
    }
}
