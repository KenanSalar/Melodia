//! What the typed needle could mean besides a station name.
//!
//! The directory's parameters AND together and none is free text, so rather than fan a keystroke
//! out into four requests the box keeps meaning `name` and anything else the needle could be is
//! *offered* as a scope. Matching is a local walk over the lists [`super::facets::prime`] holds.
//!
//! Two shapes are read off the needle instead: a **bitrate**, a bare integer in the kbps band, and
//! a **frequency**, a tag too rare for the capped tag list, offered raw with no count, which is
//! why [`Suggestion::count`] is optional.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use crate::ui::model_diff::replace_rows;
use melodia_app::state::AppState;
use melodia_core::entities::radio::{Facet, StationSearch};
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::{AppWindow, Radio, RadioSuggestionRow};

use super::facets::{self, ChipFilter, FacetIndex};
use super::{RadioTab, RadioUi, browse, mounted_tab};

/// Below this a needle names too much to be a scope, and the row fills with noise mid-typing.
const MIN_NEEDLE_LEN: usize = 2;

/// The advertised-bitrate band worth offering a floor in. Below the low end a number is a name
/// (`Radio 24`), and above the high end it is past anything the directory carries.
const MIN_BITRATE_KBPS: u32 = 32;
const MAX_BITRATE_KBPS: u32 = 640;

/// One scope the needle could be asking for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Suggestion {
    pub chip: ChipFilter,
    /// What the pill shows and what the chip is set to.
    pub name: String,
    /// The chip's second value: a country's ISO code, a bitrate's floor, empty for the rest.
    pub code: String,
    /// Stations carrying it, or `None` for a synthesized scope with no count to state.
    pub count: Option<i64>,
}

/// Everything the needle could mean besides a name, best first: a shape and one match per list at
/// most. A scope `active` already applies is left out, an offer of what's done reading as failed.
pub(super) fn suggestions(
    needle: &Needle,
    lists: &FacetIndex,
    active: &StationSearch,
) -> Vec<Suggestion> {
    // Characters, not bytes, so a two-character CJK needle counts as two.
    if needle.as_str().chars().count() < MIN_NEEDLE_LEN {
        return Vec::new();
    }

    let mut found: Vec<(bool, Suggestion)> = Vec::new();
    for (chip, facets) in lists.lists() {
        if let Some(best) = best_match(needle, chip, facets, active) {
            found.push(best);
        }
    }

    // Exact before partial, then by station count.
    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.count.cmp(&a.1.count)));

    // The shape leads: a fact about the needle rather than a guess, and for a frequency the only
    // way to those stations.
    let mut out: Vec<Suggestion> = typed_shape(needle, active).into_iter().collect();
    out.extend(found.into_iter().map(|(_, s)| s));
    out
}

/// The one entry of a facet list worth offering, and whether the needle named it exactly; one per
/// chip, the chip's own picker showing the rest better.
fn best_match(
    needle: &Needle,
    chip: ChipFilter,
    facets: &[Facet],
    active: &StationSearch,
) -> Option<(bool, Suggestion)> {
    // A `hidebroken` list's tail holds facets with no stations left behind them.
    facets
        .iter()
        .filter(|facet| !facet.name.is_empty() && facet.station_count > 0)
        .filter(|facet| facets::matches_needle(Some(chip), facet, needle))
        .filter(|facet| !already_filtered_by(chip, facet, active))
        .map(|facet| (facets::equals_needle(Some(chip), facet, needle), facet))
        .max_by(|a, b| a.0.cmp(&b.0).then(a.1.station_count.cmp(&b.1.station_count)))
        .map(|(exact, facet)| {
            // Through a picker row's pair, so a pill and the chip send the same value.
            let (label, code) = facets::drawn_as(Some(chip), facet);
            (
                exact,
                Suggestion {
                    chip,
                    name: label.into_owned(),
                    code: code.to_owned(),
                    count: Some(facet.station_count),
                },
            )
        })
}

/// Whether the query already carries this scope, read off the field
/// [`super::facets::apply_pick`] writes for the chip.
fn already_filtered_by(chip: ChipFilter, facet: &Facet, active: &StationSearch) -> bool {
    match chip {
        // Held as the code. Without the emptiness guard an unfiltered query and a codeless facet
        // would match.
        ChipFilter::Country => {
            !active.country_code.is_empty()
                && facet.code.as_deref().is_some_and(|code| code == active.country_code)
        }
        ChipFilter::Language => facet.name == active.language,
        ChipFilter::Tag => active.tags.contains(&facet.name),
        // The name, not the label, `drawn_as` being what puts the raw value in the pill's `code`.
        ChipFilter::Codec => facet.name == active.codec,
        // No list feeds it, so it is never reached from here.
        ChipFilter::BitrateMin => false,
    }
}

/// The scope a needle's own shape asks for, where no list can answer: at most one, a frequency
/// carrying the separator a bitrate can't.
fn typed_shape(needle: &Needle, active: &StationSearch) -> Option<Suggestion> {
    let text = needle.as_str();
    if let Some(freq) = frequency(text) {
        // Offered over a picked tag too, which is a genre; replacing it is the click's point.
        return Some(Suggestion {
            chip: ChipFilter::Tag,
            name: freq,
            code: String::new(),
            count: None,
        });
    }
    let floor = bitrate(text).filter(|kbps| *kbps != active.bitrate_min)?;
    Some(Suggestion {
        chip: ChipFilter::BitrateMin,
        name: floor.to_string(),
        code: floor.to_string(),
        count: None,
    })
}

/// A broadcast frequency as a station's tags spell one (`92.1`, `101,5`), handed back with a
/// point, the directory writing `92.1 fm` whatever the locale.
fn frequency(text: &str) -> Option<String> {
    let (whole, fraction) = text.split_once(['.', ','])?;
    let digits = |part: &str, range: std::ops::RangeInclusive<usize>| {
        range.contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit())
    };
    (digits(whole, 2..=3) && digits(fraction, 1..=2)).then(|| format!("{whole}.{fraction}"))
}

/// A bare integer inside the kbps band, the only number that can mean a bitrate.
fn bitrate(text: &str) -> Option<u32> {
    let kbps: u32 = text.parse().ok()?;
    (MIN_BITRATE_KBPS..=MAX_BITRATE_KBPS).contains(&kbps).then_some(kbps)
}

/// Recomputes the pill row from what Browse is asking for. Cheap, and reading the query rather
/// than taking a needle, so anything that moves what the box means can call it.
pub(super) fn refresh(ui: &AppWindow, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let rows = if offers_scopes(&g) {
        let search = browse::query(radio_ui);
        let needle = fold_needle(&search.name);
        suggestions(&needle, &radio_ui.facet_index.lock(), &search)
            .into_iter()
            .map(|s| to_row(&g, &s))
            .collect()
    } else {
        Vec::new()
    };
    replace_rows(&g.get_suggestions(), rows, "radio::suggest");
}

/// Whether a scope pill means anything here: Browse with no station page over it, the other tabs
/// filtering rows already in hand.
fn offers_scopes(g: &Radio<'_>) -> bool {
    !g.get_detail_open() && mounted_tab(g) == RadioTab::Browse
}

fn to_row(g: &Radio<'_>, suggestion: &Suggestion) -> RadioSuggestionRow {
    RadioSuggestionRow {
        kind: facets::chip_index(g, suggestion.chip),
        name: suggestion.name.as_str().into(),
        code: suggestion.code.as_str().into(),
        station_count: suggestion
            .count
            .map_or(-1, |count| i32::try_from(count).unwrap_or(i32::MAX)),
    }
}

/// Takes a suggestion: fills its chip and empties the box in **one** [`browse::edit_query`], the
/// needle and the chip filtering the same request, so two edits would fetch an empty page first.
pub(super) fn apply(
    ui: &AppWindow,
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    idx: i32,
    name: &str,
    code: &str,
) {
    let g = ui.global::<Radio>();
    let Some(chip) = facets::chip_from_index(&g, idx) else {
        return;
    };

    facets::show_pick(&g, chip, name, code);
    g.set_filter(SharedString::default());
    browse::edit_query(ui, state, radio_ui, |search| {
        facets::apply_pick(chip, name, code, search);
        search.name.clear();
    });
    // After the edit, so the row is rebuilt against the query that is actually on its way.
    refresh(ui, radio_ui);
}

/// Takes the only scope on offer where the name search found nothing; both halves are the test, a
/// choice between two scopes being the user's. Adopting empties the needle, so the query it
/// triggers offers nothing and there is no loop.
pub(super) fn adopt_only_scope(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let offered = g.get_suggestions();
    if g.get_browse_count() > 0 || offered.row_count() != 1 {
        return;
    }
    let Some(only) = offered.row_data(0) else {
        return;
    };
    if !facets::chip_from_index(&g, only.kind).is_some_and(adoptable) {
        return;
    }
    apply(ui, state, radio_ui, only.kind, &only.name, &only.code);
}

/// Whether an empty page may take a scope on the user's behalf. Not a bitrate floor, a guess about
/// digits a name could hold (`128`); a frequency reaches its stations through nothing else.
fn adoptable(chip: ChipFilter) -> bool {
    chip != ChipFilter::BitrateMin
}

#[cfg(test)]
#[path = "tests/suggest_tests.rs"]
mod tests;
