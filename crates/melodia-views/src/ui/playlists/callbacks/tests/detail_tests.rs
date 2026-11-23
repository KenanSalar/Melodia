use super::Landing;
use crate::ui::shell::notifications::Completion;

/// Landed at the end, the tracks may be out of sight, and a skipped one shows nowhere.
#[test]
fn a_drop_toasts_unless_every_track_landed_at_the_line() {
    let cases = [
        ("every track at the line", Landing::Slot(2), 3, None),
        ("one skipped at the line", Landing::Slot(2), 2, Some(Completion::Partial)),
        ("every one skipped at the line", Landing::Slot(2), 0, Some(Completion::Partial)),
        ("every track at the end", Landing::End, 3, Some(Completion::Complete)),
        ("one skipped at the end", Landing::End, 2, Some(Completion::Partial)),
        ("every one skipped at the end", Landing::End, 0, Some(Completion::Partial)),
    ];
    for (what, landing, landed, expected) in cases {
        assert_eq!(landing.notice(3, landed), expected, "{what}");
    }
}
