//! The headphone correction card on Settings ▸ Effects, and its two dialogs: `picker` sets a model
//! from `AutoEq`'s list, `import` a file or a paste.
//!
//! **The card speaks for the device the engine plays through**, read back from the engine on each
//! output change rather than from the Output card, which may not have listed a device this run.
//! Every write names that device as the card last showed it.

mod import;
mod picker;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::ui::settings_bind::{read_or_default, shadow_toggle, toggle_binding};
use crate::ui::signal::on_signal;
use crate::ui::undo::{Removal, UndoWindow};
use melodia_app::library;
use melodia_app::library::correction::{CorrectionTarget, Heard};
use melodia_app::state::AppState;
use melodia_core::entities::correction::{Correction, CorrectionOrigin};
use melodia_core::error::describe;
use melodia_ui::{AppWindow, CorrectionFrom, HeadphoneCorrection, HeardDevice};

/// Seeds both switches, wires the card and its dialogs, and keeps the card on the device heard.
/// UI thread.
pub fn install(ui: &AppWindow, state: &AppState, undo: &Rc<UndoWindow>) {
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

    let card = Rc::new(Card {
        state: state.clone(),
        ui: ui.as_weak(),
        heard: RefCell::new(None),
        asked: Cell::new(0),
    });
    let c = Rc::clone(&card);
    g.on_refresh(move || c.refresh());
    let (c, undo) = (Rc::clone(&card), Rc::clone(undo));
    g.on_clear(move || c.clear(&undo));

    picker::install(ui, state, &card);
    import::install(ui, state, &card);

    let followed =
        on_signal(&state.output_changed, ui.as_weak(), "headphone correction", move |_| {
            card.refresh();
        });
    if let Err(e) = followed {
        log::warn!("The correction card won't follow the output: {}", describe(&e));
    }
}

/// What Clear took, and the device it goes back to.
pub struct ClearUndo {
    card: Rc<Card>,
    device: Option<String>,
    correction: Correction,
}

impl ClearUndo {
    pub fn restore(self) {
        library::correction::restore(&self.card.state, self.device, self.correction);
        self.card.paint();
    }
}

struct Card {
    state: AppState,
    ui: slint::Weak<AppWindow>,
    /// `None` until the first naming lands.
    heard: RefCell<Option<Heard>>,
    /// Moved by each naming, so one landing late can't repaint over a newer.
    asked: Cell<u64>,
}

impl Card {
    /// Names the device heard off the UI thread, the listing that names it being blocking.
    fn refresh(self: &Rc<Self>) {
        let asked = self.asked.get().wrapping_add(1);
        self.asked.set(asked);
        let state = self.state.clone();
        let naming = self.state.runtime.spawn_blocking(move || library::correction::heard(&state));
        let card = Rc::downgrade(self);
        let spawned = slint::spawn_local(Compat::new(async move {
            let heard = match naming.await {
                Ok(heard) => heard,
                Err(e) => {
                    log::warn!("headphone correction: naming the device failed: {}", describe(&e));
                    return;
                }
            };
            let Some(card) = card.upgrade() else { return };
            if card.asked.get() == asked {
                *card.heard.borrow_mut() = Some(heard);
                card.paint();
            }
        }));
        if let Err(e) = spawned {
            log::warn!("headphone correction: naming not spawned: {}", describe(&e));
        }
    }

    /// The device a write lands on, `None` until the card has named one.
    fn target(&self) -> Option<CorrectionTarget> {
        self.heard.borrow().as_ref().map(|heard| heard.target.clone())
    }

    /// The device as last named, and the correction set for it as the map stands now.
    fn paint(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let g = ui.global::<HeadphoneCorrection>();
        let heard = self.heard.borrow();
        let Some(heard) = heard.as_ref() else {
            g.set_device(HeardDevice::Pending);
            return;
        };
        g.set_device(heard_device(heard));
        g.set_device_name(heard.target.device_name.clone().unwrap_or_default().into());
        let device = heard.target.device.as_deref();
        let origin = library::correction::correction_for(&self.state, device)
            .map(|correction| correction.origin);
        paint_origin(&g, origin);
    }

    fn clear(self: &Rc<Self>, undo: &UndoWindow) {
        let Some(CorrectionTarget { device, .. }) = self.target() else { return };
        let Some(correction) = library::correction::correction_for(&self.state, device.as_deref())
        else {
            return;
        };
        library::correction::clear(&self.state, device.as_deref());
        self.paint();
        undo.offer(Removal::Correction(ClearUndo { card: Rc::clone(self), device, correction }));
    }
}

fn heard_device(heard: &Heard) -> HeardDevice {
    match (&heard.target.device, heard.connected) {
        (None, _) => HeardDevice::SystemDefault,
        (Some(_), true) => HeardDevice::Connected,
        (Some(_), false) => HeardDevice::NotConnected,
    }
}

/// The card's words for a correction are Slint's, so only its kind and names cross.
fn paint_origin(g: &HeadphoneCorrection<'_>, origin: Option<CorrectionOrigin>) {
    let (from, name, source) = match origin {
        None => (CorrectionFrom::None, String::new(), String::new()),
        Some(CorrectionOrigin::AutoEq { model, source }) => (CorrectionFrom::Autoeq, model, source),
        Some(CorrectionOrigin::File { name }) => (CorrectionFrom::File, name, String::new()),
        Some(CorrectionOrigin::Paste) => (CorrectionFrom::Paste, String::new(), String::new()),
    };
    g.set_origin(from);
    g.set_origin_name(name.into());
    g.set_origin_source(source.into());
}
