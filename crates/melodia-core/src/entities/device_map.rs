//! What each output device keeps for itself, System default counting as one device.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Keyed by the device the output is aimed at, `None` naming System default.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceMap<T> {
    /// Its own field rather than a key in `devices`, which no device id can then collide with.
    pub system_default: Option<T>,
    /// By `OutputDevice::id`.
    pub devices: BTreeMap<String, T>,
}

// By hand: the derive would ask `T: Default` of a map that starts empty whatever it holds.
impl<T> Default for DeviceMap<T> {
    fn default() -> Self {
        Self { system_default: None, devices: BTreeMap::new() }
    }
}

impl<T> DeviceMap<T> {
    pub fn for_device(&self, device: Option<&str>) -> Option<&T> {
        match device {
            Some(id) => self.devices.get(id),
            None => self.system_default.as_ref(),
        }
    }

    pub fn insert(&mut self, device: Option<String>, value: T) {
        match device {
            Some(id) => {
                self.devices.insert(id, value);
            }
            None => self.system_default = Some(value),
        }
    }

    pub fn remove(&mut self, device: Option<&str>) -> Option<T> {
        match device {
            Some(id) => self.devices.remove(id),
            None => self.system_default.take(),
        }
    }
}
