//! The Output card's exclusive pickers: shared or exclusive, which card a claim takes, how its
//! writer paces the card, and whether the card's own control carries the volume.
//!
//! **A pick is applied on the blocking pool, never here**: claiming a card or handing it back
//! opens a device. Every picker changes the one choice, so each writes a synchronous shadow and
//! the task reads the shadow when it runs rather than capturing a value. Whichever task runs last
//! then applies and persists the latest pick, in whatever order the pool ran them.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use crate::ui::settings_bind::read_or_default;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_engine::player::engine::backend::OutputChoice;
use melodia_playback::player::playback::output::{
    Drive, ExclusiveTuning, OutputDevice, OutputMode,
};
use melodia_ui::{AppWindow, Settings};

/// The period chips, in the order of the inline list in `output-section.slint`.
const PERIOD_PRESETS: [Duration; 5] = [
    Duration::from_millis(5),
    Duration::from_millis(10),
    Duration::from_millis(20),
    Duration::from_millis(50),
    Duration::from_millis(100),
];

/// The choice as last picked, and the cards the device picker's indices point into.
struct Picked {
    choice: OutputChoice,
    devices: Vec<OutputDevice>,
    /// The newest listing asked for, and the newest applied, as `Settings.output-devices-revision`.
    listing_asked: i32,
    listing_applied: i32,
}

type Shadow = Arc<Mutex<Picked>>;

pub fn install(ui: &AppWindow, state: &AppState) {
    let g = ui.global::<Settings>();
    g.set_exclusive_supported(library::playback::EXCLUSIVE_SUPPORTED);
    g.set_polling_supported(library::playback::POLLING_SUPPORTED);
    g.set_hardware_volume_supported(library::playback::HARDWARE_VOLUME_SUPPORTED);
    if !library::playback::EXCLUSIVE_SUPPORTED {
        return;
    }

    let choice = read_or_default(state, "output mode").output.output_choice();
    g.set_output_mode_idx(mode_index(choice.mode));
    g.set_output_period_idx(period_index(choice.tuning.period));
    g.set_output_polling(choice.tuning.drive == Drive::Polling);
    g.set_output_hardware_volume(choice.hardware_volume);
    let shadow: Shadow = Arc::new(Mutex::new(Picked {
        choice,
        devices: Vec::new(),
        listing_asked: 0,
        listing_applied: 0,
    }));

    let state_list = state.clone();
    let shadow_list = Arc::clone(&shadow);
    let weak = ui.as_weak();
    g.on_list_output_devices(move || list_devices(weak.clone(), &state_list, &shadow_list));

    let state_mode = state.clone();
    let shadow_mode = Arc::clone(&shadow);
    g.on_output_mode_changed(move |idx| {
        shadow_mode.lock().choice.mode = mode_from_index(idx);
        apply(&state_mode, &shadow_mode);
    });

    let state_device = state.clone();
    let shadow_device = Arc::clone(&shadow);
    g.on_output_device_changed(move |idx| {
        {
            let mut picked = shadow_device.lock();
            let chosen = usize::try_from(idx).ok().and_then(|i| picked.devices.get(i));
            picked.choice.device = chosen.map(|device| device.id.clone());
        }
        apply(&state_device, &shadow_device);
    });

    let state_period = state.clone();
    let shadow_period = Arc::clone(&shadow);
    g.on_output_period_changed(move |idx| {
        let Some(&period) = usize::try_from(idx).ok().and_then(|i| PERIOD_PRESETS.get(i)) else {
            return;
        };
        {
            let mut picked = shadow_period.lock();
            picked.choice.tuning = ExclusiveTuning::new(period, picked.choice.tuning.drive);
        }
        apply(&state_period, &shadow_period);
    });

    let state_polling = state.clone();
    let shadow_polling = Arc::clone(&shadow);
    g.on_output_polling_changed(move |on| {
        shadow_polling.lock().choice.tuning.drive = if on { Drive::Polling } else { Drive::Events };
        apply(&state_polling, &shadow_polling);
    });

    let state_volume = state.clone();
    g.on_output_hardware_volume_changed(move |on| {
        shadow.lock().choice.hardware_volume = on;
        apply(&state_volume, &shadow);
    });
}

/// Apply the shadow's choice to the engine and persist it, off the UI thread.
fn apply(state: &AppState, shadow: &Shadow) {
    let ctx = state.playback_ctx();
    let shadow = Arc::clone(shadow);
    state.persist_blocking("persist output choice", move |state| {
        let choice = shadow.lock().choice.clone();
        library::playback::player_set_output_choice(&ctx, choice.clone());
        library::settings::set_output_choice(state, &choice)
    });
}

/// Ask the cards for themselves off the UI thread, then fill the picker, and return the revision
/// the answer will be published under.
fn list_devices(weak: Weak<AppWindow>, state: &AppState, shadow: &Shadow) -> i32 {
    let revision = {
        let mut picked = shadow.lock();
        picked.listing_asked = picked.listing_asked.saturating_add(1);
        picked.listing_asked
    };
    let shadow = Arc::clone(shadow);
    state.runtime.spawn_blocking(move || {
        let devices = library::playback::output_devices();
        // Applied on the UI thread, so a pick resolves against the list on screen and the
        // selection reads the choice as it stands.
        let _ =
            weak.upgrade_in_event_loop(move |ui| apply_listing(&ui, &shadow, devices, revision));
    });
    revision
}

/// Fill the picker from the listing asked for at `revision`. A saved card that isn't connected
/// selects nothing, and the picker says so in place of a name.
///
/// **The options are replaced only when the cards changed, and never by an older listing.** The
/// picker lists on a timer while it is on screen, and an open popup keeps the size it was shown
/// at, so options replaced under it paint into a box measured for others.
fn apply_listing(ui: &AppWindow, shadow: &Shadow, devices: Vec<OutputDevice>, revision: i32) {
    let (names, selected, missing) = {
        let mut picked = shadow.lock();
        if revision < picked.listing_applied {
            return;
        }
        picked.listing_applied = revision;
        let names = (picked.devices != devices).then(|| {
            devices
                .iter()
                .map(|device| SharedString::from(device.name.as_str()))
                .collect::<Vec<_>>()
        });
        picked.devices = devices;
        let selected = match &picked.choice.device {
            Some(id) => picked.devices.iter().position(|device| &device.id == id),
            // No saved card means the first one listed, which is what the claim takes.
            None => (!picked.devices.is_empty()).then_some(0),
        };
        (names, selected, picked.choice.device.is_some() && selected.is_none())
    };
    let g = ui.global::<Settings>();
    if let Some(names) = names {
        g.set_output_device_names(ModelRc::from(Rc::new(VecModel::from(names))));
    }
    g.set_output_device_idx(selected.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1));
    g.set_output_device_missing(missing);
    g.set_output_devices_revision(revision);
}

/// The chip's index, in the order of the inline list in `output-section.slint`.
fn mode_index(mode: OutputMode) -> i32 {
    match mode {
        OutputMode::Shared => 0,
        OutputMode::Exclusive => 1,
    }
}

fn mode_from_index(idx: i32) -> OutputMode {
    match idx {
        1 => OutputMode::Exclusive,
        _ => OutputMode::Shared,
    }
}

/// The chip showing `period`, or none where a hand-edited period matches no preset.
fn period_index(period: Duration) -> i32 {
    PERIOD_PRESETS
        .iter()
        .position(|&preset| preset == period)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1)
}
