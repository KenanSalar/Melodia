//! Smart-playlist rule-builder (`Dialog.kind == "smart-playlist-editor"`,
//! `SmartEditor` global) wiring.
//!
//! The `rules` model is a Rust-owned `VecModel<SmartRuleRow>` installed here.
//! Each editor mutation (`add-rule` / `remove-rule` / `set-rule-*`) rewrites the
//! affected row, recomputing the row's `field-kind` / `input-kind` codes so the
//! Slint body can pick the right operator array and value widget. On field/op
//! change the value is only cleared when the *input kind* changes (so the
//! remounted input reads an empty value) — otherwise it is kept, keeping the
//! model and the visible input in agreement (see the body's split-by-input-kind
//! value widgets).
//!
//! Entry points populate everything and open the dialog from a fresh event-loop
//! tick — a synchronous `Dialog.open = true` inside a click callback trips
//! Slint's property-recursion guard (same reason the create-playlist open is
//! inline-Slint and the export open hops through Rust).

use std::sync::Arc;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::entities::smart_criteria as sc;
use crate::library;
use crate::state::AppState;
use crate::ui::playlists::{self as playlists_ui_mod, PlaylistsUi};
use crate::ui::util::clamp_i64_to_i32;
use crate::{AppWindow, Dialog, SmartEditor, SmartRuleRow};

/// Wire the Smart-Playlist editor callbacks + install its rules model. See
/// [`super::wire`].
pub(super) fn wire(ui: &AppWindow, state: &AppState, playlists_ui: &Arc<PlaylistsUi>) {
    let se = ui.global::<SmartEditor>();

    // Install the Rust-owned rules model (starts with one blank rule so a fresh
    // "New Smart Playlist" isn't empty).
    se.set_rules(ModelRc::new(VecModel::from(vec![default_rule_row()])));

    let weak = ui.as_weak();

    // add-rule.
    {
        let weak = weak.clone();
        se.on_add_rule(move || {
            let Some(ui) = weak.upgrade() else { return };
            with_rules_model(&ui, |vm| vm.push(default_rule_row()));
        });
    }

    // remove-rule.
    {
        let weak = weak.clone();
        se.on_remove_rule(move |row| {
            let Some(ui) = weak.upgrade() else { return };
            with_rules_model(&ui, |vm| {
                if let Ok(ri) = usize::try_from(row)
                    && ri < vm.row_count()
                {
                    vm.remove(ri);
                }
            });
        });
    }

    // set-rule-field — reset the operator to this field-kind's first, then
    // recompute the codes (clearing the value only if the input kind changed).
    {
        let weak = weak.clone();
        se.on_set_rule_field(move |row, field_idx| {
            let Some(ui) = weak.upgrade() else { return };
            patch_rule_row(&ui, row, |old| {
                let field = field_at(field_idx);
                let op = first_op(field.value_type());
                rebuilt_row(field, field_idx, op, 0, &old)
            });
        });
    }

    // set-rule-op — recompute the input kind for the new operator.
    {
        let weak = weak.clone();
        se.on_set_rule_op(move |row, op_idx| {
            let Some(ui) = weak.upgrade() else { return };
            patch_rule_row(&ui, row, |old| {
                let field = field_at(old.field_index);
                let op = op_at(field.value_type(), op_idx);
                rebuilt_row(field, old.field_index, op, op_idx, &old)
            });
        });
    }

    // set-rule-value — mirror the input into the model (commit reads it back).
    {
        let weak = weak.clone();
        se.on_set_rule_value(move |row, text| {
            let Some(ui) = weak.upgrade() else { return };
            patch_rule_row(&ui, row, |old| SmartRuleRow {
                value_text: text,
                ..old
            });
        });
    }

    // request-new — populate a fresh (default) editor and open on a fresh tick.
    {
        let weak = weak.clone();
        se.on_request_new(move || {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                populate_editor(&ui, "", "", &sc::SmartCriteria::default(), -1);
                ui.global::<Dialog>().set_open(true);
            });
        });
    }

    // request-edit — load the playlist's criteria, populate, open on a fresh
    // tick (the `upgrade_in_event_loop` lands on the next UI tick).
    {
        let weak = weak.clone();
        let state = state.clone();
        se.on_request_edit(move |playlist_id| {
            let id = i64::from(playlist_id);
            let s = state.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                let detail = match library::playlists::get_playlist_detail(&s, id).await {
                    Ok(p) => p,
                    Err(e) => {
                        log::warn!("smart edit fetch {id}: {e}");
                        return;
                    }
                };
                let name = detail.name;
                let description = detail.description.unwrap_or_default();
                let criteria = sc::SmartCriteria::from_json_opt(detail.smart_criteria.as_deref());
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    populate_editor(&ui, &name, &description, &criteria, id);
                    ui.global::<Dialog>().set_open(true);
                });
            });
        });
    }

    // commit — reconstruct the criteria and create / update.
    {
        let weak = weak.clone();
        let state = state.clone();
        let playlists_ui = playlists_ui.clone();
        se.on_commit(move || {
            let Some(ui) = weak.upgrade() else { return };
            let Some(draft) = collect_criteria(&ui) else { return };
            let CriteriaDraft {
                name,
                description,
                criteria,
                target_id,
            } = draft;

            let s = state.clone();
            let pu = playlists_ui.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if target_id < 0 {
                    match library::smart_playlists::create_smart_playlist(
                        &s,
                        name.clone(),
                        description,
                        &criteria,
                    )
                    .await
                    {
                        Ok(_) => {
                            if let Err(e) = playlists_ui_mod::fetch_grid(&s, &pu, weak).await {
                                log::warn!("smart create refetch: {e}");
                            }
                            log::info!("smart playlist created: {name:?}");
                        }
                        Err(e) => log::warn!("create smart playlist {name:?}: {e}"),
                    }
                } else {
                    // Name / description are user-owned too — update them along
                    // with the criteria. `update_smart_criteria` bumps
                    // `library_changed_tx`, so the grid + open detail refresh.
                    if let Err(e) =
                        library::playlists::update_playlist(&s, target_id, name, description, None)
                            .await
                    {
                        log::warn!("update smart playlist meta {target_id}: {e}");
                    }
                    if let Err(e) =
                        library::smart_playlists::update_smart_criteria(&s, target_id, &criteria)
                            .await
                    {
                        log::warn!("update smart criteria {target_id}: {e}");
                    }
                }
            });
        });
    }
}

/// Run `f` against the installed `VecModel<SmartRuleRow>` if present.
fn with_rules_model<R>(ui: &AppWindow, f: impl FnOnce(&VecModel<SmartRuleRow>) -> R) -> Option<R> {
    ui.global::<SmartEditor>()
        .get_rules()
        .as_any()
        .downcast_ref::<VecModel<SmartRuleRow>>()
        .map(f)
}

/// Patch the rule row at editor index `row` through `f`, if it exists. The
/// keyed-by-index analogue of [`crate::ui::model_patch::patch_track_row_by_id`].
fn patch_rule_row(ui: &AppWindow, row: i32, f: impl FnOnce(SmartRuleRow) -> SmartRuleRow) {
    with_rules_model(ui, |vm| {
        let Ok(ri) = usize::try_from(row) else { return };
        if let Some(old) = vm.row_data(ri) {
            vm.set_row_data(ri, f(old));
        }
    });
}

/// Rebuild a rule row after a field or operator change: recompute the
/// `field-kind` / `input-kind` codes from `(field, op)` and keep the old value
/// text only when the input kind is unchanged (else the remounted input reads
/// empty). Shared by the `set-rule-field` / `set-rule-op` handlers.
fn rebuilt_row(
    field: sc::RuleField,
    field_index: i32,
    op: sc::RuleOp,
    op_index: i32,
    old: &SmartRuleRow,
) -> SmartRuleRow {
    let vt = field.value_type();
    let input_kind = op.input_kind(vt).as_index();
    SmartRuleRow {
        field_index,
        op_index,
        field_kind: vt.as_index(),
        input_kind,
        value_text: keep_or_clear(input_kind, old.input_kind, &old.value_text),
    }
}

/// The first (default) operator for a value type.
fn first_op(vt: sc::ValueType) -> sc::RuleOp {
    *sc::ops_for(vt).first().unwrap_or(&sc::RuleOp::Contains)
}

/// The default blank rule row: `Title contains …`.
fn default_rule_row() -> SmartRuleRow {
    let vt = sc::RuleField::Title.value_type();
    SmartRuleRow {
        field_index: 0,
        op_index: 0,
        field_kind: vt.as_index(),
        input_kind: first_op(vt).input_kind(vt).as_index(),
        value_text: SharedString::default(),
    }
}

/// Field at a dropdown index (falling back to `Title` on an out-of-range index).
fn field_at(index: i32) -> sc::RuleField {
    usize::try_from(index)
        .ok()
        .and_then(|i| sc::FIELDS.get(i).copied())
        .unwrap_or(sc::RuleField::Title)
}

/// Operator at a dropdown index within `value_type`'s operator array.
fn op_at(value_type: sc::ValueType, index: i32) -> sc::RuleOp {
    usize::try_from(index)
        .ok()
        .and_then(|i| sc::ops_for(value_type).get(i).copied())
        .unwrap_or_else(|| first_op(value_type))
}

/// Keep the old value text when the input kind is unchanged; clear it otherwise
/// (the remounted input then reads an empty value).
fn keep_or_clear(new_kind: i32, old_kind: i32, old_text: &SharedString) -> SharedString {
    if new_kind == old_kind {
        old_text.clone()
    } else {
        SharedString::default()
    }
}

/// Build an editor row from a stored rule.
fn rule_to_row(rule: &sc::Rule) -> Option<SmartRuleRow> {
    let field_index = i32::try_from(sc::FIELDS.iter().position(|f| *f == rule.field)?).ok()?;
    let vt = rule.field.value_type();
    let op_index = i32::try_from(sc::ops_for(vt).iter().position(|o| *o == rule.op)?).ok()?;
    let value_text = match &rule.value {
        Some(sc::RuleValue::Text(s)) => SharedString::from(s.as_str()),
        Some(sc::RuleValue::Number(n)) => SharedString::from(n.to_string()),
        Some(sc::RuleValue::Days(d)) => SharedString::from(d.to_string()),
        None => SharedString::default(),
    };
    Some(SmartRuleRow {
        field_index,
        op_index,
        field_kind: vt.as_index(),
        input_kind: rule.op.input_kind(vt).as_index(),
        value_text,
    })
}

/// Rebuild a rule from an editor row (dropping incoherent / incomplete rows).
fn row_to_rule(row: &SmartRuleRow) -> Option<sc::Rule> {
    let field = usize::try_from(row.field_index)
        .ok()
        .and_then(|i| sc::FIELDS.get(i).copied())?;
    let vt = field.value_type();
    let op = usize::try_from(row.op_index)
        .ok()
        .and_then(|i| sc::ops_for(vt).get(i).copied())?;
    let value = sc::RuleValue::from_input(vt, op, row.value_text.as_str());
    Some(sc::Rule { field, op, value })
}

/// A committable smart-playlist draft read out of the editor. `target_id < 0`
/// means "create"; otherwise it's an update of that playlist.
struct CriteriaDraft {
    name: String,
    description: Option<String>,
    criteria: sc::SmartCriteria,
    target_id: i64,
}

/// Read the `SmartEditor` globals + the rules model into a [`CriteriaDraft`].
/// Returns `None` when the name is blank (nothing to commit).
fn collect_criteria(ui: &AppWindow) -> Option<CriteriaDraft> {
    let se = ui.global::<SmartEditor>();
    let name = se.get_name().trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let description = se.get_description().trim().to_owned();
    let description = (!description.is_empty()).then_some(description);
    let limit = se.get_limit_enabled().then(|| sc::SmartLimit {
        count: se
            .get_limit_count_text()
            .trim()
            .parse::<u32>()
            .unwrap_or_else(|_| sc::SmartLimit::default().count)
            .clamp(1, 100_000),
        order: sc::LimitOrder::from_index(se.get_limit_order_index()),
    });
    let rules: Vec<sc::Rule> =
        with_rules_model(ui, |vm| vm.iter().filter_map(|r| row_to_rule(&r)).collect())
            .unwrap_or_default();
    Some(CriteriaDraft {
        name,
        description,
        criteria: sc::SmartCriteria {
            version: sc::SMART_CRITERIA_VERSION,
            match_mode: sc::MatchMode::from_index(se.get_match_mode_index()),
            rules,
            limit,
        },
        target_id: i64::from(se.get_target_id()),
    })
}

/// Populate every `SmartEditor` global + the rules model from a criteria and
/// target id (`-1` = create). Shared by `request-new` and `request-edit`; the
/// caller opens the dialog afterwards.
fn populate_editor(
    ui: &AppWindow,
    name: &str,
    description: &str,
    criteria: &sc::SmartCriteria,
    target_id: i64,
) {
    let se = ui.global::<SmartEditor>();
    se.set_name(SharedString::from(name));
    se.set_description(SharedString::from(description));
    se.set_match_mode_index(criteria.match_mode.as_index());
    if let Some(l) = &criteria.limit {
        se.set_limit_enabled(true);
        se.set_limit_count_text(SharedString::from(l.count.to_string()));
        se.set_limit_order_index(l.order.as_index());
    } else {
        se.set_limit_enabled(false);
        se.set_limit_count_text(SharedString::from(sc::SmartLimit::default().count.to_string()));
        se.set_limit_order_index(0);
    }
    se.set_target_id(clamp_i64_to_i32(target_id));
    with_rules_model(ui, |vm| {
        let mut rows: Vec<SmartRuleRow> =
            criteria.rules.iter().filter_map(rule_to_row).collect();
        if rows.is_empty() {
            rows.push(default_rule_row());
        }
        vm.set_vec(rows);
    });
}
