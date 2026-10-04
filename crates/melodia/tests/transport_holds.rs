//! What a held transport button does is read off `Player`, never spelled where it is mounted.
//!
//! The bar and the miniplayer each mount Previous, Play/Pause and Next, and the hold half of all
//! three lives on the global so the two cannot come to behave differently. Nothing about a mount
//! spelling its own fails a build, and the relative run a scan walks has the same exposure one
//! file over: its timer and the windows counted in its ticks sit in different sheets.

use melodia_testkit::{
    MIN_SLINT_SOURCES, UI_DIR, binding_value, block_after, block_body, normalize_ws,
    strip_line_comments, stripped_sources,
};

const SHORTCUT_SCOPE: &str = include_str!("../../melodia-ui/ui/layout/shortcut-scope.slint");

/// A transport button, by the call its click makes, and the reads every mount of it owes.
///
/// Keyed on the click rather than the component: the miniplayer mounts `MiniIconButton`, and a
/// walk over `IconButton` blocks reads straight past it.
struct Transport {
    click: &'static str,
    reads: &'static [&'static str],
}

const TRANSPORT: [Transport; 3] = [
    Transport {
        click: "Player.previous()",
        reads: &[
            "hold-delay: Player.scan-hold;",
            "repeat-interval: Player.scan-interval;",
            "tooltip-text: Player.previous-tooltip;",
            "held(step) => { Player.scan-step(-1, step); }",
        ],
    },
    Transport {
        click: "Player.play-pause()",
        reads: &[
            "hold-delay: Player.stop-hold;",
            "is-stopped: Player.stopped-at-top;",
            "held => { Player.stop(); }",
        ],
    },
    Transport {
        click: "Player.next()",
        reads: &[
            "hold-delay: Player.scan-hold;",
            "repeat-interval: Player.scan-interval;",
            "tooltip-text: Player.next-tooltip;",
            "held(step) => { Player.scan-step(1, step); }",
        ],
    },
];

/// Vacuity floor per button: the bar and the miniplayer. One would pass a walk that has stopped
/// seeing either, and a floor rather than an equality walks a third transport on arrival.
const MIN_MOUNTS: usize = 2;

/// The body of the innermost block open at `at`, braces excluded. Quote-aware for
/// [`block_body`]'s reason; pair it with [`strip_line_comments`] as that does.
fn enclosing_block(src: &str, at: usize) -> Option<&str> {
    let bytes = src.as_bytes();
    let mut open = Vec::new();
    let mut in_string = false;
    let mut i = 0;
    while i < at {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'{' if !in_string => open.push(i),
            b'}' if !in_string => {
                open.pop();
            }
            _ => {}
        }
        i += 1;
    }
    block_body(src, *open.last()?)
}

/// Every element in `src` whose own `clicked` handler calls `click`, by its body.
///
/// The handler is read rather than the element, so a container holding a transport button is
/// not taken for one.
fn mounts_clicking<'a>(src: &'a str, click: &str) -> Vec<&'a str> {
    src.match_indices("clicked =>")
        .filter(|(at, _)| block_after(&src[*at..], "clicked =>").contains(click))
        .filter_map(|(at, _)| enclosing_block(src, at))
        .collect()
}

/// **Every transport button takes its hold from `Player`.**
///
/// Walked rather than listed: the mount that gets it wrong is the third one nobody has written
/// yet, and one wiring the clicks while forgetting the holds passes every other check.
#[test]
fn every_transport_button_takes_its_hold_from_player() {
    let sources = stripped_sources(UI_DIR, "slint", MIN_SLINT_SOURCES);
    let mut offenders = Vec::new();

    for transport in &TRANSPORT {
        let mut mounts = 0usize;
        for (path, src) in &sources {
            for mount in mounts_clicking(src, transport.click) {
                mounts += 1;
                let tokens = normalize_ws(mount);
                for read in transport.reads.iter().filter(|read| !tokens.contains(*read)) {
                    offenders.push(format!("{path}: the {} mount lacks `{read}`", transport.click));
                }
            }
        }
        assert!(
            mounts >= MIN_MOUNTS,
            "only {mounts} mounts click through to `{}`; the walk has stopped matching",
            transport.click
        );
    }

    assert!(
        offenders.is_empty(),
        "a transport mount spells its own hold, or none, where `Player` already holds one; a \
         third transport reads the same properties as the bar and the miniplayer:\n{}",
        offenders.join("\n")
    );
}

/// **The relative run's timer ticks at `Player.seek-run-tick`, the period its windows are counted
/// in.** A scan sizes its window in those ticks to outlast one scan interval, so a timer with a
/// period of its own lets the run lapse between two steps of one hold, and the second walks from
/// the reported position instead of the first one's target.
#[test]
fn the_seek_run_timer_ticks_at_the_period_its_windows_are_counted_in() {
    let src = strip_line_comments(SHORTCUT_SCOPE);
    let timer = block_after(&src, "seek-run-timer := Timer");

    assert_eq!(
        binding_value(timer, "interval:").trim(),
        "Player.seek-run-tick",
        "layout/shortcut-scope.slint's seek-run-timer"
    );
}
