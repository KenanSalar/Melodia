//! The four view setters that decide something, and the getter that answers twice over.
//!
//! The rest of the module is a field assignment inside `mutate_view_state`, whose own suite
//! covers the round trip; a test of one of those would restate the assignment. These five have a
//! guard, a clamp, a dedupe or a fallback in front of the write, and each of those is invisible
//! from the outside once it is wrong: a rejected locale reads as a settings write that did not
//! happen, and a nav index past the last section lands the next boot somewhere unselectable.

use tempfile::TempDir;

use super::{
    read_view_sort, write_last_detail_id, write_last_nav_index, write_locale, write_overflow_button,
};
use crate::services;
use melodia_core::config::Paths;
use melodia_core::entities::locale::SUPPORTED_LOCALES;
use melodia_core::error::AppError;

/// A data root nothing else writes to, with the directories `Paths::resolve` would have made.
fn rooted(tmp: &TempDir) -> Result<Paths, AppError> {
    let paths = Paths::rooted_at(tmp.path().to_path_buf());
    paths.create_dirs()?;
    Ok(paths)
}

/// A code with no bundled catalogue is refused before the write, so `settings.json` cannot be
/// pinned to a locale Slint would fall back out of on every launch.
#[test]
fn an_unsupported_locale_is_refused_and_never_written() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = rooted(&tmp)?;
    let before = services::settings::read_settings(&paths)?.locale;

    let refused = write_locale(&paths, "xx-YZ".to_owned());

    assert!(matches!(refused, Err(AppError::Validation(_))));
    assert_eq!(services::settings::read_settings(&paths)?.locale, before);
    Ok(())
}

#[test]
fn a_bundled_locale_is_persisted() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = rooted(&tmp)?;
    let Some(&code) = SUPPORTED_LOCALES.iter().find(|&&code| code != "en") else {
        unreachable!("the app ships more than one locale")
    };

    write_locale(&paths, code.to_owned())?;

    assert_eq!(services::settings::read_settings(&paths)?.locale, code);
    Ok(())
}

/// The toggle is written from a row the user can bounce, so turning a button on twice has to
/// leave one entry and turning it off has to leave none. Without the unconditional remove the
/// list grows a duplicate per click and the overflow menu shows the same button twice.
#[test]
fn toggling_an_overflow_button_never_duplicates_it() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = rooted(&tmp)?;

    write_overflow_button(&paths, "shuffle".to_owned(), true)?;
    write_overflow_button(&paths, "shuffle".to_owned(), true)?;

    let buttons = services::settings::read_settings(&paths)?.overflow_buttons;
    assert_eq!(buttons.iter().filter(|id| *id == "shuffle").count(), 1);
    Ok(())
}

#[test]
fn turning_an_overflow_button_off_removes_it() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = rooted(&tmp)?;
    write_overflow_button(&paths, "shuffle".to_owned(), true)?;

    write_overflow_button(&paths, "shuffle".to_owned(), false)?;

    let buttons = services::settings::read_settings(&paths)?.overflow_buttons;
    assert!(!buttons.iter().any(|id| id == "shuffle"));
    Ok(())
}

/// Both ends of the nav bound, executed. `cross_tier.rs` pins that the clamp and the boot guard
/// take the same constant; what it cannot say is that the clamp runs, and an index past the last
/// section lands the next launch on a tab nothing routes.
#[test]
fn a_nav_index_outside_the_range_is_clamped_before_it_is_written() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = rooted(&tmp)?;
    let top = services::view_state::MAX_NAV_INDEX;

    write_last_nav_index(&paths, top + 1)?;
    assert_eq!(services::view_state::read_view_state(&paths)?.last_nav_index, top);

    write_last_nav_index(&paths, -3)?;
    assert_eq!(services::view_state::read_view_state(&paths)?.last_nav_index, 0);
    Ok(())
}

/// `None` is how a closed detail is spelled, and it has to remove the entry rather than write a
/// sentinel: an entry left behind reopens a detail the user navigated out of.
#[test]
fn closing_a_detail_drops_its_entry_rather_than_keeping_one() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = rooted(&tmp)?;

    write_last_detail_id(&paths, "album-detail", Some(7))?;
    assert_eq!(
        services::view_state::read_view_state(&paths)?.last_detail_ids.get("album-detail"),
        Some(&7),
    );

    write_last_detail_id(&paths, "album-detail", None)?;
    assert!(
        !services::view_state::read_view_state(&paths)?
            .last_detail_ids
            .contains_key("album-detail"),
    );
    Ok(())
}

/// A view that has never persisted a sort gets `None`, which is what lets every caller keep its
/// own default rather than the module inventing one.
#[test]
fn a_view_with_no_persisted_sort_answers_none() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = rooted(&tmp)?;

    assert!(read_view_sort(&paths, "tracks").is_none());
    Ok(())
}

#[test]
fn a_persisted_sort_comes_back_for_its_own_view_only() -> Result<(), AppError> {
    let tmp = TempDir::new()?;
    let paths = rooted(&tmp)?;
    let sort = services::settings::ViewSort {
        field: "album".to_owned(),
        dir: services::settings::SortDir::Desc,
    };
    services::view_state::mutate_view_state(&paths, |s| {
        s.view_sort.insert("tracks".to_owned(), sort.clone());
    })?;

    assert_eq!(read_view_sort(&paths, "tracks").map(|s| s.field), Some("album".to_owned()));
    assert!(read_view_sort(&paths, "browse").is_none());
    Ok(())
}
