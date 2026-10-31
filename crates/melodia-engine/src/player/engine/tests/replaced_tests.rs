//! What an undone play returns the deck to: whatever the replaced queue had on it, as it was.

use crate::player::engine::fixtures::{test_station, test_track};
use crate::player::engine::state::{
    PlayerAction, PlayerState, ReplacedQueue, play_track_inner, restore_station,
    resume_from_stopped,
};
use crate::player::engine::types::PlaybackStatus;
use melodia_core::error::AppError;

/// Where the held track was, apart from anywhere a fresh start would put it.
const POSITION: u64 = 42_000;

/// The deck after an undo: its status, the title or station on it, its position, and where a
/// start asked to begin, `None` where nothing started.
type Deck<'a> = (PlaybackStatus, Option<&'a str>, u64, Option<u64>);

/// Puts the deck in the state a play then replaces.
type Hold = fn(&mut PlayerState);

/// `Held` on the deck at [`POSITION`], under `status`.
fn holding_track(state: &mut PlayerState, status: PlaybackStatus) {
    play_track_inner(state, test_track("Held", None, None), None);
    state.position_ms = POSITION;
    state.status = status;
}

/// A queue built by hand with the deck as `held` leaves it, replaced by a play that started a
/// track of its own, with what undoes that play.
fn played_over(held: Hold) -> Result<(PlayerState, ReplacedQueue), AppError> {
    let mut state = PlayerState::default();
    state.queue.add_tracks(&[7, 8]);
    held(&mut state);
    let replaced = state
        .replace_queue(&[1, 2], 0)
        .ok_or_else(|| AppError::validation_msg("a queue built by hand is held"))?;
    play_track_inner(&mut state, test_track("Played", None, None), None);
    Ok((state, replaced))
}

fn deck<'a>(state: &'a PlayerState, actions: &[PlayerAction]) -> Deck<'a> {
    let on_deck = state
        .current_track()
        .map(|track| track.title.as_str())
        .or_else(|| state.station().map(|station| station.name.as_str()));
    let started_at = actions.iter().find_map(|action| match action {
        PlayerAction::PlayMedia { start_position_ms, .. } => *start_position_ms,
        _ => None,
    });
    (state.status, on_deck, state.position_ms, started_at)
}

#[test]
fn an_undone_play_returns_the_deck_to_what_it_held() -> Result<(), AppError> {
    let rows: [(&str, Hold, Deck); 5] = [
        (
            "a track playing",
            |state| holding_track(state, PlaybackStatus::Playing),
            (PlaybackStatus::Playing, Some("Held"), POSITION, Some(POSITION)),
        ),
        (
            "a track paused",
            |state| holding_track(state, PlaybackStatus::Paused),
            (PlaybackStatus::Paused, Some("Held"), POSITION, Some(POSITION)),
        ),
        (
            "a track stopped",
            |state| holding_track(state, PlaybackStatus::Stopped),
            (PlaybackStatus::Stopped, Some("Held"), POSITION, None),
        ),
        (
            "a station playing",
            |state| {
                restore_station(state, test_station("Station"));
                state.status = PlaybackStatus::Playing;
            },
            (PlaybackStatus::Paused, Some("Station"), 0, None),
        ),
        ("nothing", |_| {}, (PlaybackStatus::Stopped, None, 0, None)),
    ];
    for (what, held, expected) in rows {
        let (mut state, replaced) = played_over(held)?;

        let actions = state.restore_replaced_queue(replaced, &mut rand::rng());

        assert_eq!(deck(&state, &actions), expected, "{what}");
    }
    Ok(())
}

/// A start still waiting on its record holds no track on the deck, so the undo leaves it stopped;
/// its row stays current, so one Play owes the same start again.
#[test]
fn an_undone_play_over_a_start_waiting_on_its_record_leaves_that_row_to_play()
-> Result<(), AppError> {
    let (mut state, replaced) = played_over(|state| {
        state.queue.seat(1);
        state.start_queued(8, false, 0);
    })?;
    state.restore_replaced_queue(replaced, &mut rand::rng());

    let play = resume_from_stopped(&mut state);

    let owes_the_row = play
        .iter()
        .any(|action| matches!(action, PlayerAction::StartWhenResolved(owed) if owed.id == 8));
    assert!(owes_the_row, "Play after the undo: {play:?}");
    Ok(())
}
