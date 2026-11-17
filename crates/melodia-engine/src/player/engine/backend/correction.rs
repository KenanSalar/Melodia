//! Which headphone correction every source plays: the heard device's, while the switch is on.

use std::sync::Arc;

use melodia_core::entities::correction::{CorrectionFilters, DeviceCorrections};
use melodia_playback::player::playback::correction::shared::{CorrectionShared, CorrectionWriter};

use super::PlaybackEngine;

#[derive(Default)]
pub(super) struct CorrectionControl {
    writer: CorrectionWriter,
    corrections: DeviceCorrections,
    enabled: bool,
    /// `None` is System default.
    heard: Option<String>,
    /// `None` while the writer's set is empty.
    published: Option<CorrectionFilters>,
}

impl CorrectionControl {
    pub(super) fn shared(&self) -> Arc<CorrectionShared> {
        self.writer.shared()
    }

    pub(super) fn aim(&mut self, device: Option<&str>) {
        self.heard = device.map(str::to_owned);
        self.select();
    }

    /// Whether a set plays: the switch on and a correction set for the heard device.
    pub(super) fn engaged(&self) -> bool {
        self.published.is_some()
    }

    fn set_corrections(&mut self, corrections: DeviceCorrections) {
        self.corrections = corrections;
        self.select();
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.select();
    }

    /// Publishes only a change: the same set again would still crossfade, into a bank starting
    /// from rest.
    fn select(&mut self) {
        let wanted = if self.enabled {
            self.corrections.for_device(self.heard.as_deref()).map(|correction| &correction.filters)
        } else {
            None
        };
        if wanted == self.published.as_ref() {
            return;
        }
        match wanted {
            Some(filters) => self.writer.publish(filters),
            None => self.writer.clear(),
        }
        self.published = wanted.cloned();
    }
}

impl PlaybackEngine {
    /// Every device's correction; a change to the heard device's swaps in through a short ramp.
    pub fn set_corrections(&self, corrections: DeviceCorrections) {
        self.correction.lock().set_corrections(corrections);
    }

    /// Off plays every source uncorrected and keeps each device's correction.
    pub fn set_correction_enabled(&self, enabled: bool) {
        self.correction.lock().set_enabled(enabled);
    }

    /// The device whose correction plays, `None` naming System default; it moves once a reopen
    /// has returned.
    pub fn heard_device(&self) -> Option<String> {
        self.correction.lock().heard.clone()
    }
}

#[cfg(test)]
#[path = "tests/correction_tests.rs"]
mod tests;
