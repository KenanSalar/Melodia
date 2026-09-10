//! Save: diff the live form against the populate-time snapshot into a [`TagEdit`], apply it, and
//! report what happened.
//!
//! Numbers cross the Slint boundary as strings, so all parse and validation lives here; only
//! touched fields become a `Set` / `Clear`.

use std::cell::RefCell;
use std::rc::Rc;

use async_compat::Compat;
use slint::ComponentHandle;

use crate::ui::shell::notifications::NotificationsUi;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::artist::ArtistCredit;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::tags::{ArtworkEdit, FieldEdit, RoleCreditEdit, TagEdit};
use melodia_ui::{AppWindow, TagEditor};

use super::credits::credit_from_model;
use super::form::{CREDIT_ALBUM_ARTIST, CREDIT_ARTIST, FormState, ListFields, read_form};
use super::lists::{genres_from_model, roles_from_model};
use super::session::TagSession;
use super::toast::show_report_toast;

/// `commit`: diff the form → `TagEdit` → apply → completion toast.
pub(super) fn wire_commit(
    te: &TagEditor,
    ui: &AppWindow,
    state: &AppState,
    session: &Rc<RefCell<TagSession>>,
    notifications: &Rc<NotificationsUi>,
) {
    let weak = ui.as_weak();
    let state = state.clone();
    let session = session.clone();
    let notifications = notifications.clone();
    te.on_commit(move || {
        let Some(ui) = weak.upgrade() else { return };
        let te = ui.global::<TagEditor>();

        let (edit, ids, artwork_source) = {
            let sess = session.borrow();
            let lists = ListFields {
                credits: [
                    credit_from_model(&ui, CREDIT_ARTIST, &sess.join_phrases),
                    credit_from_model(&ui, CREDIT_ALBUM_ARTIST, &sess.join_phrases),
                ],
                genres: genres_from_model(&ui),
                roles: roles_from_model(&ui, &sess.original_lists.roles),
            };
            let edit = build_edit(
                &read_form(&te),
                &sess.originals,
                &lists,
                &sess.original_lists,
                sess.artwork.clone(),
            );
            (edit, sess.ids.clone(), sess.picked.clone())
        };

        // A reflexive open-then-Save must not touch disk (lofty rewrites the
        // tag whether or not anything changed). `is_noop()` already folds in
        // the artwork tri-state.
        if edit.is_noop() {
            return;
        }

        let s = state.clone();
        let weak = weak.clone();
        let notifications = notifications.clone();
        let _ = slint::spawn_local(Compat::new(async move {
            let result = library::tags::apply_tag_edit(&s, ids, edit, artwork_source).await;
            let Some(ui) = weak.upgrade() else { return };
            show_report_toast(&ui, &notifications, result);
        }));
    });
}

/// Build the `TagEdit` by diffing the live form against the populate-time snapshot, field by
/// named field.
fn build_edit(
    form: &FormState,
    orig: &FormState,
    lists: &ListFields,
    was_lists: &ListFields,
    artwork: ArtworkEdit,
) -> TagEdit {
    let (cur, was) = (&form.text, &orig.text);
    TagEdit {
        title: diff_str(&cur.title, &was.title),
        artist: diff_multi(&lists.credits[CREDIT_ARTIST], &was_lists.credits[CREDIT_ARTIST]),
        album_artist: diff_multi(
            &lists.credits[CREDIT_ALBUM_ARTIST],
            &was_lists.credits[CREDIT_ALBUM_ARTIST],
        ),
        album: diff_str(&cur.album, &was.album),
        genres: diff_multi(&lists.genres, &was_lists.genres),
        credits: diff_multi(&lists.roles, &was_lists.roles),
        year: diff_parsed::<u16>(&cur.year, &was.year),
        original_year: diff_parsed::<u16>(&cur.original_year, &was.original_year),
        track_number: diff_parsed::<u32>(&cur.track_number, &was.track_number),
        track_total: diff_parsed::<u32>(&cur.track_total, &was.track_total),
        disc_number: diff_parsed::<u32>(&cur.disc_number, &was.disc_number),
        disc_total: diff_parsed::<u32>(&cur.disc_total, &was.disc_total),
        disc_subtitle: diff_str(&cur.disc_subtitle, &was.disc_subtitle),
        subtitle: diff_str(&cur.subtitle, &was.subtitle),
        comment: diff_str(&cur.comment, &was.comment),
        bpm: diff_bpm(&cur.bpm, &was.bpm),
        initial_key: diff_str(&cur.initial_key, &was.initial_key),
        mood: diff_str(&cur.mood, &was.mood),
        grouping: diff_str(&cur.grouping, &was.grouping),
        work: diff_str(&cur.work, &was.work),
        movement: diff_str(&cur.movement, &was.movement),
        movement_number: diff_parsed::<u32>(&cur.movement_number, &was.movement_number),
        movement_total: diff_parsed::<u32>(&cur.movement_total, &was.movement_total),
        language: diff_str(&cur.language, &was.language),
        copyright: diff_str(&cur.copyright, &was.copyright),
        isrc: diff_str(&cur.isrc, &was.isrc),
        label: diff_str(&cur.label, &was.label),
        catalog_number: diff_str(&cur.catalog_number, &was.catalog_number),
        barcode: diff_str(&cur.barcode, &was.barcode),
        media: diff_str(&cur.media, &was.media),
        release_type: diff_str(&cur.release_type, &was.release_type),
        release_country: diff_str(&cur.release_country, &was.release_country),
        compilation: diff_flag(form.compilation, orig.compilation),
        lyrics: diff_str(&cur.lyrics, &was.lyrics),
        artwork,
        // The Edit-Tags dialog doesn't surface MusicBrainz ids; leaving them
        // `Keep` preserves whatever the file (or the auto-tag backfill) wrote.
        ..Default::default()
    }
}

/// Tri-state for a text field. Exact match ⇒ `Keep`; a now-blank value ⇒
/// `Clear`; otherwise `Set` the value verbatim (the reader trims on the way
/// back in). An exact compare (not trimmed) preserves lyrics whitespace.
fn diff_str(cur: &str, orig: &str) -> FieldEdit<String> {
    if cur == orig {
        FieldEdit::Keep
    } else if cur.trim().is_empty() {
        FieldEdit::Clear
    } else {
        FieldEdit::Set(cur.to_owned())
    }
}

/// A field edited as rows rather than as the string it renders as.
///
/// The trait exists so [`diff_multi`] is written once: the tri-state ladder is the dialog's
/// central semantic, and three copies of it are three chances for one field to start answering
/// `Keep` where its siblings answer `Clear`.
///
/// Named for the answer rather than for the state: spelled `is_empty` it shadows the inherent
/// method each impl below delegates to, so retiring one of those turns the delegation into
/// unbounded recursion that still compiles.
trait MultiValue: Clone + PartialEq {
    fn is_cleared(&self) -> bool;
}

/// Compared structurally, not through the rendered line: two different credits can render the same
/// string, and a phrase swapped for one that renders identically is still an edit the file should
/// receive. `Keep` is what makes an untouched multi-artist file safe, the list tag surviving a save
/// that never looked at it.
impl MultiValue for ArtistCredit {
    fn is_cleared(&self) -> bool {
        self.is_empty()
    }
}

/// Structural for the reason above: the rows are what the user edited and the line is derived from
/// them, so a reordering that renders the same string is still an edit.
impl MultiValue for GenreList {
    fn is_cleared(&self) -> bool {
        self.is_empty()
    }
}

/// **Every role the form answers for or nothing**, because that is what the writer clears: a set
/// built from the one box that changed would take every role beside it. So the answer is `Keep`
/// until *some* box moves, and then the set is rebuilt from every box at once. That also makes the
/// emptied box work — a role the user cleared is simply absent from the rebuild, and the writer
/// removes its key.
///
/// **Never `Clear`, and that is load-bearing rather than an oversight.** `FieldEdit::Clear` carries
/// no payload, so a writer reaching it has no scope to honour and clears all ten roles — which on
/// a selection that disagreed about one takes a credit nobody was shown. An emptied form arrives
/// as a `Set` of an empty set instead, and the scope then decides exactly what goes, which is what
/// `Clear` meant for the only selection that could have produced it.
impl MultiValue for RoleCreditEdit {
    fn is_cleared(&self) -> bool {
        false
    }
}

/// Tri-state for a field compared as a whole value.
fn diff_multi<T: MultiValue>(cur: &T, orig: &T) -> FieldEdit<T> {
    if cur == orig {
        FieldEdit::Keep
    } else if cur.is_cleared() {
        FieldEdit::Clear
    } else {
        FieldEdit::Set(cur.clone())
    }
}

/// Tri-state for a switch. `Set(false)` and `Clear` mean the same thing to the writer, so an
/// un-ticked box is `Set(false)` and removes the tag.
fn diff_flag(cur: bool, orig: bool) -> FieldEdit<bool> {
    if cur == orig {
        FieldEdit::Keep
    } else {
        FieldEdit::Set(cur)
    }
}

/// Tri-state for a numeric field that crosses the Slint boundary as its decimal
/// string. Unchanged ⇒ `Keep`; now-blank ⇒ `Clear`; a value that doesn't parse
/// degrades to `Keep` — never write garbage.
fn diff_parsed<T: std::str::FromStr>(cur: &str, orig: &str) -> FieldEdit<T> {
    if cur == orig {
        return FieldEdit::Keep;
    }
    let t = cur.trim();
    if t.is_empty() {
        return FieldEdit::Clear;
    }
    t.parse::<T>().map_or(FieldEdit::Keep, FieldEdit::Set)
}

fn diff_bpm(cur: &str, orig: &str) -> FieldEdit<f64> {
    if cur == orig {
        return FieldEdit::Keep;
    }
    let t = cur.trim();
    if t.is_empty() {
        return FieldEdit::Clear;
    }
    // `parse::<f64>()` accepts "nan" / "inf" — reject those and negatives so a
    // bogus tempo never reaches the writer.
    match t.parse::<f64>() {
        Ok(b) if b.is_finite() && b > 0.0 => FieldEdit::Set(b),
        _ => FieldEdit::Keep,
    }
}

#[cfg(test)]
#[path = "tests/commit_tests.rs"]
mod tests;
