//! The filter chips: the lists behind them, and what a pick does to the query.
//!
//! Lists are fetched on the first open of a chip rather than on entering the section: four lists
//! nobody may ever filter by are four requests, and `services::radio_browser` caches each for the
//! session, so every reopen after the first costs nothing.
//!
//! One model serves every chip, because only one picker can be up at a time. `Radio.facet-shown`
//! records which chip it holds, and is what a landing list is checked against: a user who opens
//! Country and moves to Language before the first answer arrives must not get a country list under
//! the language label.

use std::sync::Arc;

use slint::{ComponentHandle, Model};

use crate::entities::radio::{Facet, FacetKind, StationSearch};
use crate::library;
use crate::state::AppState;
use crate::ui::grid_rows::write_grid;
use crate::ui::row_match;
use crate::{AppWindow, Radio};

use super::{RadioUi, browse, rows};

/// Which filter chip an index names.
///
/// The indices live in `globals/radio.slint`'s `facet-*` constants and no Rust file restates them.
/// **Minimum bitrate is a chip but not a directory facet**: its options are a fixed list the chip
/// carries itself, so it has a [`ChipFilter`] and no [`FacetKind`], which is the whole reason the
/// two are separate types.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ChipFilter {
    Country,
    Language,
    Tag,
    Codec,
    BitrateMin,
}

impl ChipFilter {
    /// The directory list behind the chip, or `None` where the chip supplies its own.
    fn facet_kind(self) -> Option<FacetKind> {
        match self {
            Self::Country => Some(FacetKind::Countries),
            Self::Language => Some(FacetKind::Languages),
            Self::Tag => Some(FacetKind::Tags),
            Self::Codec => Some(FacetKind::Codecs),
            Self::BitrateMin => None,
        }
    }
}

/// Resolve a chip index against the global's own constants. UI thread only, that being where the
/// global is reachable.
///
/// `None` for anything unrecognised, so a sixth chip added to the global without an arm here does
/// nothing rather than filtering by the wrong field.
fn chip_from_index(g: &Radio<'_>, idx: i32) -> Option<ChipFilter> {
    if idx == g.get_facet_country() {
        Some(ChipFilter::Country)
    } else if idx == g.get_facet_language() {
        Some(ChipFilter::Language)
    } else if idx == g.get_facet_tag() {
        Some(ChipFilter::Tag)
    } else if idx == g.get_facet_codec() {
        Some(ChipFilter::Codec)
    } else if idx == g.get_facet_bitrate() {
        Some(ChipFilter::BitrateMin)
    } else {
        None
    }
}

/// Fill the shared picker model for the chip at `idx`.
///
/// A repeat open of the chip already in the model is a no-op, which is what makes the session
/// cache visible: the popup comes back up on the list it had.
pub fn request(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, idx: i32) {
    let g = ui.global::<Radio>();
    let Some(kind) = chip_from_index(&g, idx).and_then(ChipFilter::facet_kind) else {
        return;
    };
    if g.get_facet_shown() == idx && g.get_facet_options().row_count() > 0 {
        return;
    }

    // Emptied on the way out rather than left holding the previous chip's list, which would paint
    // under the new label for as long as the fetch took.
    write_grid(&g.get_facet_options(), Vec::new(), "radio::facets");
    radio_ui.facet_list.lock().take();
    g.set_facet_shown(idx);
    g.set_facet_loading(true);

    let (s, ru, weak) = (state.clone(), radio_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        let fetched = library::radio::facets(&s, kind).await;
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let g = ui.global::<Radio>();
            if g.get_facet_shown() != idx {
                return;
            }
            g.set_facet_loading(false);
            match fetched {
                Ok(facets) => {
                    // Kept whole beside the model, because the picker's own needle narrows it and
                    // Slint cannot filter an array. Re-asking the facade would be free (the list
                    // is a session `OnceCell`) but it is `async`, and a keystroke has no business
                    // hopping the runtime to answer.
                    *ru.facet_list.lock() = Some(Arc::clone(&facets));
                    write_filtered(&g, &facets, "");
                }
                Err(e) => {
                    log::warn!("radio: facet list failed: {}", crate::services::describe(&e));
                    // The popup falls back to its empty copy, and nothing is memoized, so the
                    // next open asks again.
                    g.set_facet_shown(-1);
                }
            }
        });
    });
}

/// Narrow the open picker's list to `needle`.
///
/// Through `row_match`'s fold like every other filter box in the tree, so a country typed without
/// its accents still matches.
pub fn filter(ui: &AppWindow, radio_ui: &Arc<RadioUi>, needle: &str) {
    let Some(facets) = radio_ui.facet_list.lock().clone() else {
        return;
    };
    write_filtered(&ui.global::<Radio>(), &facets, needle);
}

fn write_filtered(g: &Radio<'_>, facets: &[Facet], needle: &str) {
    let folded = row_match::fold_needle(needle);
    let facet_rows: Vec<_> = facets
        .iter()
        .filter(|facet| folded.is_empty() || folded.contains(&facet.name))
        .map(rows::to_slint_facet_row)
        .collect();
    write_grid(&g.get_facet_options(), facet_rows, "radio::facets");
}

/// Set or clear one chip's filter, and re-query if that moved anything.
pub fn pick(
    ui: &AppWindow,
    state: &AppState,
    radio_ui: &Arc<RadioUi>,
    idx: i32,
    name: &str,
    code: &str,
) {
    let g = ui.global::<Radio>();
    let Some(chip) = chip_from_index(&g, idx) else {
        return;
    };

    match chip {
        ChipFilter::Country => {
            g.set_pick_country(name.into());
            g.set_pick_country_code(code.into());
        }
        ChipFilter::Language => g.set_pick_language(name.into()),
        ChipFilter::Tag => g.set_pick_tag(name.into()),
        ChipFilter::Codec => g.set_pick_codec(name.into()),
        ChipFilter::BitrateMin => {
            g.set_pick_bitrate_min(i32::try_from(bitrate_floor(code)).unwrap_or(0));
        }
    }

    browse::edit_query(ui, state, radio_ui, |search| apply_pick(chip, name, code, search));
}

/// Fold one chip's pick into the query.
///
/// An empty `name` is the clear, which is also why clearing needs no separate path: "no country"
/// and "this country" are the same edit with different values.
///
/// **`code` only ever reaches the request for a country.** `countrycode` is the search endpoint's
/// sole code-keyed parameter, so a language filters by its name even though the directory hands
/// one an `iso_639` beside it — sent as a code, `language=en` would substring-match english,
/// armenian and slovenian alike.
fn apply_pick(chip: ChipFilter, name: &str, code: &str, search: &mut StationSearch) {
    match chip {
        ChipFilter::Country => code.clone_into(&mut search.country_code),
        ChipFilter::Language => name.clone_into(&mut search.language),
        ChipFilter::Tag => {
            // The parameter takes a list and means "all of these", where the chip offers one; an
            // empty vector is no tag filter rather than a filter for the empty tag.
            search.tags = if name.is_empty() {
                Vec::new()
            } else {
                vec![name.to_owned()]
            };
        }
        ChipFilter::Codec => name.clone_into(&mut search.codec),
        ChipFilter::BitrateMin => search.bitrate_min = bitrate_floor(code),
    }
}

/// The bitrate chip's own options carry the floor in `code`; anything unparseable is no floor.
/// The chip offers no "any" row — clearing it goes through the pill's own `close`.
fn bitrate_floor(code: &str) -> u32 {
    code.parse().unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/facets_tests.rs"]
mod tests;
