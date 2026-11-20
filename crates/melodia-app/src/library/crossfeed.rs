//! Crossfeed's library API. Each write changes one field inside the shadow's lock: a setting read
//! and written back whole would undo a write that landed in between.

use std::sync::Arc;

use crate::library;
use crate::state::AppState;
use melodia_core::entities::crossfeed::{CrossfeedSetting, CrossfeedStrength};
use melodia_core::entities::device_map::DeviceMap;

/// `device`'s crossfeed as last set, `None` naming System default; off where it has none.
pub fn crossfeed_for(state: &AppState, device: Option<&str>) -> CrossfeedSetting {
    state.crossfeed.picked().for_device(device).copied().unwrap_or_default()
}

pub fn set_enabled(state: &AppState, device: Option<String>, enabled: bool) {
    commit(state, |crossfeed| crossfeed.get_or_insert_default(device).enabled = enabled);
}

/// A slider drag, heard at once and written by the release's [`set_strength`], which also settles
/// a commit that landed an older map mid-drag.
pub fn preview_strength(state: &AppState, device: Option<String>, strength: CrossfeedStrength) {
    let crossfeed = {
        let mut crossfeed = state.crossfeed.picked();
        crossfeed.get_or_insert_default(device).strength = strength;
        crossfeed.clone()
    };
    library::playback::player_set_crossfeed(&state.playback_ctx(), crossfeed);
}

pub fn set_strength(state: &AppState, device: Option<String>, strength: CrossfeedStrength) {
    commit(state, |crossfeed| crossfeed.get_or_insert_default(device).strength = strength);
}

/// Make Bit-Perfect's half, for the device it makes bit-perfect. Call it before spawning an output
/// switch, so the card names the new device only once its crossfeed is off.
pub fn switch_off(state: &AppState, device: Option<String>) {
    if !crossfeed_for(state, device.as_deref()).enabled {
        return;
    }
    set_enabled(state, device, false);
}

fn commit(state: &AppState, edit: impl FnOnce(&mut DeviceMap<CrossfeedSetting>)) {
    edit(&mut state.crossfeed.picked());
    let ctx = state.playback_ctx();
    let shadow = Arc::clone(&state.crossfeed);
    state.persist_blocking("persist crossfeed", move |paths| {
        shadow.apply_latest(DeviceMap::clone, |crossfeed| {
            library::playback::player_set_crossfeed(&ctx, crossfeed.clone());
            library::settings::set_crossfeed(paths, crossfeed)
        })
    });
}
