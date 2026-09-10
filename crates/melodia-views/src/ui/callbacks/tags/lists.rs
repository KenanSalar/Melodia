//! The two name-list editors: the genre field, and the ten role boxes behind the Credits tab.
//!
//! One shape, mounted twice. A row view reports an edit and never writes what it was handed, so
//! Rust owns both models and every callback below routes through [`with_name_model`] — which is
//! also what keeps the three edits (add, remove, rename) spelled once rather than once per field.

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use melodia_core::entities::credits::{CreditRole, ROLES, RoleCredit, RoleCredits};
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::tags::RoleCreditEdit;
use melodia_ui::{AppWindow, TagEditor};

/// Run `f` over a name-row model, if it is the `VecModel` the wiring installed.
fn with_name_model(model: &ModelRc<SharedString>, f: impl FnOnce(&VecModel<SharedString>)) {
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<SharedString>>() {
        f(vm);
    }
}

/// A fresh model holding the one blank row every editor opens with.
fn blank_rows() -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(vec![SharedString::default()]))
}

/// Drop a row, keeping one blank behind when it was the last.
///
/// A field with no rows would save as "cleared", which is not what removing one name means.
fn remove_row(vm: &VecModel<SharedString>, row: i32) {
    if let Ok(row) = usize::try_from(row)
        && row < vm.row_count()
    {
        vm.remove(row);
    }
    if vm.row_count() == 0 {
        vm.push(SharedString::default());
    }
}

fn rename_row(vm: &VecModel<SharedString>, row: i32, name: SharedString) {
    if let Ok(row) = usize::try_from(row)
        && row < vm.row_count()
    {
        vm.set_row_data(row, name);
    }
}

/// Replace a model's rows, keeping one blank so the editor always has a field to type into.
fn write_rows(model: &ModelRc<SharedString>, names: impl Iterator<Item = SharedString>) {
    let mut rows: Vec<SharedString> = names.collect();
    if rows.is_empty() {
        rows.push(SharedString::default());
    }
    with_name_model(model, |vm| vm.set_vec(rows));
}

// ---- Genres ----

/// The genre list's three callbacks.
///
/// A model of `SharedString` rather than a row struct: a genre is a name and nothing else, where
/// an `ArtistCreditRow` exists to carry the phrase beside it.
pub(super) fn wire_genres(te: &TagEditor, ui: &AppWindow) {
    // Installed once and never replaced: a default-constructed `ModelRc` is a no-op model that
    // silently ignores `set_vec`, so `populate` needs a real one to be waiting for it.
    te.set_genres(blank_rows());

    let weak = ui.as_weak();
    te.on_add_genre(move || {
        let Some(ui) = weak.upgrade() else { return };
        with_name_model(&genre_model(&ui), |vm| vm.push(SharedString::default()));
    });

    let weak = ui.as_weak();
    te.on_remove_genre(move |row| {
        let Some(ui) = weak.upgrade() else { return };
        with_name_model(&genre_model(&ui), |vm| remove_row(vm, row));
    });

    let weak = ui.as_weak();
    te.on_set_genre_name(move |row, name| {
        let Some(ui) = weak.upgrade() else { return };
        with_name_model(&genre_model(&ui), |vm| rename_row(vm, row, name));
    });
}

fn genre_model(ui: &AppWindow) -> ModelRc<SharedString> {
    ui.global::<TagEditor>().get_genres()
}

/// Fill the genre rows.
pub(super) fn write_genre_rows(te: &TagEditor, genres: &GenreList) {
    write_rows(&te.get_genres(), genres.names().iter().map(|n| SharedString::from(n.as_str())));
}

/// The genre list the form currently shows, blanks dropped.
pub(super) fn genres_from_model(ui: &AppWindow) -> GenreList {
    let names = ui
        .global::<TagEditor>()
        .get_genres()
        .iter()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .collect();
    GenreList::new(names)
}

// ---- Role credits ----

/// The role lists' three callbacks — [`wire_genres`] once more, indexed by role.
///
/// One outer model of ten inner ones, so a role is a position rather than a property name and the
/// ten editors are one mount. `ROLES` is that position's meaning on both sides.
pub(super) fn wire_roles(te: &TagEditor, ui: &AppWindow) {
    let rows: Vec<ModelRc<SharedString>> = ROLES.iter().map(|_| blank_rows()).collect();
    te.set_role_names(ModelRc::new(VecModel::from(rows)));

    let weak = ui.as_weak();
    te.on_add_role(move |role| {
        let Some(ui) = weak.upgrade() else { return };
        with_role_model(&ui, role, |vm| vm.push(SharedString::default()));
    });

    let weak = ui.as_weak();
    te.on_remove_role(move |role, row| {
        let Some(ui) = weak.upgrade() else { return };
        with_role_model(&ui, role, |vm| remove_row(vm, row));
    });

    let weak = ui.as_weak();
    te.on_set_role_name(move |role, row, name| {
        let Some(ui) = weak.upgrade() else { return };
        with_role_model(&ui, role, |vm| rename_row(vm, row, name));
    });
}

/// Run `f` over one role's row model, if `role` names one and it is the model `wire_roles`
/// installed.
fn with_role_model(ui: &AppWindow, role: i32, f: impl FnOnce(&VecModel<SharedString>)) {
    let Ok(role) = usize::try_from(role) else {
        return;
    };
    let Some(inner) = ui.global::<TagEditor>().get_role_names().row_data(role) else {
        return;
    };
    with_name_model(&inner, f);
}

/// Fill every role's rows.
pub(super) fn write_role_rows(ui: &AppWindow, credits: &RoleCredits) {
    let outer = ui.global::<TagEditor>().get_role_names();
    for (index, role) in ROLES.into_iter().enumerate() {
        let Some(inner) = outer.row_data(index) else {
            continue;
        };
        write_rows(&inner, credits.for_role(role).map(|c| SharedString::from(c.name.as_str())));
    }
}

/// The role credit set the form currently shows, blanks dropped, scoped to what it can answer for.
///
/// Rebuilt in `ROLES` order, which is also the order the rows are mounted in, so the rendered
/// `credits` line is stable across a save that changed one name.
///
/// **The scope grows by what the user typed.** It opens as the roles the selection agreed on, and
/// a role it disagreed about joins once its box holds a name — that being the only way a batch
/// form can say anything about one. Left blank it stays out, so the credit each file carries
/// survives an edit to the role beside it. The consequence, and it is the same one every
/// ‹multiple values› field already has: a role the selection disagrees about cannot be *emptied*
/// across it, a blank box there meaning "leave this alone".
pub(super) fn roles_from_model(ui: &AppWindow, original: &RoleCreditEdit) -> RoleCreditEdit {
    let outer = ui.global::<TagEditor>().get_role_names();
    let mut credits = Vec::new();
    let mut answered = original.answered();

    for (index, role) in ROLES.into_iter().enumerate() {
        let Some(inner) = outer.row_data(index) else {
            continue;
        };
        let named: Vec<RoleCredit> = inner
            .iter()
            .filter_map(|name| {
                let name = name.trim();
                (!name.is_empty()).then(|| RoleCredit {
                    role,
                    detail: detail_for(original.credits(), role, name),
                    name: name.to_owned(),
                })
            })
            .collect();
        answered[index] |= !named.is_empty();
        credits.extend(named);
    }

    RoleCreditEdit::new(RoleCredits::new(credits), answered)
}

/// The instrument or voice `name` was credited with in `role`, carried across from what the file
/// said.
///
/// The editor has no field for it — only [`CreditRole::Performer`] has one at all, and a second
/// column on every row would be ten empty boxes to explain one. Without this the round trip is
/// *lossy in both directions*: the read-back would differ from the baseline for any track carrying
/// one, so the diff would answer `Set` on a form nobody touched and write the credit back with the
/// instrument stripped. Matched on the name, which is the only handle a row still has.
fn detail_for(original: &RoleCredits, role: CreditRole, name: &str) -> String {
    original
        .for_role(role)
        .find(|credit| credit.name.eq_ignore_ascii_case(name))
        .map(|credit| credit.detail.clone())
        .unwrap_or_default()
}

/// The role credits a selection agrees on, scoped to the roles it agreed about.
///
/// Per role rather than for the set as a whole: a batch that shares a composer and differs on the
/// producer should still show the composer, the way `common_str` shows every other field the
/// selection agrees on.
///
/// The scope is what makes that safe to save. A disagreeing role shows nothing, so the form cannot
/// speak for it and the writer must not clear it — [`RoleCreditEdit`] carries that, and the
/// placeholder row reads the same answer back off it.
pub(super) fn common_roles(sets: &[RoleCredits]) -> RoleCreditEdit {
    let mut agreed = Vec::new();
    let mut answered = [true; ROLES.len()];

    for (index, role) in ROLES.into_iter().enumerate() {
        let mut per_track = sets.iter().map(|set| set.for_role(role).cloned().collect::<Vec<_>>());
        let Some(first) = per_track.next() else {
            continue;
        };
        if per_track.all(|other| other == first) {
            agreed.extend(first);
        } else {
            answered[index] = false;
        }
    }

    RoleCreditEdit::new(RoleCredits::new(agreed), answered)
}

#[cfg(test)]
#[path = "tests/lists_tests.rs"]
mod tests;
