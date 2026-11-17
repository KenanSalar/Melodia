//! The device the engine plays through, as the headphone card names it for the settings each
//! device keeps.

use crate::library;
use crate::state::AppState;

/// The device a write lands on, as the card named it when the user acted: a profile still
/// downloading as the output moves lands where it was asked for.
#[derive(Clone, Debug)]
pub struct DeviceTarget {
    /// `None` is System default.
    pub device: Option<String>,
    /// Stored as the correction's `device_name`.
    pub device_name: Option<String>,
}

/// The device the engine plays through, as the card names it.
#[derive(Clone, Debug)]
pub struct Heard {
    pub target: DeviceTarget,
    /// Whether the listing found it. One it didn't is named by what its correction stored, if
    /// anything; System default always counts as found.
    pub connected: bool,
}

/// Blocking while a device is heard: naming it asks every device.
pub fn heard(state: &AppState) -> Heard {
    let Some(id) = library::playback::player_heard_device(&state.playback_ctx()) else {
        let target = DeviceTarget { device: None, device_name: None };
        return Heard { target, connected: true };
    };
    let listed = library::playback::output_devices()
        .into_iter()
        .find(|device| device.id == id)
        .map(|device| device.name);
    let connected = listed.is_some();
    let device_name = listed.or_else(|| {
        library::correction::correction_for(state, Some(&id))
            .and_then(|correction| correction.device_name)
    });
    Heard { target: DeviceTarget { device: Some(id), device_name }, connected }
}
