//! The filter chips: the lists behind them, and what a pick does to the query.
//!
//! **All four lists are fetched on the first Radio enter, whichever tab it lands on**, so a
//! [`super::suggest`] pass has them before the needle it is wanted for. The client holds each in a
//! `OnceCell`, so that is one round per run.
//!
//! One model serves every chip, only one picker being up at a time. `Radio.facet-shown` records
//! which chip it holds, and a landing list is checked against it so it can't fill another chip.

use std::borrow::Cow;
use std::sync::Arc;

use slint::{ComponentHandle, SharedString};

use crate::ui::model_diff::replace_rows;
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::radio::{Facet, FacetKind, StationSearch, UNKNOWN_CODEC};
use melodia_core::utils::needle::{Needle, fold_needle};
use melodia_ui::{AppWindow, Radio};

use super::{RadioUi, browse, rows};

/// Which filter chip an index names; the indices are `globals/radio.slint`'s `facet-*` constants.
/// Minimum bitrate is a chip but not a directory facet, its options being the chip's own, which is
/// why this and [`FacetKind`] are separate types.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ChipFilter {
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

/// The chip indices, read off the global's own constants: the one place the mapping is spelled,
/// both directions coming out of it.
fn chip_indices(g: &Radio<'_>) -> [(ChipFilter, i32); 5] {
    [
        (ChipFilter::Country, g.get_facet_country()),
        (ChipFilter::Language, g.get_facet_language()),
        (ChipFilter::Tag, g.get_facet_tag()),
        (ChipFilter::Codec, g.get_facet_codec()),
        (ChipFilter::BitrateMin, g.get_facet_bitrate()),
    ]
}

/// Resolves a chip index; `None` for anything unrecognised, so a sixth chip without an entry above
/// filters nothing rather than the wrong field.
pub(super) fn chip_from_index(g: &Radio<'_>, idx: i32) -> Option<ChipFilter> {
    chip_indices(g).into_iter().find(|(_, at)| *at == idx).map(|(chip, _)| chip)
}

/// [`chip_from_index`]'s inverse, for a suggestion naming the chip it offers to fill. `-1` where
/// the chip has no index, which no live [`ChipFilter`] can be.
pub(super) fn chip_index(g: &Radio<'_>, chip: ChipFilter) -> i32 {
    chip_indices(g).into_iter().find(|(c, _)| *c == chip).map_or(-1, |(_, at)| at)
}

/// The four directory lists held together, so a keystroke matches against them off the runtime.
/// Beside [`RadioUi::facet_list`](super::RadioUi), which is the one list the open picker narrows.
#[derive(Default)]
pub(super) struct FacetIndex {
    countries: Option<Arc<[Facet]>>,
    languages: Option<Arc<[Facet]>>,
    tags: Option<Arc<[Facet]>>,
    codecs: Option<Arc<[Facet]>>,
}

impl FacetIndex {
    fn slot_mut(&mut self, kind: FacetKind) -> &mut Option<Arc<[Facet]>> {
        match kind {
            FacetKind::Countries => &mut self.countries,
            FacetKind::Languages => &mut self.languages,
            FacetKind::Tags => &mut self.tags,
            FacetKind::Codecs => &mut self.codecs,
        }
    }

    /// A primed index, for tests that need the four lists without a directory behind them.
    #[cfg(test)]
    pub(super) fn from_lists(
        countries: Vec<Facet>,
        languages: Vec<Facet>,
        tags: Vec<Facet>,
        codecs: Vec<Facet>,
    ) -> Self {
        Self {
            countries: Some(countries.into()),
            languages: Some(languages.into()),
            tags: Some(tags.into()),
            codecs: Some(codecs.into()),
        }
    }

    /// Each chip and its list, empty where it hasn't landed, so a list in flight suggests nothing
    /// and [`prime`] can stay asynchronous.
    pub(super) fn lists(&self) -> [(ChipFilter, &[Facet]); 4] {
        fn entries(list: Option<&Arc<[Facet]>>) -> &[Facet] {
            list.map(Arc::as_ref).unwrap_or_default()
        }
        [
            (ChipFilter::Country, entries(self.countries.as_ref())),
            (ChipFilter::Language, entries(self.languages.as_ref())),
            (ChipFilter::Tag, entries(self.tags.as_ref())),
            (ChipFilter::Codec, entries(self.codecs.as_ref())),
        ]
    }
}

/// Fetches every facet list once, on the section enter, for the suggestion pass. A list already
/// held is skipped and the facade answers a repeat itself; a failure leaves that facet suggesting
/// nothing until the chip's own open retries it.
pub fn prime(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>) {
    let wanted: Vec<FacetKind> = {
        let index = radio_ui.facet_index.lock();
        index
            .lists()
            .into_iter()
            .filter(|(_, entries)| entries.is_empty())
            .filter_map(|(chip, _)| chip.facet_kind())
            .collect()
    };
    if wanted.is_empty() {
        return;
    }

    let (s, ru, weak) = (state.clone(), radio_ui.clone(), ui.as_weak());
    state.runtime.spawn(async move {
        for kind in wanted {
            match library::radio::facets(&s, kind).await {
                Ok(facets) => *ru.facet_index.lock().slot_mut(kind) = Some(facets),
                Err(e) => {
                    log::warn!(
                        "radio: priming the {kind:?} facet list failed: {}",
                        melodia_core::error::describe(&e)
                    );
                    continue;
                }
            }
            // Per list, so the typed needle gets answers as they land.
            let _ = weak.upgrade_in_event_loop({
                let ru = ru.clone();
                move |ui| super::suggest::refresh(&ui, &ru)
            });
        }
    });
}

/// Fills the shared picker model for the chip at `idx`, on the whole list: the needle is emptied
/// here rather than at the chip, the box and the model being one reset across two trees. A list
/// already in hand skips the fetch.
pub fn request(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, idx: i32) {
    let g = ui.global::<Radio>();
    let Some(kind) = chip_from_index(&g, idx).and_then(ChipFilter::facet_kind) else {
        return;
    };
    let narrowed = !g.get_facet_filter().is_empty();
    g.set_facet_filter(SharedString::default());

    // Keyed on the list held, not the rows drawn, which a needle matching nothing leaves empty.
    // A failed fetch parks `facet-shown` at `-1`, so that one asks again.
    if g.get_facet_shown() == idx && radio_ui.facet_list.lock().is_some() {
        // An empty box was already drawing the whole list, so there is nothing to widen.
        if narrowed {
            filter(ui, radio_ui, "");
        }
        return;
    }

    // Emptied, or the previous chip's list paints under the new label until the fetch lands.
    replace_rows(&g.get_facet_options(), Vec::new(), "radio::facets");
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
                    // Kept whole beside the model: Slint can't filter an array, and a keystroke
                    // shouldn't hop the runtime to re-ask the facade.
                    *ru.facet_list.lock() = Some(Arc::clone(&facets));
                    // Not the empty needle: `filter` bails while the list is in flight, so a box
                    // typed into under the spinner is only honoured here.
                    write_filtered(&g, &facets, &g.get_facet_filter());
                }
                Err(e) => {
                    log::warn!("radio: facet list failed: {}", melodia_core::error::describe(&e));
                    // Nothing is memoized, so the next open asks again.
                    g.set_facet_shown(-1);
                }
            }
        });
    });
}

/// Narrows the open picker's list to `needle`.
pub fn filter(ui: &AppWindow, radio_ui: &Arc<RadioUi>, needle: &str) {
    let Some(facets) = radio_ui.facet_list.lock().clone() else {
        return;
    };
    write_filtered(&ui.global::<Radio>(), &facets, needle);
}

/// How many options the picker draws, typed into or not: a cap on the drawing, never on what the
/// needle searches. The lists arrive ordered by station count, so it shows the popular head. The
/// needle is unthrottled, so uncapped it would build a row per match on every keystroke over the
/// tag list, the only list long enough to reach this.
pub(super) const PICKER_ROW_CAP: usize = 500;

fn write_filtered(g: &Radio<'_>, facets: &[Facet], needle: &str) {
    // Off the global: both callers have already made `facet-shown` this chip.
    let chip = chip_from_index(g, g.get_facet_shown());
    let folded = fold_needle(needle);
    // An empty needle matches everything, so the cap alone decides what an untouched picker draws.
    let facet_rows: Vec<_> = facets
        .iter()
        .filter(|facet| matches_needle(chip, facet, &folded))
        .take(PICKER_ROW_CAP)
        .map(|facet| rows::to_slint_facet_row(chip, facet))
        .collect();
    replace_rows(&g.get_facet_options(), facet_rows, "radio::facets");
}

/// The pair a row carries for a facet: what to draw, and what to send back. They differ only for
/// codecs, whose value rides in `code` as a country's does; [`apply_pick`] is the other half.
pub(super) fn drawn_as(chip: Option<ChipFilter>, facet: &Facet) -> (Cow<'_, str>, &str) {
    if chip != Some(ChipFilter::Codec) {
        return (Cow::Borrowed(&facet.name), facet.code.as_deref().unwrap_or_default());
    }
    (codec_label(&facet.name), &facet.name)
}

/// The label a chip rewrote, or `None` where the drawn spelling is the value sent. Both filter
/// passes test it beside `facet.name`, so a row drawn as `HLS` matches what it shows.
fn rewritten_label(chip: Option<ChipFilter>, facet: &Facet) -> Option<Cow<'_, str>> {
    match drawn_as(chip, facet).0 {
        Cow::Borrowed(_) => None,
        owned @ Cow::Owned(_) => Some(owned),
    }
}

/// Whether `needle` matches this facet, on either spelling.
pub(super) fn matches_needle(chip: Option<ChipFilter>, facet: &Facet, needle: &Needle) -> bool {
    needle.contains(&facet.name)
        || rewritten_label(chip, facet).is_some_and(|label| needle.contains(&label))
}

/// Whether `needle` *is* this facet, on either spelling. The suggestion ranking's exactness term.
pub(super) fn equals_needle(chip: Option<ChipFilter>, facet: &Facet, needle: &Needle) -> bool {
    needle.equals(&facet.name)
        || rewritten_label(chip, facet).is_some_and(|label| needle.equals(&label))
}

/// The name the chip offers the [`UNKNOWN_CODEC`] bucket under: the checker probes a byte stream,
/// so it fails on segmented stations, which are nearly all of that bucket. Per station,
/// `rows::display_codec` goes by the row's own `hls` flag instead.
pub(super) const SEGMENTED_CODEC_LABEL: &str = "HLS";

/// Borrowed for every real format, so only the handful of entries that need rewriting allocate.
fn codec_label(codec: &str) -> Cow<'_, str> {
    if !codec.contains(',') && !codec.eq_ignore_ascii_case(UNKNOWN_CODEC) {
        return Cow::Borrowed(codec);
    }
    // A comma is a picture track beside the audio, which the directory writes with no space.
    let spelled: Vec<&str> =
        codec
            .split(',')
            .map(str::trim)
            .map(|part| {
                if part.eq_ignore_ascii_case(UNKNOWN_CODEC) { SEGMENTED_CODEC_LABEL } else { part }
            })
            .collect();
    Cow::Owned(spelled.join(", "))
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

    show_pick(&g, chip, name, code);
    browse::edit_query(ui, state, radio_ui, |search| apply_pick(chip, name, code, search));
}

/// Puts a pick on its chip. Split from [`pick`] because a suggestion moves the needle out of the
/// name in the same query edit, which has to stay one edit or the page fetches twice.
pub(super) fn show_pick(g: &Radio<'_>, chip: ChipFilter, name: &str, code: &str) {
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
}

/// Folds one chip's pick into the query; an empty `name` is the clear.
///
/// `code` reaches the request only for a country, `countrycode` being the one code-keyed
/// parameter: sent as a code, `language=en` would substring-match english, armenian and slovenian.
pub(super) fn apply_pick(chip: ChipFilter, name: &str, code: &str, search: &mut StationSearch) {
    match chip {
        ChipFilter::Country => code.clone_into(&mut search.country_code),
        ChipFilter::Language => name.clone_into(&mut search.language),
        ChipFilter::Tag => {
            // An empty vector is no tag filter, not a filter for the empty tag.
            search.tags = if name.is_empty() { Vec::new() } else { vec![name.to_owned()] };
        }
        // The value rides in `code` (`drawn_as`); empty on both is the clear.
        ChipFilter::Codec => {
            let wire = if code.is_empty() { name } else { code };
            wire.clone_into(&mut search.codec);
        }
        ChipFilter::BitrateMin => search.bitrate_min = bitrate_floor(code),
    }
}

/// The bitrate chip's own options carry the floor in `code`; anything unparseable is no floor.
fn bitrate_floor(code: &str) -> u32 {
    code.parse().unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/facets_tests.rs"]
mod tests;
