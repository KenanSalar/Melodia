//! The two artist-credit editors: an ordered list of names with a phrase between each pair.
//!
//! Rust owns both models: a row view reports an edit and never writes what it was handed, which
//! is what lets the `Dropdown`s bind one-way and keep re-reading after a removal shifts every
//! index below it.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use melodia_core::entities::artist::{ArtistCredit, CreditedArtist};
use melodia_ui::{AppWindow, ArtistCreditRow, TagEditor};

use super::form::{CREDIT_ALBUM_ARTIST, CREDIT_ARTIST};
use super::session::TagSession;

pub(super) fn wire_credits(te: &TagEditor, ui: &AppWindow, session: &Rc<RefCell<TagSession>>) {
    // Installed once, and never replaced: a default-constructed `ModelRc` is a no-op model that
    // silently ignores `set_vec`, so `populate` needs a real one to be waiting for it.
    te.set_artists(ModelRc::new(VecModel::from(vec![blank_credit_row()])));
    te.set_album_artists(ModelRc::new(VecModel::from(vec![blank_credit_row()])));

    let weak = ui.as_weak();
    let sess = session.clone();
    te.on_add_credit(move |field| {
        let Some(ui) = weak.upgrade() else { return };
        let field = credit_field(field);
        with_credits_model(&ui, field, |vm| vm.push(blank_credit_row()));
        refresh_preview(&ui, &sess, field);
    });

    let weak = ui.as_weak();
    let sess = session.clone();
    te.on_remove_credit(move |field, row| {
        let Some(ui) = weak.upgrade() else { return };
        let field = credit_field(field);
        with_credits_model(&ui, field, |vm| {
            if let Ok(row) = usize::try_from(row)
                && row < vm.row_count()
            {
                vm.remove(row);
            }
            // A credit with no names is a state the editor should not be able to reach: the
            // field would read as "cleared" on save, which is not what removing a collaborator
            // means.
            if vm.row_count() == 0 {
                vm.push(blank_credit_row());
            }
        });
        refresh_preview(&ui, &sess, field);
    });

    let weak = ui.as_weak();
    let sess = session.clone();
    te.on_set_credit_name(move |field, row, name| {
        let Some(ui) = weak.upgrade() else { return };
        let field = credit_field(field);
        patch_credit_row(&ui, field, row, |mut r| {
            r.name = name;
            r
        });
        refresh_preview(&ui, &sess, field);
    });

    let weak = ui.as_weak();
    let sess = session.clone();
    te.on_set_credit_join(move |field, row, phrase| {
        let Some(ui) = weak.upgrade() else { return };
        let field = credit_field(field);
        patch_credit_row(&ui, field, row, |mut r| {
            r.join_index = phrase;
            r
        });
        refresh_preview(&ui, &sess, field);
    });
}

/// Clamp a Slint-side field discriminator onto one of the two credits.
fn credit_field(field: i32) -> usize {
    if usize::try_from(field).unwrap_or(CREDIT_ARTIST) == CREDIT_ALBUM_ARTIST {
        CREDIT_ALBUM_ARTIST
    } else {
        CREDIT_ARTIST
    }
}

/// The model behind one of the two credits. Spelled once: three call sites used to carry the same
/// branch, which is two chances for the album artist's rows to be written into the artist's.
fn credit_model(te: &TagEditor, field: usize) -> ModelRc<ArtistCreditRow> {
    if field == CREDIT_ALBUM_ARTIST { te.get_album_artists() } else { te.get_artists() }
}

fn blank_credit_row() -> ArtistCreditRow {
    ArtistCreditRow { name: SharedString::new(), join_index: 0 }
}

fn with_credits_model<R>(
    ui: &AppWindow,
    field: usize,
    f: impl FnOnce(&VecModel<ArtistCreditRow>) -> R,
) -> Option<R> {
    credit_model(&ui.global::<TagEditor>(), field)
        .as_any()
        .downcast_ref::<VecModel<ArtistCreditRow>>()
        .map(f)
}

fn patch_credit_row(
    ui: &AppWindow,
    field: usize,
    row: i32,
    f: impl FnOnce(ArtistCreditRow) -> ArtistCreditRow,
) {
    with_credits_model(ui, field, |vm| {
        if let Ok(row) = usize::try_from(row)
            && let Some(old) = vm.row_data(row)
        {
            vm.set_row_data(row, f(old));
        }
    });
}

/// Re-render the credit line under the rows after every edit, so the user sees what will be
/// written rather than having to picture the join phrases in place.
fn refresh_preview(ui: &AppWindow, session: &Rc<RefCell<TagSession>>, field: usize) {
    let credit = credit_from_model(ui, field, &session.borrow().join_phrases);
    let line = SharedString::from(credit.line().unwrap_or_default());
    let te = ui.global::<TagEditor>();
    if field == CREDIT_ALBUM_ARTIST {
        te.set_album_artist_preview(line);
    } else {
        te.set_artist_preview(line);
    }
}

/// The credit the rows currently spell.
///
/// Blank names drop out, so a row added and left empty writes nothing, and the **last** surviving
/// name loses its phrase whatever its picker says — there is nothing after it to join to, and a
/// trailing " feat. " in the file is exactly the kind of thing nobody notices until another
/// player shows it.
pub(super) fn credit_from_model(
    ui: &AppWindow,
    field: usize,
    phrases: &[(String, String)],
) -> ArtistCredit {
    let rows =
        with_credits_model(ui, field, |vm| vm.iter().collect::<Vec<_>>()).unwrap_or_default();
    let named: Vec<ArtistCreditRow> =
        rows.into_iter().filter(|r| !r.name.trim().is_empty()).collect();
    let last = named.len().saturating_sub(1);
    let artists = named
        .iter()
        .enumerate()
        .map(|(i, r)| CreditedArtist {
            name: r.name.trim().to_owned(),
            join_phrase: if i == last { String::new() } else { phrase_at(phrases, r.join_index) },
        })
        .collect();
    ArtistCredit::new(artists)
}

fn phrase_at(phrases: &[(String, String)], index: i32) -> String {
    usize::try_from(index)
        .ok()
        .and_then(|i| phrases.get(i))
        .map_or_else(String::new, |(_, rendered)| rendered.clone())
}

/// The rows for one credit, registering any phrase the built-in list doesn't have.
pub(super) fn rows_from_credit(
    credit: &ArtistCredit,
    phrases: &mut Vec<(String, String)>,
) -> Vec<ArtistCreditRow> {
    let rows: Vec<ArtistCreditRow> = credit
        .artists()
        .iter()
        .map(|a| ArtistCreditRow {
            name: SharedString::from(a.name.as_str()),
            join_index: register_phrase(phrases, &a.join_phrase),
        })
        .collect();
    if rows.is_empty() { vec![blank_credit_row()] } else { rows }
}

/// The picker index for `rendered`, appending it when the file uses a phrase the built-in list
/// has never heard of.
///
/// Round-tripping somebody else's " meets " matters more than a tidy picker: without this, opening
/// such a file and saving any field at all would silently rewrite the credit to whatever sits at
/// index 0.
fn register_phrase(phrases: &mut Vec<(String, String)>, rendered: &str) -> i32 {
    if rendered.is_empty() {
        return 0;
    }
    if let Some(i) = phrases.iter().position(|(_, known)| known == rendered) {
        return i32::try_from(i).unwrap_or(0);
    }
    phrases.push((rendered.trim().to_owned(), rendered.to_owned()));
    i32::try_from(phrases.len() - 1).unwrap_or(0)
}

/// Replace one credit's rows in place. The model is installed once by [`wire_credits`], so this
/// refills it rather than handing over a new one.
pub(super) fn write_credit_rows(te: &TagEditor, field: usize, rows: Vec<ArtistCreditRow>) {
    if let Some(vm) = credit_model(te, field).as_any().downcast_ref::<VecModel<ArtistCreditRow>>() {
        vm.set_vec(rows);
    }
}

#[cfg(test)]
#[path = "tests/credits_tests.rs"]
mod tests;
