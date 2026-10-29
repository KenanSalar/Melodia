//! What the undo slot holds: one arm per surface, each wrapping the token its surface hands over.
//! Each arm's restore is its own surface's, so no surface names another's.

use slint::{ComponentHandle, SharedString};

use crate::ui::equalizer::ResetUndo;
use crate::ui::shell::notifications::RowText;
use melodia_ui::{AppWindow, UndoText};

/// Renders the notice in the language active now.
pub(super) type Recipe = Box<dyn Fn(&AppWindow) -> RowText>;

pub enum Removal {
    Equalizer(ResetUndo),
}

impl Removal {
    /// Takes `next` into this hold when both took from the same place, handing it back otherwise.
    /// A whole-curve hold keeps the older curve: undoing a run newest first ends there anyway.
    pub(super) fn absorb(&mut self, next: Removal) -> Option<Removal> {
        match (self, next) {
            (Removal::Equalizer(_), Removal::Equalizer(_)) => None,
        }
    }

    pub(super) fn recipe(&self) -> Recipe {
        match self {
            Removal::Equalizer(_) => Box::new(|ui: &AppWindow| {
                let text = ui.global::<UndoText>();
                RowText {
                    title: text.invoke_equalizer_reset(),
                    message: SharedString::default(),
                    action_label: text.invoke_undo_label(),
                }
            }),
        }
    }

    pub(super) fn restore(self) {
        match self {
            Removal::Equalizer(reset) => reset.restore(),
        }
    }
}
