//! When a section's card grid is what the window draws, which is what a cover prewarm asks: the
//! tier keeps what it decodes until a card draws it again, and a grid under a detail draws
//! nothing.

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
