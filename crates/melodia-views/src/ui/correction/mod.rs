//! The headphone correction's rows on the headphone card, and its two dialogs: `picker` sets a
//! model from `AutoEq`'s list, `import` a file or a paste.

mod import;
mod picker;

use std::rc::Rc;

use slint::ComponentHandle;

use crate::ui::headphones::Card;
use crate::ui::settings_bind::{read_or_default, shadow_toggle, toggle_binding};
use crate::ui::undo::{Removal, UndoWindow};
use melodia_app::library;
use melodia_app::library::headphones::DeviceTarget;
use melodia_app::state::AppState;
use melodia_core::entities::correction::{Correction, CorrectionOrigin};
use melodia_ui::{AppWindow, CorrectionFrom, HeadphoneCorrection};

/// Seeds both switches and wires the rows and their dialogs onto `card`. UI thread.
pub(crate) fn install(ui: &AppWindow, undo: &Rc<UndoWindow>, card: &Rc<Card>) {
    let state = card.state();
    let g = ui.global::<HeadphoneCorrection>();
    g.set_enabled(read_or_default(state, "headphone correction").correction.correction_enabled);
    g.set_online(state.correction_online.get());
    g.on_set_enabled(toggle_binding(
        state,
        "persist correction_enabled",
        library::playback::player_set_correction_enabled,
        library::settings::set_correction_enabled,
    ));
    g.on_set_online(shadow_toggle(
        state,
        &state.correction_online,
        "persist correction_online",
        library::settings::set_correction_online,
    ));

    let (c, undo) = (Rc::clone(card), Rc::clone(undo));
    g.on_clear(move || clear(&c, &undo));

    picker::install(ui, card);
    import::install(ui, card);
}

/// What Clear took, and the device it goes back to.
pub struct ClearUndo {
    card: Rc<Card>,
    device: Option<String>,
    correction: Correction,
}

impl ClearUndo {
    pub fn restore(self) {
        library::correction::restore(self.card.state(), self.device, self.correction);
        self.card.paint();
    }
}

/// The correction set for `device` as the map stands now. The card's words for one are Slint's,
/// so only its kind and names cross.
pub(crate) fn paint(ui: &AppWindow, state: &AppState, device: Option<&str>) {
    let origin = library::correction::correction_for(state, device).map(|c| c.origin);
    let (from, name, source) = match origin {
        None => (CorrectionFrom::None, String::new(), String::new()),
        Some(CorrectionOrigin::AutoEq { model, source }) => (CorrectionFrom::Autoeq, model, source),
        Some(CorrectionOrigin::File { name }) => (CorrectionFrom::File, name, String::new()),
        Some(CorrectionOrigin::Paste) => (CorrectionFrom::Paste, String::new(), String::new()),
    };
    let g = ui.global::<HeadphoneCorrection>();
    g.set_origin(from);
    g.set_origin_name(name.into());
    g.set_origin_source(source.into());
}

fn clear(card: &Rc<Card>, undo: &UndoWindow) {
    let Some(DeviceTarget { device, .. }) = card.target() else { return };
    let Some(correction) = library::correction::clear(card.state(), device.as_deref()) else {
        return;
    };
    card.paint();
    undo.offer(Removal::Correction(ClearUndo { card: Rc::clone(card), device, correction }));
}
