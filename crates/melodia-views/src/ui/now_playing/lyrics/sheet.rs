//! Getting a sheet for whatever is playing, and handing it back.
//!
//! **Three unrelated edges reach [`reseed`], and dropping any one of them shows an empty panel**:
//! a track change, the view re-opening and the header's switch. It is idempotent by
//! [`super::LyricsUi::holds`], so they may overlap freely — whichever gets there first pays.

use std::rc::Rc;
use std::sync::Arc;

use async_compat::Compat;
use slint::{ComponentHandle, Weak};

use super::rows::rows_for;
use super::{Claim, LyricsUi, NowPlayingState, OnAir, follow};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::lyrics::{Lyrics as Sheet, LyricsOutcome, LyricsSource};
use melodia_core::entities::track::TrackSummary;
use melodia_core::utils::toast::{self, ToastKind};
use melodia_ui::{AppWindow, Lyrics, LyricsState};

/// What the panel would show words for.
enum Playing {
    Track(Arc<TrackSummary>),
    OnAir(OnAir),
}

impl Playing {
    /// What is playing, where it is something with words to look up: a track, or a station's song
    /// that names an artist.
    fn now(np_state: &NowPlayingState) -> Option<Self> {
        if let Some(track) = playing_track(np_state) {
            return Some(Self::Track(track));
        }
        np_state.on_air.borrow().clone().map(Self::OnAir)
    }

    fn claim(&self) -> Claim {
        match self {
            Self::Track(track) => Claim::Track(track.file_path.clone()),
            Self::OnAir(on_air) => Claim::OnAir(on_air.clone()),
        }
    }
}

/// The sheet for what is playing.
///
/// A failure is logged and reads as "nothing found": a sidecar in some old codepage or a directory
/// that is down are both, to a reader, a panel with no words in it, and neither is worth a toast.
async fn fetch(state: &AppState, playing: &Playing) -> LyricsOutcome {
    let fetched = match playing {
        Playing::Track(track) => library::lyrics::for_track(state, track).await,
        Playing::OnAir(on_air) => library::lyrics::for_live(state, &on_air.song).await,
    };
    fetched.unwrap_or_else(|e| {
        log::debug!("ui::now_playing lyrics: {}", melodia_core::error::describe(&e));
        LyricsOutcome::Absent
    })
}

/// Bring the panel in line with whatever is playing, looking a sheet up if it has to.
///
/// **Three unrelated edges reach this, and dropping any one of them shows an empty panel.** The
/// sheet is handed back on every close, which is the trade this feature makes against holding a
/// resident copy for the life of the process — so a re-open of the *same* track has nothing to
/// paint, and the artwork's already-applied guard has no way to know that. The three are a track
/// change, the view re-opening and the header's switch.
///
/// Idempotent by [`LyricsUi::holds`], so the edges may overlap freely: whichever gets there first
/// pays, and the claim is taken before the first `.await` rather than after the last.
fn reseed(weak: &Weak<AppWindow>, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let Some(ui) = weak.upgrade() else { return };
    let ly = &np_state.lyrics;

    // **Both terms, and the mount is the one that is not obvious.** The switch is the persisted
    // setting rather than "the panel is mounted", so on its own it answers `true` for a closed
    // view behind which the source-change path is still running. That would spend a request per
    // track on a panel nobody can see. Two surfaces mount one, the full view and the
    // miniplayer's column; the card and the strip have no slot for it whatever they paint on.
    let wanted = np_state.renders_panel() && library::lyrics::is_enabled(state);
    let Some(playing) = Playing::now(np_state).filter(|_| wanted) else {
        // Closed, switched off, or a station announcing nothing with an artist in it. Either way
        // the previous song's sheet must not sit under it.
        if ly.holding.borrow().is_some() {
            release(&ui, ly);
        }
        return;
    };
    let claim = playing.claim();
    if ly.holds(&claim) {
        return;
    }

    mark_loading(&ui, ly, claim.clone());
    let weak = weak.clone();
    let state = state.clone();
    let np_state = np_state.clone();
    let res = slint::spawn_local(Compat::new(async move {
        let outcome = fetch(&state, &playing).await;
        let Some(ui) = weak.upgrade() else { return };
        // The song may have moved under the lookup; the claim taken above is what says so, and it
        // is the same test the two other edges dedupe on.
        if !np_state.lyrics.holds(&claim) {
            return;
        }
        apply(&ui, &np_state.lyrics, claim, &outcome, state.lyrics_online_enabled.get());
    }));
    if let Err(e) = res {
        log::warn!("ui::now_playing lyrics reseed task spawn_local: {e}");
    }
}

/// Wire the menu's two actions, which need the same `NowPlayingState` the reseed hook does.
///
/// **Both act on whatever is playing, read at click time rather than captured.** The menu is a
/// popup over a view that outlives any one track, so a handle taken at wire time names the wrong
/// song by the second verse.
pub(crate) fn wire_menu(ui: &AppWindow, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let global = ui.global::<Lyrics>();

    {
        let weak = ui.as_weak();
        let state = state.clone();
        let np_state = np_state.clone();
        global.on_refresh(move || {
            let Some(track) = playing_track(&np_state) else {
                return;
            };
            let Some(ui) = weak.upgrade() else { return };

            // Given up before the lookup, not after: the claim is what stops the reseed below
            // deduping this against the sheet it is meant to replace.
            release(&ui, &np_state.lyrics);

            let weak = weak.clone();
            let state = state.clone();
            let np_state = np_state.clone();
            let res = slint::spawn_local(Compat::new(async move {
                if let Err(e) = library::lyrics::forget(&state, &track.file_path).await {
                    log::warn!("lyrics refresh: {}", melodia_core::error::describe(&e));
                }
                if weak.upgrade().is_some() {
                    np_state.lyrics.kick();
                }
            }));
            if let Err(e) = res {
                log::warn!("ui::now_playing lyrics refresh spawn_local: {e}");
            }
        });
    }

    {
        let weak = ui.as_weak();
        let state = state.clone();
        let np_state = np_state.clone();
        global.on_save_to_tag(move || {
            let Some(track) = playing_track(&np_state) else {
                return;
            };
            let state = state.clone();
            let weak = weak.clone();
            let np_state = np_state.clone();
            let res = slint::spawn_local(Compat::new(async move {
                let found = library::lyrics::resident_text(&state, &track.file_path).await;
                let text = match found {
                    Ok(Some(text)) => text,
                    // The row is only offered against a sheet on screen, so nothing here is the
                    // store having lost the copy it was resolved from.
                    Ok(None) => {
                        log::debug!("lyrics save: nothing resident for {}", track.id);
                        return;
                    }
                    Err(e) => return report_save_failure(&e),
                };
                if let Err(e) = library::lyrics::write_to_tag(&state, track.id, &text).await {
                    return report_save_failure(&e);
                }
                toast::notify(ToastKind::LyricsSaved, track.title.clone());
                // The tag now holds what the panel is showing, so the row that offered this has
                // nothing left to do — and the sheet is the file's own from here.
                let Some(ui) = weak.upgrade() else { return };
                ui.global::<Lyrics>().set_can_save_to_tag(false);
                np_state.lyrics.holding.borrow_mut().take();
                np_state.lyrics.kick();
            }));
            if let Err(e) = res {
                log::warn!("ui::now_playing lyrics save spawn_local: {e}");
            }
        });
    }
}

/// Say a save did not happen, where it was asked for by name.
///
/// **A toast rather than a log line**, the radio vote's rule: this runs only because somebody
/// pressed a control that says it writes to their file, and a control that quietly does nothing is
/// worse than one that says why.
fn report_save_failure(e: &melodia_core::error::AppError) {
    let reason = melodia_core::error::describe(e);
    log::warn!("lyrics save: {reason}");
    toast::notify(ToastKind::OperationFailed, reason);
}

/// The track on the deck, or `None` on a station or an empty queue.
fn playing_track(np_state: &NowPlayingState) -> Option<Arc<TrackSummary>> {
    np_state.current_source.borrow().as_ref().and_then(|s| s.track.clone())
}

/// Wire the reseed hook, once the `NowPlayingState` this panel's state is a field of exists.
pub(crate) fn wire_reseed(ui: &AppWindow, state: &AppState, np_state: &Rc<NowPlayingState>) {
    let weak_ui = ui.as_weak();
    let state = state.clone();
    let weak_np = Rc::downgrade(np_state);
    *np_state.lyrics.reseed.borrow_mut() = Some(Box::new(move || {
        let Some(np_state) = weak_np.upgrade() else {
            return;
        };
        reseed(&weak_ui, &state, &np_state);
    }));
}

/// Write an outcome into the panel.
///
/// `online_enabled` comes from the caller rather than being read here, because it decides only
/// which of two sentences an empty panel shows and the caller is the half holding `AppState`.
fn apply(
    ui: &AppWindow,
    ly: &Rc<LyricsUi>,
    claim: Claim,
    outcome: &LyricsOutcome,
    online_enabled: bool,
) {
    ly.pinned.set(None);
    let global = ui.global::<Lyrics>();
    // Only a track has a tag to write a sheet into.
    let has_tag = matches!(claim, Claim::Track(_));
    // A station's song is followed from where it started, and a song joined midway has no start
    // to follow from, so it is printed whole.
    let (followable, origin) = match &claim {
        Claim::Track(_) => (true, None),
        Claim::OnAir(on_air) => (
            on_air.started_ms.is_some(),
            on_air.started_ms.map(|ms| i32::try_from(ms).unwrap_or(i32::MAX)),
        ),
    };
    ly.live_origin_ms.set(origin);

    // Recorded whichever of the three this came to, so a re-open that finds the same track already
    // answered does not pay for a second lookup — including the two that draw no rows, which are
    // answers rather than absences.
    *ly.holding.borrow_mut() = Some(claim);

    match outcome {
        LyricsOutcome::Sheet(sheet) => {
            let untimed;
            let sheet = if followable {
                sheet
            } else {
                untimed = without_timings(sheet);
                &untimed
            };
            take_sheet(ly, sheet);
            follow::republish(ui, ly);
            global.set_synced(sheet.is_synced());
            // Every source but the file's own tag is worth offering to write into it. A sidecar
            // counts: it is the user's file, but it is not the one that travels with the track.
            global.set_can_save_to_tag(has_tag && sheet.source != LyricsSource::Tag);
            global.set_state(LyricsState::Ready);
        }
        LyricsOutcome::Instrumental => {
            clear(ui, ly);
            global.set_state(LyricsState::Instrumental);
        }
        LyricsOutcome::Absent => {
            clear(ui, ly);
            // The one state that names a setting, being the only one a reader can act on here.
            global.set_state(if online_enabled { LyricsState::Missing } else { LyricsState::Off });
        }
        // **The claim above is still recorded, deliberately.** Nothing retries on its own, so
        // without it every tick that reaches `reseed` would start another lookup against a service
        // that has just told us to stop. Refresh is the retry, and it drops the claim itself.
        LyricsOutcome::Unavailable => {
            clear(ui, ly);
            global.set_state(LyricsState::Unavailable);
        }
    }
}

/// Say a lookup is out, so the panel is not claiming "not found" while it is still looking.
///
/// **Takes the claim before the `.await`, not after it.** The lookup can reach a file and then a
/// socket, and a reseed landing in that window would otherwise see a sheet for the *previous*
/// song and start a second one for the same song.
fn mark_loading(ui: &AppWindow, ly: &Rc<LyricsUi>, claim: Claim) {
    clear(ui, ly);
    *ly.holding.borrow_mut() = Some(claim);
    ui.global::<Lyrics>().set_state(LyricsState::Loading);
}

/// Hand back the rows and the table, on the view's own teardown.
///
/// Gives up the claim with them, so the next open looks the sheet up again rather than believing
/// it is still on screen. That belief is what left a re-opened view blank until a restart.
pub(crate) fn release(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    ly.holding.borrow_mut().take();
    clear(ui, ly);
    ui.global::<Lyrics>().set_state(LyricsState::Idle);
}

fn clear(ui: &AppWindow, ly: &Rc<LyricsUi>) {
    ly.rows.borrow_mut().clear();
    ly.offsets.borrow_mut().clear();
    ly.pinned.set(None);
    ly.model.set_vec(Vec::new());

    let global = ui.global::<Lyrics>();
    global.set_synced(false);
    global.set_can_save_to_tag(false);
    global.set_active_index(-1);
    global.set_active_offset(0.0);
    global.set_active_height(0.0);
}

/// Keep the sheet's text and stamps. Heights are not kept, following the width instead.
fn take_sheet(ly: &Rc<LyricsUi>, sheet: &Sheet) {
    *ly.rows.borrow_mut() = rows_for(sheet);
}

/// The sheet printed whole, for a song whose stamps have no clock to be read against.
fn without_timings(sheet: &Sheet) -> Sheet {
    let mut sheet = sheet.clone();
    for line in &mut sheet.lines {
        line.at_ms = None;
        line.end_ms = None;
    }
    sheet
}
