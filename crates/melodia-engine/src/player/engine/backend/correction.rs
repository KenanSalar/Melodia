//! Which headphone correction every source plays: the heard device's, while the switch is on.

use std::sync::Arc;

use melodia_core::entities::correction::{Correction, CorrectionFilters};
use melodia_core::entities::device_map::DeviceMap;
use melodia_playback::player::playback::correction::shared::{CorrectionShared, CorrectionWriter};

use super::PlaybackEngine;

#[derive(Default)]
pub(super) struct CorrectionControl {
    writer: CorrectionWriter,
    corrections: DeviceMap<Correction>,
    enabled: bool,
    /// `None` while the writer's set is empty.
    published: Option<CorrectionFilters>,
}

impl CorrectionControl {
    pub(super) fn shared(&self) -> Arc<CorrectionShared> {
        self.writer.shared()
    }

    /// Whether a set plays: the switch on and a correction set for the heard device.
    pub(super) fn engaged(&self) -> bool {
        self.published.is_some()
    }

    pub(super) fn set_corrections(
        &mut self,
        corrections: DeviceMap<Correction>,
        heard: Option<&str>,
    ) {
        self.corrections = corrections;
        self.select(heard);
    }

    pub(super) fn set_enabled(&mut self, enabled: bool, heard: Option<&str>) {
        self.enabled = enabled;
        self.select(heard);
    }

    /// Publishes only a change: the same set again would still crossfade, into a bank starting
    /// from rest.
    pub(super) fn select(&mut self, heard: Option<&str>) {
        let wanted = if self.enabled {
            self.corrections.for_device(heard).map(|correction| &correction.filters)
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
    pub fn set_corrections(&self, corrections: DeviceMap<Correction>) {
        self.headphones.lock().set_corrections(corrections);
    }

    /// Off plays every source uncorrected and keeps each device's correction.
    pub fn set_correction_enabled(&self, enabled: bool) {
        self.headphones.lock().set_correction_enabled(enabled);
    }
}

#[cfg(test)]
#[path = "tests/correction_tests.rs"]
mod tests;
