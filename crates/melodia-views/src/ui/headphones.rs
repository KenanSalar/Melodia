//! The headphone card on Settings ▸ Effects, holding each setting a device keeps.
//!
//! **The card speaks for the device the engine plays through**, read back from the engine on each
//! output change rather than from the Output card, which may not have listed a device this run.
//! Every write names that device as the card last showed it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::ui::correction;
use crate::ui::signal::on_signal;
use crate::ui::undo::UndoWindow;
use melodia_app::library;
use melodia_app::library::headphones::{DeviceTarget, Heard};
use melodia_app::state::AppState;
use melodia_core::error::describe;
use melodia_ui::{AppWindow, HeadphoneCorrection, HeardDevice};

/// Wires the card's rows and keeps the card on the device heard. UI thread.
pub fn install(ui: &AppWindow, state: &AppState, undo: &Rc<UndoWindow>) {
    let card = Rc::new(Card {
        state: state.clone(),
        ui: ui.as_weak(),
        heard: RefCell::new(None),
        asked: Cell::new(0),
    });
    let c = Rc::clone(&card);
    ui.global::<HeadphoneCorrection>().on_refresh(move || c.refresh());

    correction::install(ui, state, undo, &card);

    let followed = on_signal(&state.output_changed, ui.as_weak(), "headphones", move |_| {
        card.refresh();
    });
    if let Err(e) = followed {
        log::warn!("The headphone card won't follow the output: {}", describe(&e));
    }
}

pub(crate) struct Card {
    state: AppState,
    ui: slint::Weak<AppWindow>,
    /// `None` until the first naming lands.
    heard: RefCell<Option<Heard>>,
    /// Moved by each naming, so one landing late can't repaint over a newer.
    asked: Cell<u64>,
}

impl Card {
    pub(crate) fn state(&self) -> &AppState {
        &self.state
    }

    /// Names the device heard off the UI thread, the listing that names it being blocking.
    fn refresh(self: &Rc<Self>) {
        let asked = self.asked.get().wrapping_add(1);
        self.asked.set(asked);
        let state = self.state.clone();
        let naming = self.state.runtime.spawn_blocking(move || library::headphones::heard(&state));
        let card = Rc::downgrade(self);
        let spawned = slint::spawn_local(Compat::new(async move {
            let heard = match naming.await {
                Ok(heard) => heard,
                Err(e) => {
                    log::warn!("headphones: naming the device failed: {}", describe(&e));
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
            log::warn!("headphones: naming not spawned: {}", describe(&e));
        }
    }

    /// The device a write lands on, `None` until the card has named one.
    pub(crate) fn target(&self) -> Option<DeviceTarget> {
        self.heard.borrow().as_ref().map(|heard| heard.target.clone())
    }

    /// The device as last named, and what each row holds for it as the maps stand now.
    pub(crate) fn paint(&self) {
        let Some(ui) = self.ui.upgrade() else { return };
        let g = ui.global::<HeadphoneCorrection>();
        let heard = self.heard.borrow();
        let Some(heard) = heard.as_ref() else {
            g.set_device(HeardDevice::Pending);
            return;
        };
        g.set_device(heard_device(heard));
        g.set_device_name(heard.target.device_name.clone().unwrap_or_default().into());
        correction::paint(&ui, &self.state, heard.target.device.as_deref());
    }
}

fn heard_device(heard: &Heard) -> HeardDevice {
    match (&heard.target.device, heard.connected) {
        (None, _) => HeardDevice::SystemDefault,
        (Some(_), true) => HeardDevice::Connected,
        (Some(_), false) => HeardDevice::NotConnected,
    }
}
