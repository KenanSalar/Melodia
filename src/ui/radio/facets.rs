//! The filter chips: the lists behind them, and what a pick does to the query.
//!
//! **All four lists are fetched once, on the first Radio enter, whichever tab it lands on.** They
//! used to wait for a chip to be opened, on the argument that four lists nobody may ever filter by
//! are four requests. [`super::suggest`] is what overtook that: a list arriving on the chip's own
//! open arrives after the needle it was wanted for. They are still four requests a session that
//! never leaves Favorites will not read — `services::radio_browser` holds each in a `OnceCell`, so
//! that is one round of them per run, and every chip now opens on its list rather than a spinner.
//!
//! One model serves every chip, because only one picker can be up at a time. `Radio.facet-shown`
//! records which chip it holds, and is what a landing list is checked against: a user who opens
//! Country and moves to Language before the first answer arrives must not get a country list under
//! the language label.

use std::borrow::Cow;
use std::sync::Arc;

use slint::ComponentHandle;

use crate::entities::radio::{Facet, FacetKind, StationSearch, UNKNOWN_CODEC};
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

/// The chip indices, read off the global's own constants. **The one place the mapping is
/// spelled**, and both directions come out of it rather than out of a lookup and a hand-written
/// inverse that can disagree about a single arm.
///
/// UI thread only, that being where the global is reachable.
fn chip_indices(g: &Radio<'_>) -> [(ChipFilter, i32); 5] {
    [
        (ChipFilter::Country, g.get_facet_country()),
        (ChipFilter::Language, g.get_facet_language()),
        (ChipFilter::Tag, g.get_facet_tag()),
        (ChipFilter::Codec, g.get_facet_codec()),
        (ChipFilter::BitrateMin, g.get_facet_bitrate()),
    ]
}

/// Resolve a chip index against the global's own constants.
///
/// `None` for anything unrecognised, so a sixth chip added to the global without an entry above
/// does nothing rather than filtering by the wrong field.
pub(super) fn chip_from_index(g: &Radio<'_>, idx: i32) -> Option<ChipFilter> {
    chip_indices(g).into_iter().find(|(_, at)| *at == idx).map(|(chip, _)| chip)
}

/// [`chip_from_index`]'s inverse, for a suggestion naming the chip it offers to fill. `-1` where
/// the chip has no index, which no live [`ChipFilter`] can be.
pub(super) fn chip_index(g: &Radio<'_>, chip: ChipFilter) -> i32 {
    chip_indices(g).into_iter().find(|(c, _)| *c == chip).map_or(-1, |(_, at)| at)
}

/// The four directory lists held together, so a keystroke can match against them without going
/// near the runtime.
///
/// Beside [`RadioUi::facet_list`](super::RadioUi) rather than replacing it: that one is whichever
/// list the *open picker* is narrowing, and answers a question about one chip. This is all four at
/// once and answers a question about the needle.
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

    /// Each chip paired with the list behind it, empty where that list has not landed. A list
    /// still in flight suggests nothing rather than blocking, which is what makes [`prime`] safe
    /// to leave asynchronous.
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

/// Fetch every facet list once, so the suggestion pass has something to match against.
///
/// Fired on the section enter and idempotent twice over: a list already held is skipped here,
/// and the facade's own `OnceCell` answers a repeat without a request. A failure is logged and
/// left unfilled — that facet simply suggests nothing, and the chip's own open retries it.
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
                        crate::services::describe(&e)
                    );
                    continue;
                }
            }
            // Per list rather than after all four: the needle the user has already typed gets
            // the answers as they land instead of waiting on the slowest.
            let _ = weak.upgrade_in_event_loop({
                let ru = ru.clone();
                move |ui| super::suggest::refresh(&ui, &ru)
            });
        }
    });
}

/// Fill the shared picker model for the chip at `idx`.
///
/// **A picker opens on the whole list, and the needle is emptied here rather than at the chip.**
/// The box and the model it narrows are one reset; split across the two trees only one half runs,
/// leaving a filter no box on screen still names.
///
/// A chip whose list is already in hand skips the fetch, which is what makes the session cache
/// visible: the popup comes straight back up, widened.
pub fn request(ui: &AppWindow, state: &AppState, radio_ui: &Arc<RadioUi>, idx: i32) {
    let g = ui.global::<Radio>();
    let Some(kind) = chip_from_index(&g, idx).and_then(ChipFilter::facet_kind) else {
        return;
    };
    let narrowed = !g.get_facet_filter().is_empty();
    g.set_facet_filter("".into());

    // Keyed on the list held, not on the rows drawn: a needle that matched nothing leaves an empty
    // model over a list already in hand, and asking again would clear the picker and flash a
    // spinner to re-derive it. A failed fetch parks `facet-shown` at `-1`, so that one still asks.
    if g.get_facet_shown() == idx && radio_ui.facet_list.lock().is_some() {
        // An empty box was already drawing the whole list, so there is nothing to widen.
        if narrowed {
            filter(ui, radio_ui, "");
        }
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
                    // Not the empty needle: `filter` bails while the list is in flight, so a box
                    // typed into under the spinner is only honoured here.
                    write_filtered(&g, &facets, &g.get_facet_filter());
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

/// How many options the picker draws when the user has typed nothing.
///
/// A ceiling on the *drawing*, never on what is reachable: the lists arrive ordered
/// by station count, so an untouched picker shows the popular head, and the needle
/// below searches every entry the fetch brought back. Nobody scrolls to the end of
/// the tag list, and building a row per entry to let them try costs a model rebuild
/// on every open and every clear.
///
/// Only the tag list is long enough to reach this; the curated three sit well under
/// it and are drawn whole.
pub(super) const UNFILTERED_ROW_CAP: usize = 500;

fn write_filtered(g: &Radio<'_>, facets: &[Facet], needle: &str) {
    // Off the global rather than threaded in: the picker only ever draws the chip it is showing,
    // and both callers have already made `facet-shown` that chip.
    let chip = chip_from_index(g, g.get_facet_shown());
    let folded = row_match::fold_needle(needle);
    let matches = facets.iter().filter(|facet| matches_needle(chip, facet, &folded));
    let row = |facet: &Facet| rows::to_slint_facet_row(chip, facet);

    // `Needle::contains` answers true for an empty needle, so the filter above is
    // already the identity there and the cap is the whole difference. Gated on the
    // *folded* needle, since a box holding only spaces narrows nothing either.
    let facet_rows: Vec<_> = if folded.is_empty() {
        matches.take(UNFILTERED_ROW_CAP).map(row).collect()
    } else {
        matches.map(row).collect()
    };
    write_grid(&g.get_facet_options(), facet_rows, "radio::facets");
}

/// The value the directory sent, as the pair a row carries: what to draw, and what to send back.
///
/// **They differ only for codecs.** The value the search parameter takes rides in `code` — the
/// slot a country already uses for exactly this reason — so nothing below the row boundary ever
/// sees the label. [`apply_pick`] is the other half.
pub(super) fn drawn_as(chip: Option<ChipFilter>, facet: &Facet) -> (Cow<'_, str>, &str) {
    if chip != Some(ChipFilter::Codec) {
        return (Cow::Borrowed(&facet.name), facet.code.as_deref().unwrap_or_default());
    }
    (codec_label(&facet.name), &facet.name)
}

/// The label a chip rewrote, or `None` where the drawn spelling *is* the value the directory sent.
///
/// Both filter passes test it beside `facet.name`, and neither can drop it: matching the raw value
/// alone hid the row that says `HLS` from anyone who typed it, and surfaced it for `UNKNOWN`, which
/// is a word on screen nowhere.
fn rewritten_label(chip: Option<ChipFilter>, facet: &Facet) -> Option<Cow<'_, str>> {
    match drawn_as(chip, facet).0 {
        Cow::Borrowed(_) => None,
        owned @ Cow::Owned(_) => Some(owned),
    }
}

/// Whether `needle` matches this facet, on either spelling.
pub(super) fn matches_needle(
    chip: Option<ChipFilter>,
    facet: &Facet,
    needle: &row_match::Needle,
) -> bool {
    needle.contains(&facet.name)
        || rewritten_label(chip, facet).is_some_and(|label| needle.contains(&label))
}

/// Whether `needle` *is* this facet, on either spelling. The suggestion ranking's exactness term.
pub(super) fn equals_needle(
    chip: Option<ChipFilter>,
    facet: &Facet,
    needle: &row_match::Needle,
) -> bool {
    needle.equals(&facet.name)
        || rewritten_label(chip, facet).is_some_and(|label| needle.equals(&label))
}

/// What the [`UNKNOWN_CODEC`] bucket actually holds, and the name the chip offers it under.
///
/// The checker probes a byte stream and a playlist is not one, so it fails on precisely the
/// segmented stations: measured across the bucket, under two per cent of it is anything else, and
/// most of that remainder answers with nothing playable. Naming a *filter* for what it holds beats
/// naming it for what the directory could not work out, which reads beside `MP3` and `AAC+` as a
/// value that failed to load. Per station the generalisation is unnecessary and is not made —
/// [`super::rows::display_codec`] has the row's own `hls` flag to go on.
pub(super) const SEGMENTED_CODEC_LABEL: &str = "HLS";

/// Borrowed for every real format, so only the handful of entries that need rewriting allocate.
fn codec_label(codec: &str) -> Cow<'_, str> {
    if !codec.contains(',') && !codec.eq_ignore_ascii_case(UNKNOWN_CODEC) {
        return Cow::Borrowed(codec);
    }
    // A comma means the checker found a picture track beside the audio, and the directory writes
    // the pair with no space.
    let spelled: Vec<&str> = codec
        .split(',')
        .map(str::trim)
        .map(|part| {
            if part.eq_ignore_ascii_case(UNKNOWN_CODEC) {
                SEGMENTED_CODEC_LABEL
            } else {
                part
            }
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

/// Put a pick on the chip that carries it. Split from [`pick`] because a suggestion sets the same
/// chip through a *different* query edit — it moves the needle out of the name as it goes, which
/// has to be one edit or the page fetches twice.
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

/// Fold one chip's pick into the query.
///
/// An empty `name` is the clear, which is also why clearing needs no separate path: "no country"
/// and "this country" are the same edit with different values.
///
/// **`code` only ever reaches the request for a country.** `countrycode` is the search endpoint's
/// sole code-keyed parameter, so a language filters by its name even though the directory hands
/// one an `iso_639` beside it — sent as a code, `language=en` would substring-match english,
/// armenian and slovenian alike.
pub(super) fn apply_pick(chip: ChipFilter, name: &str, code: &str, search: &mut StationSearch) {
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
        // The label is what a row hands back, so the value the parameter takes rides in `code` —
        // see [`drawn_as`]. Empty on both is the clear, which either branch spells the same way.
        ChipFilter::Codec => {
            let wire = if code.is_empty() { name } else { code };
            wire.clone_into(&mut search.codec);
        }
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
