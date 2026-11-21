//! The Find lyrics dialog: lrclib.net's matches for the playing track, the picked one's lyrics,
//! and the pick stored as the track's online answer.
//!
//! The track is the one playing at the open, held until the close, so a song that ends under the
//! dialog doesn't take the pick.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use tokio::runtime::Handle;

use super::NowPlayingState;
use super::sheet::{self, playing_track};
use crate::ui::dialog_landing::{DialogTicket, TicketedDialog};
use crate::ui::tracks::format_duration_ms;
use crate::ui::util::len_as_i32;
use melodia_app::library::lyrics::LyricsKey;
use melodia_app::library::lyrics::finder::{self, Choice, ChoiceKind, Outranking, SearchFailure};
use melodia_app::state::AppState;
use melodia_core::entities::lyrics::LyricsCandidate;
use melodia_core::entities::track::TrackSummary;
use melodia_core::error::describe;
use melodia_ui::{
    AppWindow, Dialog, DialogKind, Lyrics, LyricsFinder, LyricsMasking, LyricsSearchPhase, PickRow,
};

pub(crate) fn wire_finder(ui: &AppWindow, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let finder = Rc::new(Finder {
        ui: ui.as_weak(),
        state: state.clone(),
        np_state: Rc::clone(np_state),
        ticket: DialogTicket::new(DialogKind::LyricsFinder),
        rows: Rc::new(VecModel::default()),
        track: RefCell::new(None),
        choices: RefCell::new(Vec::new()),
        outranking: Cell::new(Outranking::Nothing),
    });
    let g = ui.global::<LyricsFinder>();
    g.set_rows(ModelRc::from(finder.rows.clone()));

    let f = Rc::clone(&finder);
    g.on_load(move || f.load());
    let f = Rc::clone(&finder);
    g.on_turn_on_lookup(move || f.turn_on_lookup());
    let f = Rc::clone(&finder);
    g.on_search(move |query| f.search(query.into()));
    let f = Rc::clone(&finder);
    g.on_picked_changed(move |index| f.pick(index));
    let f = Rc::clone(&finder);
    g.on_apply(move |index| f.apply(index));
    g.on_released(move || finder.release());
}

struct Finder {
    ui: slint::Weak<AppWindow>,
    state: AppState,
    np_state: Rc<NowPlayingState>,
    ticket: DialogTicket,
    rows: Rc<VecModel<PickRow>>,
    track: RefCell<Option<Arc<TrackSummary>>>,
    /// The last search's answer, which a row's `index` points into.
    choices: RefCell<Vec<Choice>>,
    /// Read beside each search, so it lands with the rows it describes under one ticket.
    outranking: Cell<Outranking>,
}

impl Finder {
    fn load(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else { return };
        let g = ui.global::<LyricsFinder>();
        let Some(track) = playing_track(&self.np_state) else {
            g.set_phase(LyricsSearchPhase::Failed);
            return;
        };
        *self.track.borrow_mut() = Some(track);

        // Read here rather than met as the facade's refusal, so the card opens on the notice.
        if !self.state.lyrics_online_enabled.get() {
            g.set_phase(LyricsSearchPhase::LookupOff);
            return;
        }
        self.open_on_default_query();
    }

    /// Through the menu switch's own handler, which also sends the panel to look again.
    fn turn_on_lookup(self: &Rc<Self>) {
        let Some(ui) = self.ui.upgrade() else { return };
        ui.global::<Lyrics>().invoke_set_online_enabled(true);
        ui.global::<LyricsFinder>().set_phase(LyricsSearchPhase::Searching);
        self.open_on_default_query();
    }

    fn open_on_default_query(self: &Rc<Self>) {
        let Some(track) = self.track.borrow().clone() else { return };
        let state = self.state.clone();
        let opening = async move { finder::default_query(&state, &track).await };
        let finder = Rc::clone(self);
        self.after(opening, move |ui, _, query| {
            ui.global::<LyricsFinder>().set_query(SharedString::from(query.as_str()));
            finder.search(query);
        });
    }

    /// A fresh answer may rank the sheets differently, so the pick goes with the old rows.
    fn search(self: &Rc<Self>, query: String) {
        let Some(track) = self.track.borrow().clone() else { return };
        let Some(ui) = self.ui.upgrade() else { return };
        let g = ui.global::<LyricsFinder>();
        // The ticket this moves is the one a pick being stored closes the dialog under.
        if g.get_applying() {
            return;
        }

        self.ticket.retire();
        self.choices.take();
        self.rows.set_vec(Vec::new());
        g.set_picked(-1);
        g.set_preview(SharedString::new());
        g.set_masking(LyricsMasking::None);
        g.set_phase(LyricsSearchPhase::Searching);

        let state = self.state.clone();
        let key = LyricsKey::from(&*track);
        let found = async move {
            // A failed read costs only the warning a hidden pick would carry.
            let outranking = finder::outranking(&state, key).await.unwrap_or_else(|e| {
                log::debug!("lyrics finder: the track's own lyrics: {}", describe(&e));
                Outranking::Nothing
            });
            (finder::search(&state, &track, &query).await, outranking)
        };
        self.after(found, |ui, finder, (found, outranking)| {
            let g = ui.global::<LyricsFinder>();
            match found {
                Ok(choices) => finder.show(&g, choices, outranking),
                Err(SearchFailure::RateLimited { .. }) => {
                    g.set_phase(LyricsSearchPhase::RateLimited);
                }
                Err(SearchFailure::Failed(e)) => {
                    log::warn!("lyrics finder: {}", describe(&e));
                    g.set_phase(LyricsSearchPhase::Failed);
                }
            }
        });
    }

    fn show(&self, g: &LyricsFinder<'_>, choices: Vec<Choice>, outranking: Outranking) {
        self.rows.set_vec(choices.iter().enumerate().map(row).collect::<Vec<_>>());
        *self.choices.borrow_mut() = choices;
        self.outranking.set(outranking);
        g.set_phase(LyricsSearchPhase::Listing);
    }

    fn pick(&self, index: i32) {
        let choices = self.choices.borrow();
        let Some(choice) = usize::try_from(index).ok().and_then(|i| choices.get(i)) else {
            return;
        };
        let Some(ui) = self.ui.upgrade() else { return };
        let g = ui.global::<LyricsFinder>();
        g.set_preview(choice.preview.as_str().into());
        g.set_masking(masking(self.outranking.get(), choice.kind));
    }

    /// Lands whatever the ticket: a pick stored as the dialog closed still changed what the panel
    /// reads.
    fn apply(self: &Rc<Self>, index: i32) {
        let candidate = usize::try_from(index)
            .ok()
            .and_then(|i| self.choices.borrow().get(i).map(|choice| choice.candidate.clone()));
        let (Some(candidate), Some(track)) = (candidate, self.track.borrow().clone()) else {
            return;
        };
        let Some(ui) = self.ui.upgrade() else { return };
        let g = ui.global::<LyricsFinder>();
        g.set_apply_failed(false);

        let state = self.state.clone();
        let key = LyricsKey::from(&*track);
        let chosen = async move { finder::choose(&state, key, candidate).await };
        let ticket = self.ticket.get();
        let landed = self.after_any_ticket("lyrics pick", chosen, move |ui, finder, chosen| {
            if matches!(chosen, Some(Ok(()))) {
                finder.reload_panel(ui, track.id);
            }
            if !finder.ticket.holds(ticket, ui) {
                return;
            }
            let g = ui.global::<LyricsFinder>();
            g.set_applying(false);
            match chosen {
                Some(Ok(())) => ui.global::<Dialog>().set_open(false),
                Some(Err(e)) => {
                    log::warn!("lyrics pick: {}", describe(&e));
                    g.set_apply_failed(true);
                }
                None => g.set_apply_failed(true),
            }
        });
        g.set_applying(landed.is_some());
    }

    /// A panel showing the track gives its sheet back and looks again, finding the pick. One on
    /// another track, or none, reads the store on its own when it next shows this one.
    fn reload_panel(&self, ui: &AppWindow, track_id: i64) {
        let lyrics = &self.np_state.lyrics;
        if lyrics.holds(track_id) {
            sheet::release(ui, lyrics);
            lyrics.kick();
        }
    }

    fn release(&self) {
        self.ticket.retire();
        self.track.take();
        self.choices.take();
        self.outranking.set(Outranking::Nothing);
        self.rows.set_vec(Vec::new());
        let Some(ui) = self.ui.upgrade() else { return };
        let g = ui.global::<LyricsFinder>();
        g.set_preview(SharedString::new());
        g.set_masking(LyricsMasking::None);
        g.set_applying(false);
        g.set_apply_failed(false);
    }
}

fn row((index, choice): (usize, &Choice)) -> PickRow {
    let candidate = &choice.candidate;
    PickRow {
        index: len_as_i32(index),
        title: candidate.title.as_str().into(),
        subtitle: byline(candidate).into(),
        trailing: format_duration_ms(i64::from(candidate.duration_secs) * 1000).into(),
        badge: badge(choice.kind),
    }
}

/// Artist and album, either left out where the directory files none.
fn byline(candidate: &LyricsCandidate) -> String {
    [candidate.artist.as_str(), candidate.album.as_str()]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// An index into the labels `lyrics-finder-body.slint` lists in this order.
fn badge(kind: ChoiceKind) -> i32 {
    match kind {
        ChoiceKind::Words => 0,
        ChoiceKind::Lines => 1,
        ChoiceKind::Plain => 2,
        ChoiceKind::Instrumental => 3,
    }
}

fn masking(outranking: Outranking, kind: ChoiceKind) -> LyricsMasking {
    if !outranking.hides(kind) {
        return LyricsMasking::None;
    }
    match outranking {
        Outranking::Sidecar => LyricsMasking::Sidecar,
        Outranking::TimedTag => LyricsMasking::TimedTag,
        Outranking::PlainTag => LyricsMasking::PlainTag,
        Outranking::Nothing => LyricsMasking::None,
    }
}

impl TicketedDialog for Finder {
    const LABEL: &'static str = "lyrics finder";

    fn window(&self) -> &slint::Weak<AppWindow> {
        &self.ui
    }

    fn ticket(&self) -> &DialogTicket {
        &self.ticket
    }

    fn runtime(&self) -> &Handle {
        &self.state.runtime
    }
}
