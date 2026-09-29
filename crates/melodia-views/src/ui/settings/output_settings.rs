//! The Output card's settings: following the file's rate and the silence a rate change writes
//! first, then the exclusive pickers (shared or exclusive, which card a claim takes, how its writer
//! paces the card, and whether the card's own control carries the volume). The card's live
//! readout is [`super::signal_path`]'s.
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

use crate::ui::settings_bind::{read_or_default, toggle_binding};
use crate::ui::shell::tray_bridge;
use melodia_app::library;
use melodia_app::services::settings::OutputFlags;
use melodia_app::state::AppState;
use melodia_engine::player::engine::backend::OutputChoice;
use melodia_playback::player::playback::output::{
    self, Drive, ExclusiveTuning, OutputDevice, OutputMode,
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
    g.set_follow_rate_supported(library::playback::FOLLOW_RATE_SUPPORTED);
    g.set_exclusive_supported(library::playback::EXCLUSIVE_SUPPORTED);
    g.set_polling_supported(library::playback::POLLING_SUPPORTED);
    g.set_hardware_volume_supported(library::playback::HARDWARE_VOLUME_SUPPORTED);

    let flags = read_or_default(state, "output").output;
    install_rate_rows(ui, state, &flags);
    if library::playback::EXCLUSIVE_SUPPORTED {
        install_pickers(ui, state, flags.output_choice());
    }
}

/// Following the file's rate and the resync hold, which apply whether or not a card can be claimed.
fn install_rate_rows(ui: &AppWindow, state: &AppState, flags: &OutputFlags) {
    let g = ui.global::<Settings>();
    // The range first, so the value seeded after it lands inside the slider's track.
    g.set_output_resync_max_ms(millis(output::MAX_RESYNC_HOLD));
    g.set_output_follow_rate(flags.output_follow_rate);
    g.set_output_resync_ms(millis(flags.resync_hold().min(output::MAX_RESYNC_HOLD)));

    g.on_output_follow_rate_changed(toggle_binding(
        state,
        "persist output_follow_rate",
        library::playback::player_set_follow_rate,
        library::settings::set_output_follow_rate,
    ));

    // Engine and disk together, on the pool: setting the hold waits out any reopen in flight.
    let state = state.clone();
    g.on_output_resync_committed(move |ms| {
        let ms = u32::try_from(ms).unwrap_or(0);
        let ctx = state.playback_ctx();
        state.persist_blocking("persist output_resync_ms", move |s| {
            library::playback::player_set_resync_hold(&ctx, Duration::from_millis(u64::from(ms)));
            library::settings::set_output_resync_ms(s, ms)
        });
    });
}

fn install_pickers(ui: &AppWindow, state: &AppState, choice: OutputChoice) {
    let g = ui.global::<Settings>();
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

    g.on_output_mode_changed(pick(state, &shadow, |picked: &mut Picked, idx: i32| {
        picked.choice.mode = mode_from_index(idx);
    }));
    g.on_output_device_changed(pick(state, &shadow, |picked: &mut Picked, idx: i32| {
        let chosen = usize::try_from(idx).ok().and_then(|i| picked.devices.get(i));
        picked.choice.device = chosen.map(|device| device.id.clone());
    }));
    g.on_output_period_changed(pick(state, &shadow, |picked: &mut Picked, idx: i32| {
        if let Some(&period) = usize::try_from(idx).ok().and_then(|i| PERIOD_PRESETS.get(i)) {
            picked.choice.tuning = ExclusiveTuning::new(period, picked.choice.tuning.drive);
        }
    }));
    g.on_output_polling_changed(pick(state, &shadow, |picked: &mut Picked, on: bool| {
        picked.choice.tuning.drive = Drive::from_polling(on);
    }));
    g.on_output_hardware_volume_changed(pick(state, &shadow, |picked: &mut Picked, on: bool| {
        picked.choice.hardware_volume = on;
    }));
}

/// A picker's callback: `edit` the shadow with what was picked, then [`apply`] it.
fn pick<T>(
    state: &AppState,
    shadow: &Shadow,
    edit: impl Fn(&mut Picked, T) + 'static,
) -> impl Fn(T) + 'static {
    let state = state.clone();
    let shadow = Arc::clone(shadow);
    move |value| {
        edit(&mut shadow.lock(), value);
        apply(&state, &shadow);
    }
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
    // The picker's timer keeps firing behind a window hidden to the tray, where nobody reads it.
    // The listing it already shows is the answer, so a waiting dropdown opens at once.
    if !tray_bridge::is_window_visible() {
        return shadow.lock().listing_applied;
    }
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

/// `span` as the whole milliseconds a slider reads. The spans here are a second at most.
fn millis(span: Duration) -> f32 {
    f32::from(u16::try_from(span.as_millis()).unwrap_or(u16::MAX))
}

/// The chip showing `period`, or none where a hand-edited period matches no preset.
fn period_index(period: Duration) -> i32 {
    PERIOD_PRESETS
        .iter()
        .position(|&preset| preset == period)
        .and_then(|i| i32::try_from(i).ok())
        .unwrap_or(-1)
}
