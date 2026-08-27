//! What the typed needle could mean besides a station name.
//!
//! The directory has no free-text search: `name`, `tag`, `country`, `language`, `codec` and
//! `bitrateMin` are separate parameters that AND together, so a box meaning "all of them" would
//! have to fan a settled keystroke out into four concurrent requests and merge them. This is the
//! cheaper half of that trade — the box keeps meaning `name`, and anything else the needle could
//! be is *offered* as a scope the user takes in one click.
//!
//! **Matching is local and costs no traffic.** [`super::facets::prime`] holds the four lists
//! resident, and everything below is a walk over them.
//!
//! Two shapes have no list to be matched against, and are read off the needle instead. A
//! **bitrate** is a bare integer in the kbps band. A **frequency** is the harder case and the
//! reason [`Suggestion::count`] is optional: the resident tag list is capped to the most-used
//! entries, and a dial position is a tag only one station's own listeners use, so it sits far
//! below that cut — the pill offers the raw needle as a tag scope, with no count because nothing
//! has asked the directory for one.

use std::sync::Arc;

use slint::{ComponentHandle, Model, SharedString};

use crate::entities::radio::{Facet, StationSearch};
use crate::state::AppState;
use crate::ui::grid_rows::write_grid;
use crate::ui::row_match::{self, Needle};
use crate::{AppWindow, Radio, RadioSuggestionRow};

use super::facets::{self, ChipFilter, FacetIndex};
use super::{RadioTab, RadioUi, browse, tab_from_index};

/// How many pills the row shows. It sits above the grid on one line and does not wrap, so this is
/// what the line holds rather than a relevance judgement.
const MAX_SUGGESTIONS: usize = 4;

/// Below this a needle names too much to be a scope — a single letter matches a third of the
/// country list, and the row would fill with noise while the user is still typing.
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
    /// Stations carrying it, or `None` where the needle was synthesized rather than matched and
    /// there is no count to state.
    pub count: Option<i64>,
}

/// Everything the needle could mean besides a name, best first.
///
/// `active` is the query already on screen, and it is here to suppress a scope that query is
/// already filtered by — an offer to do what has been done reads as a filter that failed.
pub(super) fn suggestions(
    needle: &Needle,
    lists: &FacetIndex,
    active: &StationSearch,
) -> Vec<Suggestion> {
    // Characters, not bytes: a two-character CJK needle is six bytes and is not the noise this
    // floor is for.
    if needle.as_str().chars().count() < MIN_NEEDLE_LEN {
        return Vec::new();
    }

    let mut found: Vec<(bool, Suggestion)> = Vec::new();
    for (chip, facets) in lists.lists() {
        if let Some(best) = best_match(needle, chip, facets, active) {
            found.push(best);
        }
    }

    // Exact before partial, then by how many stations carry it. A partial match on a huge facet
    // is still less likely to be what was meant than an exact one on a small facet.
    found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.count.cmp(&a.1.count)));

    // **The shape leads, and it is the cap that makes that matter.** A facet match is a guess at
    // what the needle names; a shape is a fact about the needle itself, and for a frequency it is
    // the only pill that reaches those stations at all.
    let mut out: Vec<Suggestion> = typed_shape(needle, active).into_iter().collect();
    out.extend(found.into_iter().map(|(_, s)| s));
    out.truncate(MAX_SUGGESTIONS);
    out
}

/// The one entry of a facet list worth offering, and whether the needle named it exactly.
///
/// One per chip rather than the whole match set: five near-identical country pills say nothing the
/// chip's own picker does not say better, and the row has four slots for four *kinds* of answer.
fn best_match(
    needle: &Needle,
    chip: ChipFilter,
    facets: &[Facet],
    active: &StationSearch,
) -> Option<(bool, Suggestion)> {
    // The tail of a `hidebroken` list carries facets every station of which was filtered out, and
    // a scope with nothing behind it is a click that guarantees an empty grid.
    facets
        .iter()
        .filter(|facet| !facet.name.is_empty() && facet.station_count > 0)
        .filter(|facet| needle.contains(&facet.name))
        .filter(|facet| !already_filtered_by(chip, facet, active))
        .map(|facet| (needle.equals(&facet.name), facet))
        .max_by(|a, b| a.0.cmp(&b.0).then(a.1.station_count.cmp(&b.1.station_count)))
        .map(|(exact, facet)| {
            (
                exact,
                Suggestion {
                    chip,
                    name: facet.name.clone(),
                    code: facet.code.clone().unwrap_or_default(),
                    count: Some(facet.station_count),
                },
            )
        })
}

/// Whether the query on screen already carries this exact scope.
///
/// Compared against the field [`super::facets::apply_pick`] writes for that chip, so the two
/// cannot disagree about what "already applied" means.
fn already_filtered_by(chip: ChipFilter, facet: &Facet, active: &StationSearch) -> bool {
    match chip {
        // Held as the code, `countrycode` being the endpoint's only code-keyed parameter, which is
        // also why this takes the whole `Facet`. The emptiness guard is load-bearing: an
        // unfiltered query and a facet carrying no code would otherwise read as a match.
        ChipFilter::Country => {
            !active.country_code.is_empty()
                && facet.code.as_deref().is_some_and(|code| code == active.country_code)
        }
        ChipFilter::Language => facet.name == active.language,
        ChipFilter::Tag => active.tags.contains(&facet.name),
        ChipFilter::Codec => facet.name == active.codec,
        // No list feeds it, so it is never reached from here.
        ChipFilter::BitrateMin => false,
    }
}

/// The scope a needle's own shape asks for, where no facet list can answer.
///
/// At most one: the two shapes are mutually exclusive by construction, a frequency carrying the
/// separator a bitrate must not have.
fn typed_shape(needle: &Needle, active: &StationSearch) -> Option<Suggestion> {
    let text = needle.as_str();
    if let Some(freq) = frequency(text) {
        // Offered even where a tag is already picked: the picked one is a genre and this is a
        // number, so replacing it is exactly what the click is for.
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

/// A broadcast frequency as a station's tags spell one: two or three digits, a decimal separator,
/// one or two more. `92.1`, `101,5`.
///
/// Handed back rather than merely recognised because the comma spelling has to reach the tag as a
/// point — the directory's own tags are written `92.1 fm` whatever the locale that typed them.
fn frequency(text: &str) -> Option<String> {
    let (whole, fraction) = text.split_once(['.', ','])?;
    let digits = |part: &str, range: std::ops::RangeInclusive<usize>| {
        range.contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit())
    };
    (digits(whole, 2..=3) && digits(fraction, 1..=2)).then(|| format!("{whole}.{fraction}"))
}

/// A bare integer inside the kbps band, which is the only number that can mean a bitrate. Anything
/// carrying a separator was a frequency and never reaches here.
fn bitrate(text: &str) -> Option<u32> {
    let kbps: u32 = text.parse().ok()?;
    (MIN_BITRATE_KBPS..=MAX_BITRATE_KBPS).contains(&kbps).then_some(kbps)
}

/// Recompute the pill row from whatever Browse is currently asking for.
///
/// A walk of four resident lists, so it is cheap enough to run from anything that can move what
/// the box means: a settled keystroke, a facet list landing, the mounted tab or detail changing
/// under it. Reading the query rather than taking a needle is what lets every one of those call it
/// without knowing which.
pub(super) fn refresh(ui: &AppWindow, radio_ui: &Arc<RadioUi>) {
    let g = ui.global::<Radio>();
    let rows = if offers_scopes(&g) {
        let search = browse::query(radio_ui);
        let needle = row_match::fold_needle(&search.name);
        suggestions(&needle, &radio_ui.facet_index.lock(), &search)
            .into_iter()
            .map(|s| to_row(&g, &s))
            .collect()
    } else {
        Vec::new()
    };
    write_grid(&g.get_suggestions(), rows, "radio::suggest");
}

/// Whether the page is somewhere a scope pill would mean anything: Browse, with no station page
/// over it. The other two tabs filter rows already in hand, where the needle is not a query and
/// there is no second field to offer.
fn offers_scopes(g: &Radio<'_>) -> bool {
    !g.get_detail_open() && tab_from_index(g, g.get_tab_idx()) == RadioTab::Browse
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

/// Take a suggestion: fill its chip and empty the box, in one query edit.
///
/// **One edit, not two.** The needle and the chip filter the same request, so applying the scope
/// while leaving the name behind asks for stations that are both named *turkish* and in Turkish —
/// reliably nothing. Two separate [`browse::edit_query`] calls would each fetch, and the first
/// would fetch exactly that empty page.
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

/// Take the only scope on offer, where the name search found nothing.
///
/// The 1:1 rule, and **both halves are the test**: one scope offered, and an empty page for it to
/// displace. A needle that found stations is a name search that worked, and choosing between two
/// scopes is the user's — so the pills stay pills in every other case.
///
/// Terminates without a guard: adopting empties the needle, so the query this triggers offers no
/// scopes of its own however few stations come back.
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

/// Whether a scope is one an empty page may take on the user's behalf.
///
/// **A bitrate floor is not**, being the one scope that is a guess about digits rather than
/// something a list confirmed: `128` is as likely to be part of a station's name, and answering a
/// search for one with every station at 128 kbps and up is further from the intent than the empty
/// page. Its sibling shape stays in — a frequency reaches those stations through nothing else,
/// which is the case the rule was written for.
fn adoptable(chip: ChipFilter) -> bool {
    chip != ChipFilter::BitrateMin
}

#[cfg(test)]
#[path = "tests/suggest_tests.rs"]
mod tests;
