//! What the undo slot holds: one arm per surface, each wrapping the token its surface hands over.
//! Each arm's restore is its own surface's, so no surface names another's.

use slint::{ComponentHandle, SharedString};

use crate::ui::equalizer::ResetUndo;
use crate::ui::search::RecentClearUndo;
use crate::ui::shell::notifications::RowText;
use melodia_app::state::AppState;
use melodia_ui::{AppWindow, UndoText};

/// Renders the notice in the language active now.
pub(super) type Recipe = Box<dyn Fn(&AppWindow) -> RowText>;

pub enum Removal {
    Equalizer(ResetUndo),
    RecentSearches(RecentClearUndo),
}

impl Removal {
    /// Takes `next` into this hold when both took from the same place, handing it back otherwise.
    /// A whole-curve hold keeps the older curve: undoing a run newest first ends there anyway.
    pub(super) fn absorb(&mut self, next: Removal) -> Option<Removal> {
        match (self, next) {
            (Removal::Equalizer(_), Removal::Equalizer(_)) => None,
            (Removal::RecentSearches(held), Removal::RecentSearches(next)) => {
                held.absorb(next);
                None
            }
            (_, next) => Some(next),
        }
    }

    pub(super) fn recipe(&self) -> Recipe {
        match self {
            Removal::Equalizer(_) => titled(|ui| ui.global::<UndoText>().invoke_equalizer_reset()),
            Removal::RecentSearches(_) => {
                titled(|ui| ui.global::<UndoText>().invoke_recent_searches_cleared())
            }
        }
    }

    pub(super) fn restore(self, state: &AppState, ui: &slint::Weak<AppWindow>) {
        match self {
            Removal::Equalizer(reset) => reset.restore(),
            Removal::RecentSearches(cleared) => cleared.restore(state, ui),
        }
    }
}

/// A notice whose title says it all.
fn titled(title: fn(&AppWindow) -> SharedString) -> Recipe {
    Box::new(move |ui: &AppWindow| RowText {
        title: title(ui),
        message: SharedString::default(),
        action_label: ui.global::<UndoText>().invoke_undo_label(),
    })
}
