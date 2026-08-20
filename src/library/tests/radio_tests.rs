/// The count records that the user *chose* a station, so it must not be conditional on the server
/// being up — and the natural spelling, `player_play_station(..).await?` ahead of `mark_played`,
/// makes it exactly that. Pinned by reading the source because the alternative needs an
/// `AppState`, a socket and a station that is reliably down; the ordering is the whole invariant
/// and it is legible from the text.
#[test]
fn a_station_that_cannot_be_reached_is_still_counted_as_played() {
    let source = include_str!("../radio.rs");
    let body = source
        .split_once("pub async fn play_station")
        .and_then(|(_, rest)| rest.split_once("\n}\n"))
        .map_or("", |(body, _)| body);

    assert!(!body.is_empty(), "`play_station` moved or changed shape, so this pin reads nothing");
    assert!(
        matches!(
            (body.find("mark_played"), body.find("player_play_station")),
            (Some(counted), Some(opened)) if counted < opened
        ),
        "`play_station` must count the play before it opens the stream, or a station that is down \
         today never reaches the recents list that would let the user find it again"
    );
}
