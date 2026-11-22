use super::album_card;
use crate::ui::fixtures::dated_album;

#[test]
fn an_album_card_reads_its_year_and_nothing_for_none_zero_or_below() {
    let years = [None, Some(-1), Some(0), Some(1)];

    let subtitles = years.map(|year| album_card(&dated_album(year)).subtitle);

    assert_eq!(subtitles, ["", "", "", "1"]);
}
