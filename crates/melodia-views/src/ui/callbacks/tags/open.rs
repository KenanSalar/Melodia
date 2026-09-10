//! Opening the dialog: fetch what the selection says, fold it down to what it agrees on, and
//! record the baseline the commit diffs against.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use async_compat::Compat;
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, SharedString, VecModel,
};

use crate::ui::util::{format_channels, format_sample_rate, len_as_i32};
use melodia_app::library;
use melodia_app::state::AppState;
use melodia_core::entities::album::ReleaseTagRow;
use melodia_core::entities::artist::{ArtistCredit, JOIN_PHRASES};
use melodia_core::entities::credits::RoleCredits;
use melodia_core::entities::genre::GenreList;
use melodia_core::entities::tags::ArtworkEdit;
use melodia_core::entities::track::TagEditRow;
use melodia_core::error::{AppError, describe};
use melodia_ui::{AppWindow, Dialog, Settings, TagEditor};

use super::artwork::decode_cover_preview;
use super::credits::{rows_from_credit, write_credit_rows};
use super::fold::{
    bpm_key, common_by, common_str, common_value, fmt_bpm, fmt_int, fmt_size, int_key,
};
use super::form::{CREDIT_ALBUM_ARTIST, CREDIT_ARTIST, FormState, ListFields};
use super::lists::{common_roles, write_genre_rows, write_role_rows};
use super::session::TagSession;

/// `request-edit`: fetch rows (+ lyrics + cover for a single selection),
/// populate, and open the dialog on the next UI tick.
pub(super) fn wire_request_edit(
    te: &TagEditor,
    ui: &AppWindow,
    state: &AppState,
    session: &Rc<RefCell<TagSession>>,
) {
    let weak = ui.as_weak();
    let state = state.clone();
    let session = session.clone();
    te.on_request_edit(move |ids_model, tab| {
        let ids: Vec<i64> = ids_model.iter().map(i64::from).collect();
        if ids.is_empty() {
            return;
        }
        let weak = weak.clone();
        let s = state.clone();
        let session = session.clone();
        let _ = slint::spawn_local(Compat::new(async move {
            let rows = match library::tags::get_tag_edit_rows(&s, &ids).await {
                Ok(rows) if !rows.is_empty() => rows,
                Ok(_) => return,
                Err(e) => {
                    log::warn!("tag edit fetch: {e}");
                    return;
                }
            };
            let single = rows.len() == 1;

            // Lyrics live in the file, not the DB, and so does the authoritative credit — a
            // library whose join rows were seeded from `artist_id` alone knows only the first
            // name. One read for both; a multi-selection takes the database instead, N file reads
            // on the open path being what the row projection exists to avoid.
            let (lyrics, credits) = if single {
                let path = PathBuf::from(&rows[0].file_path);
                let read =
                    s.runtime.spawn_blocking(move || library::tags::read_lyrics_and_credits(&path));
                match read.await {
                    Ok(Ok((lyrics, credits))) => (lyrics.unwrap_or_default(), vec![credits]),
                    Ok(Err(e)) => {
                        log::warn!("tag edit: reading {}: {}", rows[0].file_path, describe(&e));
                        (String::new(), vec![<(ArtistCredit, ArtistCredit)>::default()])
                    }
                    Err(e) => {
                        log::warn!("tag edit: reading {} failed: {e}", rows[0].file_path);
                        (String::new(), vec![<(ArtistCredit, ArtistCredit)>::default()])
                    }
                }
            } else {
                let by_id =
                    or_logged(library::tags::get_tag_edit_credits(&s, &ids).await, "credits");
                (String::new(), by_row(&rows, &by_id))
            };

            // What the Now Playing panel would show for this track, which is the tag only when
            // nothing better exists. Offered rather than applied: writing it is a tag edit like
            // any other, so it goes through this dialog's own Save.
            let resident = if single {
                library::lyrics::resident_text(&s, &rows[0].file_path)
                    .await
                    .unwrap_or_else(|e| {
                        log::debug!("tag edit: no resident sheet: {}", describe(&e));
                        None
                    })
                    .filter(|text| text.trim() != lyrics.trim())
            } else {
                None
            };

            // Cover preview from the first row that has one (decode off the
            // UI thread — a full-res source could jank it).
            let cover_path = rows.iter().find_map(|r| {
                r.artwork_path.as_deref().filter(|p| !p.is_empty()).map(PathBuf::from)
            });
            let cover = match cover_path {
                Some(p) => {
                    s.runtime.spawn_blocking(move || decode_cover_preview(&p)).await.ok().flatten()
                }
                None => None,
            };

            // Always from the database, single selection included — argued at
            // `library::tags::get_tag_edit_role_credits`.
            let roles_by_id =
                or_logged(library::tags::get_tag_edit_role_credits(&s, &ids).await, "role credits");
            let roles: Vec<RoleCredits> = by_row(&rows, &roles_by_id);

            let genres_by_id =
                or_logged(library::tags::get_tag_edit_genres(&s, &ids).await, "genres");
            let genre_lists: Vec<GenreList> = by_row(&rows, &genres_by_id);

            // Release-level tags live on `albums`, so the Details tab reads them through the album
            // each selected track sits on.
            let release =
                or_logged(library::tags::get_tag_edit_release_tags(&s, &ids).await, "release tags");

            let Some(ui) = weak.upgrade() else { return };
            populate(
                &ui,
                &session,
                Fetched {
                    rows,
                    credits,
                    roles,
                    genre_lists,
                    release,
                    lyrics,
                    resident,
                    cover,
                },
            );
            // After `populate`, which resets to Tags: the request's tab is the last word. Only a
            // single selection can honour it, Lyrics and Summary being unmounted in batch mode, so
            // a request for one over many rows would open on a tab that draws nothing. The bounds
            // are the global's own, so nothing here restates a position the body owns.
            if single {
                let te = ui.global::<TagEditor>();
                te.set_active_tab(tab.clamp(te.get_tab_tags(), te.get_tab_summary()));
            }
            ui.global::<Dialog>().set_open(true);
        }));
    });
}

/// What an opening fetch answered, or its default with a line in the log.
///
/// A read error costs the dialog one field and nothing else: a form missing its genres is better
/// than no form at all, and the field stays at `Keep` because its baseline is what was shown.
fn or_logged<T: Default>(result: Result<T, AppError>, what: &str) -> T {
    result.unwrap_or_else(|e| {
        log::warn!("tag edit {what}: {}", describe(&e));
        T::default()
    })
}

/// Line a by-id fetch up with `rows`, defaulting whatever it didn't answer for.
fn by_row<T: Clone + Default>(rows: &[TagEditRow], by_id: &HashMap<i64, T>) -> Vec<T> {
    rows.iter().map(|r| by_id.get(&r.id).cloned().unwrap_or_default()).collect()
}

/// The ‹multiple values› sentinel where the selection disagrees, the normal hint otherwise.
fn placeholder(disagrees: bool, sentinel: &SharedString) -> SharedString {
    if disagrees {
        sentinel.clone()
    } else {
        SharedString::default()
    }
}

/// Everything one open of the dialog fetched, before any of it reaches a property.
///
/// One value rather than seven parameters: the four slices are all per-selected-track and
/// **parallel to `rows`**, which is an invariant a reader can check here and could not check across
/// an argument list. The three singles come off the same fetch and travel with them.
struct Fetched {
    rows: Vec<TagEditRow>,
    credits: Vec<(ArtistCredit, ArtistCredit)>,
    roles: Vec<RoleCredits>,
    genre_lists: Vec<GenreList>,
    release: Vec<ReleaseTagRow>,
    lyrics: String,
    resident: Option<String>,
    cover: Option<SharedPixelBuffer<Rgb8Pixel>>,
}

/// Fill the `TagEditor` global from the fetched rows and record the snapshot.
fn populate(ui: &AppWindow, session: &Rc<RefCell<TagSession>>, fetched: Fetched) {
    let Fetched {
        rows,
        credits,
        roles,
        genre_lists,
        release,
        lyrics,
        resident,
        cover,
    } = fetched;
    let (rows, credits, roles, release) =
        (rows.as_slice(), credits.as_slice(), roles.as_slice(), release.as_slice());

    let te = ui.global::<TagEditor>();
    let sentinel = ui.global::<Settings>().invoke_tag_multiple_values();

    let mut originals = FormState::default();

    // Set one field: value + placeholder (the ‹multiple values› sentinel iff the
    // selection disagrees) + record the populated string as the diff baseline.
    //
    // The leading field name is what pairs this with `read_text_fields` — the commit reads the
    // same name back, so the two can't drift the way an index into a shared array could.
    macro_rules! field {
        ($name:ident, $val:expr, $set:ident, $set_ph:ident) => {{
            let (value, disagrees) = $val;
            te.$set(SharedString::from(value.as_str()));
            te.$set_ph(placeholder(disagrees, &sentinel));
            originals.text.$name = value;
        }};
    }

    /// A `TagEditRow` string column, folded to the value the whole selection agrees on.
    macro_rules! shared {
        ($field:ident) => {
            common_str(rows.iter().map(|r| r.$field.as_deref().unwrap_or_default()))
        };
    }

    field!(
        title,
        common_str(rows.iter().map(|r| r.title.as_str())),
        set_title,
        set_title_placeholder
    );

    // The two credits. `common_value` decides the ‹multiple values› sentinel the way `common_str`
    // does, and the hint rides on the first row's own input rather than a field-wide placeholder.
    let mut phrases: Vec<(String, String)> = JOIN_PHRASES
        .iter()
        .map(|(label, rendered)| ((*label).to_owned(), (*rendered).to_owned()))
        .collect();
    let (artist, artist_disagrees) = common_value(credits.iter().map(|(a, _)| a));
    let (album_artist, album_artist_disagrees) = common_value(credits.iter().map(|(_, a)| a));
    let artist_rows = rows_from_credit(&artist, &mut phrases);
    let album_artist_rows = rows_from_credit(&album_artist, &mut phrases);

    te.set_join_phrases(ModelRc::new(VecModel::from(
        phrases.iter().map(|(label, _)| SharedString::from(label.as_str())).collect::<Vec<_>>(),
    )));
    write_credit_rows(&te, CREDIT_ARTIST, artist_rows);
    write_credit_rows(&te, CREDIT_ALBUM_ARTIST, album_artist_rows);
    te.set_artist_placeholder(placeholder(artist_disagrees, &sentinel));
    te.set_album_artist_placeholder(placeholder(album_artist_disagrees, &sentinel));
    te.set_artist_preview(SharedString::from(artist.line().unwrap_or_default()));
    te.set_album_artist_preview(SharedString::from(album_artist.line().unwrap_or_default()));
    field!(album, shared!(album), set_album, set_album_placeholder);
    // The genre list, one row per name. The rows themselves are the baseline, so nothing is
    // recorded in `originals.text`.
    let (genres, genres_disagree) = common_value(genre_lists.iter());
    write_genre_rows(&te, &genres);
    te.set_genre_placeholder(placeholder(genres_disagree, &sentinel));
    field!(
        year,
        common_by(rows.iter().map(|r| r.year), int_key, fmt_int),
        set_year,
        set_year_placeholder
    );
    field!(
        original_year,
        common_by(rows.iter().map(|r| r.original_year), int_key, fmt_int),
        set_original_year,
        set_original_year_placeholder
    );
    field!(
        track_number,
        common_by(rows.iter().map(|r| r.track_number), int_key, fmt_int),
        set_track_number,
        set_track_number_placeholder
    );
    field!(
        track_total,
        common_by(rows.iter().map(|r| r.track_total), int_key, fmt_int),
        set_track_total,
        set_track_total_placeholder
    );
    field!(
        disc_number,
        common_by(rows.iter().map(|r| r.disc_number), int_key, fmt_int),
        set_disc_number,
        set_disc_number_placeholder
    );
    field!(
        disc_total,
        common_by(rows.iter().map(|r| r.disc_total), int_key, fmt_int),
        set_disc_total,
        set_disc_total_placeholder
    );
    field!(comment, shared!(comment), set_comment, set_comment_placeholder);
    field!(
        bpm,
        common_by(rows.iter().map(|r| r.bpm), bpm_key, fmt_bpm),
        set_bpm,
        set_bpm_placeholder
    );

    // The role credits, a list of rows per role. Structural like the genres and the two artist
    // credits, so the sentinel is per role: one selection can agree on the composer and disagree
    // on the producer.
    //
    // The sentinel rides the edit's own scope rather than a mask beside it — a role the form
    // cannot answer for is exactly the one that shows the hint, so one answer serves both.
    let role_edit = common_roles(roles);
    write_role_rows(ui, role_edit.credits());
    te.set_role_placeholders(ModelRc::new(VecModel::from(
        role_edit
            .answered()
            .iter()
            .map(|answered| placeholder(!answered, &sentinel))
            .collect::<Vec<_>>(),
    )));

    field!(subtitle, shared!(subtitle), set_subtitle, set_subtitle_placeholder);
    field!(disc_subtitle, shared!(disc_subtitle), set_disc_subtitle, set_disc_subtitle_placeholder);
    field!(grouping, shared!(grouping), set_grouping, set_grouping_placeholder);
    field!(work, shared!(work), set_work, set_work_placeholder);
    field!(movement, shared!(movement), set_movement, set_movement_placeholder);
    field!(
        movement_number,
        common_by(rows.iter().map(|r| r.movement_number), int_key, fmt_int),
        set_movement_number,
        set_movement_number_placeholder
    );
    field!(
        movement_total,
        common_by(rows.iter().map(|r| r.movement_total), int_key, fmt_int),
        set_movement_total,
        set_movement_total_placeholder
    );
    field!(initial_key, shared!(initial_key), set_initial_key, set_initial_key_placeholder);
    field!(mood, shared!(mood), set_mood, set_mood_placeholder);
    field!(language, shared!(language), set_language, set_language_placeholder);
    field!(isrc, shared!(isrc), set_isrc, set_isrc_placeholder);
    field!(copyright, shared!(copyright), set_copyright, set_copyright_placeholder);

    // Release-level, read off the album the selection sits on rather than off `tracks`.
    field!(
        label,
        common_str(release.iter().map(|r| r.label.as_str())),
        set_label,
        set_label_placeholder
    );
    field!(
        catalog_number,
        common_str(release.iter().map(|r| r.catalog_number.as_str())),
        set_catalog_number,
        set_catalog_number_placeholder
    );
    field!(
        barcode,
        common_str(release.iter().map(|r| r.barcode.as_str())),
        set_barcode,
        set_barcode_placeholder
    );
    field!(
        media,
        common_str(release.iter().map(|r| r.media.as_str())),
        set_media,
        set_media_placeholder
    );
    field!(
        release_type,
        common_str(release.iter().map(|r| r.release_type.as_str())),
        set_release_type,
        set_release_type_placeholder
    );
    field!(
        release_country,
        common_str(release.iter().map(|r| r.release_country.as_str())),
        set_release_country,
        set_release_country_placeholder
    );

    // A switch has no ‹multiple values› state to show, so a selection that disagrees starts off
    // and an untouched switch stays `Keep` — nothing is written unless the user moves it.
    originals.compilation = release.iter().all(|r| r.is_compilation) && !release.is_empty();
    te.set_compilation(originals.compilation);

    // Lyrics (single selection only; multi mode leaves it "" ⇒ Keep).
    te.set_lyrics(SharedString::from(lyrics.as_str()));
    te.set_lyrics_placeholder(SharedString::default());
    originals.text.lyrics = lyrics;

    finalize_populate(
        &te,
        session,
        rows,
        Baseline {
            originals,
            lists: ListFields {
                credits: [artist, album_artist],
                genres,
                roles: role_edit,
            },
            join_phrases: phrases,
        },
        resident,
        cover,
    );
}

/// Everything the commit diffs against, as it was the moment the dialog opened.
///
/// One value rather than four parameters because that is what the four are: a baseline. They are
/// written once by `populate` and read once by the commit, and `join_phrases` rides with them
/// because the picker list the dialog offers is part of what the user was shown.
struct Baseline {
    originals: FormState,
    lists: ListFields,
    join_phrases: Vec<(String, String)>,
}

/// Finish a `populate`: scalar flags, cover, Summary, and the session snapshot.
fn finalize_populate(
    te: &TagEditor,
    session: &Rc<RefCell<TagSession>>,
    rows: &[TagEditRow],
    baseline: Baseline,
    resident: Option<String>,
    cover: Option<SharedPixelBuffer<Rgb8Pixel>>,
) {
    let single = rows.len() == 1;
    te.set_track_count(len_as_i32(rows.len()));
    te.set_active_tab(te.get_tab_tags());
    te.set_lyrics_enabled(single);
    te.set_has_resident_lyrics(resident.is_some());

    if let Some(buf) = cover {
        te.set_cover(Image::from_rgb8(buf));
        te.set_has_cover(true);
    } else {
        te.set_cover(Image::default());
        te.set_has_cover(false);
    }

    // Single row ⇒ fill the Summary tab; multi ⇒ blank it (tab unmounted).
    set_summary(te, single.then(|| &rows[0]));

    *session.borrow_mut() = TagSession {
        ids: rows.iter().map(|r| r.id).collect(),
        originals: baseline.originals,
        original_lists: baseline.lists,
        join_phrases: baseline.join_phrases,
        artwork: ArtworkEdit::Keep,
        picked: None,
        resident_lyrics: resident,
    };
}

/// Fill the read-only Summary strings from a single row's technical columns, or
/// blank all ten when `row` is `None` (multi mode — the tab isn't mounted, but a
/// stale single-select value shouldn't linger).
fn set_summary(te: &TagEditor, row: Option<&TagEditRow>) {
    // Each field is the row's value, or "" when `row` is None.
    fn s(row: Option<&TagEditRow>, get: impl Fn(&TagEditRow) -> String) -> SharedString {
        SharedString::from(row.map(get).unwrap_or_default().as_str())
    }
    te.set_summary_path(s(row, |r| r.file_path.clone()));
    te.set_summary_codec(s(row, |r| r.codec.as_deref().unwrap_or_default().to_uppercase()));
    te.set_summary_bitrate(s(row, |r| r.bitrate.map(|b| format!("{b} kbps")).unwrap_or_default()));
    te.set_summary_sample_rate(s(row, |r| {
        r.sample_rate.map(format_sample_rate).unwrap_or_default()
    }));
    te.set_summary_bit_depth(s(row, |r| {
        r.bit_depth.map(|d| format!("{d}-bit")).unwrap_or_default()
    }));
    te.set_summary_channels(s(row, |r| r.channels.map(format_channels).unwrap_or_default()));
    te.set_summary_size(s(row, |r| r.file_size.map(fmt_size).unwrap_or_default()));
    te.set_summary_duration(s(row, |r| crate::ui::tracks::format_duration_ms(r.duration_ms)));
    te.set_summary_modified(s(row, |r| r.date_modified.as_deref().unwrap_or_default().to_owned()));
    te.set_summary_hash(s(row, |r| r.file_hash.as_deref().unwrap_or_default().to_owned()));
}
