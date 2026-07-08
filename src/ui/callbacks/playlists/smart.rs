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
/// [`super::wire_playlists`].
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

    // set-rule-field — reset the operator to this field-kind's first, recompute
    // the codes, clear the value only if the input kind changed.
    {
        let weak = weak.clone();
        se.on_set_rule_field(move |row, field_idx| {
            let Some(ui) = weak.upgrade() else { return };
            with_rules_model(&ui, |vm| {
                let Ok(ri) = usize::try_from(row) else { return };
                let Some(old) = vm.row_data(ri) else { return };
                let field = field_at(field_idx);
                let vt = field.value_type();
                let op = *sc::ops_for(vt).first().unwrap_or(&sc::RuleOp::Contains);
                let input_kind = op.input_kind(vt).as_index();
                let value_text = keep_or_clear(input_kind, old.input_kind, &old.value_text);
                vm.set_row_data(
                    ri,
                    SmartRuleRow {
                        field_index: field_idx,
                        op_index: 0,
                        field_kind: vt.as_index(),
                        input_kind,
                        value_text,
                    },
                );
            });
        });
    }

    // set-rule-op — recompute the input kind for the new operator.
    {
        let weak = weak.clone();
        se.on_set_rule_op(move |row, op_idx| {
            let Some(ui) = weak.upgrade() else { return };
            with_rules_model(&ui, |vm| {
                let Ok(ri) = usize::try_from(row) else { return };
                let Some(old) = vm.row_data(ri) else { return };
                let field = field_at(old.field_index);
                let vt = field.value_type();
                let op = op_at(vt, op_idx);
                let input_kind = op.input_kind(vt).as_index();
                let value_text = keep_or_clear(input_kind, old.input_kind, &old.value_text);
                vm.set_row_data(
                    ri,
                    SmartRuleRow {
                        op_index: op_idx,
                        input_kind,
                        value_text,
                        ..old
                    },
                );
            });
        });
    }

    // set-rule-value — mirror the input into the model (commit reads it back).
    {
        let weak = weak.clone();
        se.on_set_rule_value(move |row, text| {
            let Some(ui) = weak.upgrade() else { return };
            with_rules_model(&ui, |vm| {
                let Ok(ri) = usize::try_from(row) else { return };
                let Some(old) = vm.row_data(ri) else { return };
                vm.set_row_data(
                    ri,
                    SmartRuleRow {
                        value_text: text,
                        ..old
                    },
                );
            });
        });
    }

    // request-new — reset to a fresh editor and open on a fresh tick.
    {
        let weak = weak.clone();
        se.on_request_new(move || {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak.upgrade() else { return };
                let se = ui.global::<SmartEditor>();
                se.set_name(SharedString::default());
                se.set_description(SharedString::default());
                se.set_match_mode_index(0);
                se.set_limit_enabled(false);
                se.set_limit_count_text(SharedString::from("25"));
                se.set_limit_order_index(0);
                se.set_target_id(-1);
                with_rules_model(&ui, |vm| vm.set_vec(vec![default_rule_row()]));
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
                let name = SharedString::from(detail.name.as_str());
                let description =
                    SharedString::from(detail.description.as_deref().unwrap_or(""));
                let criteria =
                    sc::SmartCriteria::from_json_opt(detail.smart_criteria.as_deref());
                let _ = weak.upgrade_in_event_loop(move |ui| {
                    let se = ui.global::<SmartEditor>();
                    se.set_name(name);
                    se.set_description(description);
                    se.set_match_mode_index(match_mode_index(criteria.match_mode));
                    if let Some(l) = &criteria.limit {
                        se.set_limit_enabled(true);
                        se.set_limit_count_text(SharedString::from(l.count.to_string()));
                        se.set_limit_order_index(limit_order_index(l.order));
                    } else {
                        se.set_limit_enabled(false);
                        se.set_limit_count_text(SharedString::from("25"));
                        se.set_limit_order_index(0);
                    }
                    se.set_target_id(clamp_i64_to_i32(id));
                    with_rules_model(&ui, |vm| {
                        let mut rows: Vec<SmartRuleRow> =
                            criteria.rules.iter().filter_map(rule_to_row).collect();
                        if rows.is_empty() {
                            rows.push(default_rule_row());
                        }
                        vm.set_vec(rows);
                    });
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
            let se = ui.global::<SmartEditor>();
            let name = se.get_name().trim().to_owned();
            if name.is_empty() {
                return;
            }
            let description = se.get_description().trim().to_owned();
            let description_opt = if description.is_empty() {
                None
            } else {
                Some(description)
            };
            let match_mode = if se.get_match_mode_index() == 1 {
                sc::MatchMode::Any
            } else {
                sc::MatchMode::All
            };
            let limit = if se.get_limit_enabled() {
                let count = se
                    .get_limit_count_text()
                    .trim()
                    .parse::<u32>()
                    .unwrap_or(25)
                    .clamp(1, 100_000);
                Some(sc::SmartLimit {
                    count,
                    order: limit_order_from_index(se.get_limit_order_index()),
                })
            } else {
                None
            };
            let rules: Vec<sc::Rule> =
                with_rules_model(&ui, |vm| vm.iter().filter_map(|r| row_to_rule(&r)).collect())
                    .unwrap_or_default();
            let target_id = i64::from(se.get_target_id());
            let criteria = sc::SmartCriteria {
                version: sc::SMART_CRITERIA_VERSION,
                match_mode,
                rules,
                limit,
            };

            let s = state.clone();
            let pu = playlists_ui.clone();
            let weak = weak.clone();
            s.runtime.clone().spawn(async move {
                if target_id < 0 {
                    match library::smart_playlists::create_smart_playlist(
                        &s,
                        name.clone(),
                        description_opt,
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
                    if let Err(e) = library::playlists::update_playlist(
                        &s,
                        target_id,
                        name,
                        description_opt,
                        None,
                    )
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

/// The default blank rule row: `Title contains …`.
fn default_rule_row() -> SmartRuleRow {
    let vt = sc::RuleField::Title.value_type();
    let op = *sc::ops_for(vt).first().unwrap_or(&sc::RuleOp::Contains);
    SmartRuleRow {
        field_index: 0,
        op_index: 0,
        field_kind: vt.as_index(),
        input_kind: op.input_kind(vt).as_index(),
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
    let ops = sc::ops_for(value_type);
    usize::try_from(index)
        .ok()
        .and_then(|i| ops.get(i).copied())
        .unwrap_or_else(|| *ops.first().unwrap_or(&sc::RuleOp::Contains))
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

fn match_mode_index(mode: sc::MatchMode) -> i32 {
    match mode {
        sc::MatchMode::All => 0,
        sc::MatchMode::Any => 1,
    }
}

/// Mirrors the `limit-orders` dropdown array in `smart-playlist-editor-body.slint`.
fn limit_order_index(order: sc::LimitOrder) -> i32 {
    match order {
        sc::LimitOrder::DateAddedDesc => 0,
        sc::LimitOrder::DateAddedAsc => 1,
        sc::LimitOrder::PlayCountDesc => 2,
        sc::LimitOrder::PlayCountAsc => 3,
        sc::LimitOrder::LastPlayedDesc => 4,
        sc::LimitOrder::LastPlayedAsc => 5,
        sc::LimitOrder::RatingDesc => 6,
        sc::LimitOrder::Random => 7,
    }
}

fn limit_order_from_index(index: i32) -> sc::LimitOrder {
    match index {
        1 => sc::LimitOrder::DateAddedAsc,
        2 => sc::LimitOrder::PlayCountDesc,
        3 => sc::LimitOrder::PlayCountAsc,
        4 => sc::LimitOrder::LastPlayedDesc,
        5 => sc::LimitOrder::LastPlayedAsc,
        6 => sc::LimitOrder::RatingDesc,
        7 => sc::LimitOrder::Random,
        _ => sc::LimitOrder::DateAddedDesc,
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
