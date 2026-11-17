//! Crossfeed's rows on the headphone card: the switch, and the strength slider under it.

use std::rc::Rc;

use slint::ComponentHandle;

use crate::ui::headphones::Card;
use melodia_app::library;
use melodia_app::library::headphones::DeviceTarget;
use melodia_app::state::AppState;
use melodia_core::entities::crossfeed::{CrossfeedStrength, DEFAULT_STRENGTH, STRENGTH_RANGE};
use melodia_core::error::describe;
use melodia_ui::{AppWindow, Crossfeed, CrossfeedLevel};

/// Wires the switch and the slider onto `card`. UI thread.
pub(crate) fn install(ui: &AppWindow, card: &Rc<Card>) {
    let g = ui.global::<Crossfeed>();
    let c = Rc::clone(card);
    g.on_set_enabled(move |enabled| {
        let Some(DeviceTarget { device, .. }) = c.target() else { return };
        library::crossfeed::set_enabled(c.state(), device, enabled);
    });
    let c = Rc::clone(card);
    g.on_preview_strength(move |value| {
        write_strength(&c, value, library::crossfeed::preview_strength);
    });
    let c = Rc::clone(card);
    g.on_set_strength(move |value| write_strength(&c, value, library::crossfeed::set_strength));
}

/// The crossfeed kept for `device` as the map stands now.
pub(crate) fn paint(ui: &AppWindow, state: &AppState, device: Option<&str>) {
    let setting = library::crossfeed::crossfeed_for(state, device);
    let g = ui.global::<Crossfeed>();
    g.set_enabled(setting.enabled);
    g.set_strength(setting.strength.get());
    g.set_level(level(setting.strength));
}

/// Repaints after the write, so the readout follows a drag.
fn write_strength(
    card: &Card,
    value: f32,
    write: fn(&AppState, Option<String>, CrossfeedStrength),
) {
    let Some(DeviceTarget { device, .. }) = card.target() else { return };
    let strength = match CrossfeedStrength::try_from(value) {
        Ok(strength) => strength,
        Err(e) => {
            log::warn!("crossfeed: slider value refused: {}", describe(&e));
            return;
        }
    };
    write(card.state(), device, strength);
    card.paint();
}

/// The readout's word: whichever of the slider's two ends and the default strength is nearest.
fn level(strength: CrossfeedStrength) -> CrossfeedLevel {
    let medium = DEFAULT_STRENGTH.get();
    let strength = strength.get();
    if strength < STRENGTH_RANGE.start().midpoint(medium) {
        CrossfeedLevel::Subtle
    } else if strength < medium.midpoint(*STRENGTH_RANGE.end()) {
        CrossfeedLevel::Medium
    } else {
        CrossfeedLevel::Strong
    }
}
