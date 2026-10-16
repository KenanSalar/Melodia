//! When a section's card grid is what the window draws.

use super::SectionState;

fn on_screen() -> SectionState {
    let section = SectionState::new();
    section.set_active(true);
    section
}

#[test]
fn a_section_off_screen_draws_no_grid() {
    assert!(!SectionState::new().grid_on_screen(|| false));
}

#[test]
fn a_grid_under_an_open_detail_is_not_drawn() {
    assert!(!on_screen().grid_on_screen(|| true));
}

/// A launch reopening the last detail writes its id only once its rows are in, so through that
/// window nothing but the flag says the grid is covered.
#[test]
fn a_grid_under_a_detail_still_being_reopened_is_not_drawn() {
    let section = on_screen();

    section.begin_restore();

    assert!(!section.grid_on_screen(|| false));
}

#[test]
fn a_section_up_with_no_detail_over_it_draws_its_grid() {
    assert!(on_screen().grid_on_screen(|| false));
}

/// Lowered however the reopen went, a failed one included, which hands the launch back its grid.
#[test]
fn a_finished_reopen_hands_the_grid_back() {
    let section = on_screen();
    section.begin_restore();

    section.end_restore();

    assert!(section.grid_on_screen(|| false));
}

// --- the reopen a launch parks for a hidden tab's first enter ---

const PARKED: i64 = 7;

/// Every enter of the tab asks, and only the first may reopen the launch's detail: a second would
/// pull it back over whatever the user opened since.
#[test]
fn a_parked_reopen_is_handed_out_once() {
    let section = SectionState::new();
    section.defer_restore(PARKED);

    let enters = (section.take_deferred_restore(|| false), section.take_deferred_restore(|| false));

    assert_eq!(enters, (Some(PARKED), None), "(first enter, second enter)");
}

/// A drill that reached the tab first is the newer intent.
#[test]
fn a_drill_that_landed_first_drops_the_parked_reopen() {
    let section = SectionState::new();
    section.defer_restore(PARKED);

    let resumed = section.take_deferred_restore(|| true);

    assert_eq!(resumed, None);
}

/// Dropped means gone: closing the drilled detail must not bring the launch's back over the grid.
#[test]
fn a_dropped_reopen_does_not_come_back_once_the_detail_closes() {
    let section = SectionState::new();
    section.defer_restore(PARKED);
    let _dropped = section.take_deferred_restore(|| true);

    let resumed = section.take_deferred_restore(|| false);

    assert_eq!(resumed, None);
}

#[test]
fn a_section_with_nothing_parked_hands_out_nothing() {
    assert_eq!(SectionState::new().take_deferred_restore(|| false), None);
}
