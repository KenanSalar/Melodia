//! The Output card's settings: following the file's rate and the silence a rate change writes
//! first, then the exclusive pickers (shared or exclusive, which card a claim takes, and shared
//! output too where the platform can aim it, what a claim does about a rate the card lacks, how its
//! writer paces the card, and whether the card's own control carries the volume), and whether a
//! long pause gives the card back. The card's live readout is [`super::signal_path`]'s.
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

use crate::ui::settings::signal_path;
use crate::ui::settings_bind::{read_or_default, toggle_binding};
use crate::ui::shell::tray_bridge;
use melodia_app::library;
use melodia_app::services::settings::OutputFlags;
use melodia_app::state::AppState;
use melodia_engine::player::engine::backend::OutputChoice;
use melodia_playback::player::playback::output::{
    self, Drive, ExclusiveTuning, OutputDevice, OutputMode, RateFallback,
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
    /// The row ahead of the cards that stands for no saved card, as it reads on screen, where the
    /// picker chooses for shared output too.
    default_label: Option<SharedString>,
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
    g.set_rate_fallback_supported(library::playback::RATE_FALLBACK_SUPPORTED);
    g.set_shared_device_supported(library::playback::SHARED_DEVICE_SUPPORTED);

    let flags = read_or_default(state, "output").output;
    install_rate_rows(ui, state, &flags);
    if library::playback::EXCLUSIVE_SUPPORTED {
        install_pickers(ui, state, flags.output_choice());
        install_release_row(ui, state, &flags);
    }
}

/// Giving the device back after a long pause. A toggle beside the pickers rather than one of them,
/// since it changes nothing about the claim and so has nothing to reopen.
fn install_release_row(ui: &AppWindow, state: &AppState, flags: &OutputFlags) {
    let g = ui.global::<Settings>();
    g.set_output_release_paused(flags.output_paused_device.releases());
    g.on_output_release_paused_changed(toggle_binding(
        state,
        "persist output_paused_device",
        library::playback::player_set_release_when_paused,
        library::settings::set_output_paused_device,
    ));
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
    g.set_output_rate_fallback_idx(rate_fallback_index(choice.rate_fallback));
    g.set_output_period_idx(period_index(choice.tuning.period));
    g.set_output_polling(choice.tuning.drive == Drive::Polling);
    g.set_output_hardware_volume(choice.hardware_volume);
    let shadow: Shadow = Arc::new(Mutex::new(Picked {
        choice,
        devices: Vec::new(),
        default_label: None,
        listing_asked: 0,
        listing_applied: 0,
    }));

    let state_list = state.clone();
    let shadow_list = Arc::clone(&shadow);
    let weak = ui.as_weak();
    g.on_list_output_devices(move || list_devices(weak.clone(), &state_list, &shadow_list));

    g.on_output_mode_changed(pick(state, &shadow, Picked::pick_mode));
    g.on_output_device_changed(pick(state, &shadow, Picked::pick_device));
    g.on_output_rate_fallback_changed(pick(state, &shadow, Picked::pick_rate_fallback));
    g.on_output_period_changed(pick(state, &shadow, Picked::pick_period));
    g.on_output_polling_changed(pick(state, &shadow, Picked::pick_polling));
    g.on_output_hardware_volume_changed(pick(state, &shadow, Picked::pick_hardware_volume));
    install_bit_perfect_switch(ui, state, &shadow);
}

/// The confirmed Make Bit-Perfect from shared output: pick exclusive, then reset.
///
/// **One task, the claim first.** The reset keeps the volume only where the claim carries it on the
/// device, which it can't know before the claim opens. Run first, it raises the voices to full and
/// the claim opens at that level, which on a device a release in this run put back is the device
/// raised to full.
fn install_bit_perfect_switch(ui: &AppWindow, state: &AppState, shadow: &Shadow) {
    let state = state.clone();
    let shadow = Arc::clone(shadow);
    let weak = ui.as_weak();
    ui.global::<Settings>().on_output_switch_to_bit_perfect(move || {
        shadow.lock().choice.mode = OutputMode::Exclusive;
        let ctx = state.playback_ctx();
        let shadow = Arc::clone(&shadow);
        state.persist_blocking("persist bit-perfect switch", move |state| {
            let choice = shadow.lock().choice.clone();
            library::playback::player_set_output_choice(&ctx, choice.clone());
            let volume = library::playback::player_make_bit_perfect(&ctx);
            library::settings::set_output_choice(state, &choice)?;
            library::settings::reset_for_bit_perfect(state, volume)
        });
        if let Some(ui) = weak.upgrade() {
            signal_path::show_bit_perfect_reset(&ui);
        }
    });
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

/// Fill the picker from the listing asked for at `revision`, unless a newer one is on screen.
fn apply_listing(ui: &AppWindow, shadow: &Shadow, devices: Vec<OutputDevice>, revision: i32) {
    let g = ui.global::<Settings>();
    let default_label =
        library::playback::SHARED_DEVICE_SUPPORTED.then(|| g.invoke_output_device_system_default());
    let Some(listing) = shadow.lock().take_listing(devices, default_label, revision) else {
        return;
    };
    if let Some(names) = listing.names {
        g.set_output_device_names(ModelRc::from(Rc::new(VecModel::from(names))));
    }
    g.set_output_device_idx(listing.selected);
    g.set_output_device_missing(listing.missing);
    g.set_output_devices_revision(revision);
}

/// What the device picker shows for one listing.
struct Listing {
    /// The options, or `None` where the cards are the ones on screen already.
    names: Option<Vec<SharedString>>,
    /// The chosen card's row, or -1 for none.
    selected: i32,
    missing: bool,
}

impl Picked {
    fn pick_mode(&mut self, idx: i32) {
        self.choice.mode = mode_from_index(idx);
    }

    /// The card at `idx` in the listing on screen, which is the one the user clicked, or none for
    /// the system default's row.
    fn pick_device(&mut self, idx: i32) {
        let chosen = usize::try_from(idx)
            .ok()
            .and_then(|row| row.checked_sub(self.lead_rows()))
            .and_then(|i| self.devices.get(i));
        self.choice.device = chosen.map(|device| device.id.clone());
    }

    /// How many rows the picker shows ahead of the cards.
    fn lead_rows(&self) -> usize {
        usize::from(self.default_label.is_some())
    }

    fn pick_rate_fallback(&mut self, idx: i32) {
        self.choice.rate_fallback = rate_fallback_from_index(idx);
    }

    fn pick_period(&mut self, idx: i32) {
        if let Some(&period) = usize::try_from(idx).ok().and_then(|i| PERIOD_PRESETS.get(i)) {
            self.choice.tuning = ExclusiveTuning::new(period, self.choice.tuning.drive);
        }
    }

    fn pick_polling(&mut self, on: bool) {
        self.choice.tuning.drive = Drive::from_polling(on);
    }

    fn pick_hardware_volume(&mut self, on: bool) {
        self.choice.hardware_volume = on;
    }

    /// Take the listing asked for at `revision`, led by `default_label` where there is one,
    /// answering what the picker shows for it, or `None` where a newer one is on screen already. A
    /// saved card that isn't connected selects nothing, and the picker says so in place of a name.
    ///
    /// **The options are replaced only when the cards or the lead row changed, and never by an
    /// older listing.** The picker lists on a timer while it is on screen, and an open popup keeps
    /// the size it was shown at, so options replaced under it paint into a box measured for others.
    fn take_listing(
        &mut self,
        devices: Vec<OutputDevice>,
        default_label: Option<SharedString>,
        revision: i32,
    ) -> Option<Listing> {
        if revision < self.listing_applied {
            return None;
        }
        self.listing_applied = revision;
        let names = (self.devices != devices || self.default_label != default_label).then(|| {
            let cards = devices.iter().map(|device| SharedString::from(device.name.as_str()));
            default_label.iter().cloned().chain(cards).collect()
        });
        self.devices = devices;
        self.default_label = default_label;
        let selected = match &self.choice.device {
            Some(id) => self
                .devices
                .iter()
                .position(|device| &device.id == id)
                .map(|i| i + self.lead_rows()),
            // No saved card means the system default's row where there is one, or else the first
            // card listed, which is what the claim takes.
            None => (self.lead_rows() > 0 || !self.devices.is_empty()).then_some(0),
        };
        Some(Listing {
            names,
            selected: selected.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1),
            missing: self.choice.device.is_some() && selected.is_none(),
        })
    }
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

/// The chip's index, in the order of the inline list in `output-section.slint`.
fn rate_fallback_index(fallback: RateFallback) -> i32 {
    match fallback {
        RateFallback::Shared => 0,
        RateFallback::Resample => 1,
    }
}

fn rate_fallback_from_index(idx: i32) -> RateFallback {
    match idx {
        1 => RateFallback::Resample,
        _ => RateFallback::Shared,
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

#[cfg(test)]
#[path = "tests/output_settings_tests.rs"]
mod tests;
