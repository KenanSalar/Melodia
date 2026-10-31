//! Source pins for `SliderPointer`, the one place a value slider decides what the pointer does.
//!
//! Each rule here builds and looks right without it, and each was a defect first: a right click
//! moved the value, and a grab that never moved seeked back to where the press landed.

use melodia_testkit::{block_after, code_tokens};

const POINTER: &str = include_str!("../../../../melodia-ui/ui/components/slider-pointer.slint");

fn pointer_event(code: &str) -> &str {
    block_after(code, "pointer-event(ev) =>").trim()
}

/// A `TouchArea` arms on a press of any button and only `pressed` is left-only, so the handler
/// filters the button itself. A cancel carries no button, and has to get through to end the drag.
#[test]
fn a_slider_takes_no_button_but_the_left() {
    let code = code_tokens(POINTER);

    let handler = pointer_event(&code);

    assert!(
        handler.starts_with(
            "if (ev.button != PointerEventButton.left && ev.kind != PointerEventKind.cancel) \
             { return; }"
        ),
        "the handler no longer opens by refusing every button but the left, cancel aside"
    );
}

/// The player advances under a held thumb, so committing a grab that never moved would seek back to
/// where the press landed.
#[test]
fn a_slider_commits_only_what_its_press_moved() {
    let code = code_tokens(POINTER);

    let handler = pointer_event(&code);

    assert_eq!(code.matches("root.committed(").count(), 1, "a second path commits the value");
    assert!(
        handler.contains("&& root.moved-since-press) { root.committed(root.drag-value);"),
        "the release commits whether or not the press moved anything"
    );
}

/// `pressed` is already true when the handler sees the press, so a flag left from the last press
/// would turn `dragging` on and commit that press's movement for this one.
#[test]
fn a_press_starts_with_nothing_moved() {
    let code = code_tokens(POINTER);

    let handler = pointer_event(&code);

    assert!(
        handler.contains("if (ev.kind == PointerEventKind.down) { root.moved-since-press = false;"),
        "a press no longer clears the last press's movement before anything reads it"
    );
}
