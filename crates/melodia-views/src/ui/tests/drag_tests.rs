use super::lifted_run;
use melodia_ui::LiftedRun;

/// What a drag of the picks tells the drop target. A run widens the slots that draw no line to
/// itself; scattered picks have none, since any drop gathers them.
#[test]
fn the_lifted_run_is_the_picks_where_they_sit_together() {
    const NONE: LiftedRun = LiftedRun { first: -1, count: 0 };
    let cases: [(&[usize], LiftedRun, &str); 5] = [
        (&[], NONE, "no rows"),
        (&[4], LiftedRun { first: 4, count: 1 }, "one row"),
        (&[2, 3, 4], LiftedRun { first: 2, count: 3 }, "a run"),
        (&[0, 1], LiftedRun { first: 0, count: 2 }, "a run from the top row"),
        (&[1, 3], NONE, "rows with a gap between them"),
    ];
    for (rows, expected, case) in cases {
        assert_eq!(lifted_run(rows), expected, "{case}");
    }
}
