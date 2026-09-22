//! Source pins for the decoration buttons, across the two hosts that mount them.
//!
//! Every line held here builds, reads as deliberate, and is wrong only against
//! something no test runner ever sees: the decoration the OS draws beside the
//! window, and the miniplayer's own backdrop, which is off by default and off in
//! CI. A centred pill is a defensible caption at any size and any offset, so the
//! three facts that make these buttons Windows 11's rather than merely round
//! have no reviewer to answer to.

use melodia_testkit::strip_line_comments;

const CAPTION_BUTTONS: &str =
    include_str!("../../../../melodia-ui/ui/components/caption-buttons.slint");
const MINI_CAPTIONS: &str =
    include_str!("../../../../melodia-ui/ui/components/mini-player/mini-captions.slint");

/// The lines up to the brace closing the element `rest` opens inside.
fn brace_body(rest: &str) -> String {
    let mut depth = 1usize;
    let mut body: Vec<&str> = Vec::new();
    for line in rest.lines() {
        depth += line.matches('{').count();
        depth = depth.saturating_sub(line.matches('}').count());
        if depth == 0 {
            break;
        }
        body.push(line);
    }
    body.join("\n")
}

/// The value bound after `prefix`, whitespace-normalized onto one line.
///
/// Callers pass a prefix long enough to be unambiguous: a bare name matches the
/// mounts that forward it as well as the declaration that states it.
fn binding_after(src: &str, prefix: &str) -> String {
    let rest = src.split_once(prefix).map_or("", |(_, rest)| rest);
    let value = rest.split_once(';').map_or("", |(value, _)| value);
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A property of the element whose body this is, found by its indented name so
/// `width` can't read `min-width` back.
fn binding(body: &str, name: &str) -> String {
    let found = binding_after(body, &format!(" {name}:"));
    assert!(!found.is_empty(), "no terminated `{name}` binding here, so every read of it passes");
    found
}

/// `CaptionButton`'s hover fill: the first element declared inside the component,
/// past the `inherits Rectangle` the root itself is.
fn fill_body() -> String {
    let code = strip_line_comments(CAPTION_BUTTONS);
    let inside = code.split_once("inherits Rectangle {").map_or("", |(_, rest)| rest);
    let rest = inside.split_once("Rectangle {").map_or("", |(_, rest)| rest);
    assert!(
        !rest.is_empty(),
        "`CaptionButton` declares no `Rectangle` of its own any more — every assertion over that \
         body would walk an empty string and pass"
    );
    brace_body(rest)
}

/// The captions row's inner layout, anchored on the fade it is declared under.
fn row_layout_body() -> String {
    let code = strip_line_comments(MINI_CAPTIONS);
    let inside = code.split_once("opacity: root.row-fade;").map_or("", |(_, rest)| rest);
    let rest = inside.split_once("HorizontalLayout {").map_or("", |(_, rest)| rest);
    assert!(
        !rest.is_empty(),
        "the captions row no longer mounts a layout under its fade — this pin is reading nothing"
    );
    brace_body(rest)
}

/// The fill is the cell, not a pill inside it.
///
/// An inset fill is what this replaced, and its argument — a click target wider
/// than the highlight — never held, the `TouchArea` being the whole cell either
/// way. What the inset actually cost is the only thing that says "caption"
/// rather than "button": close running into the window's corner, which is
/// visible solely against a native decoration on the same screen.
#[test]
fn the_standard_caption_fill_is_the_whole_cell() {
    let fill = fill_body();

    let (width, height) = (binding(&fill, "width"), binding(&fill, "height"));

    assert!(
        width.ends_with(": parent.width") && height.ends_with(": parent.height"),
        "the standard fill must be the whole cell, and is `{width}` by `{height}`: inset, close \
         stops short of the window corner it is supposed to run into"
    );
}

/// Square, and the shell's clip is what rounds it at the corner.
///
/// The radius this replaced tracked `Theme.shell-radius`, which reads as care
/// and is exactly wrong: a caption concentric with the window's arc is a pill
/// sitting near the corner rather than a cell meeting it, and the arc is the
/// clip's to draw.
#[test]
fn the_standard_caption_fill_is_square() {
    let radius = binding(&fill_body(), "border-radius");

    assert!(
        radius.ends_with(": 0px"),
        "the standard fill must carry no radius of its own, and carries `{radius}`: the window's \
         clip is what rounds it where it meets the corner"
    );
}

/// The miniplayer's cell is the titlebar's, stated rather than restated.
///
/// Only the macOS arm reads it as a constraint — a `HorizontalLayout` stretches
/// its cross-axis children over their min and max, where the cluster's
/// `GridLayout` honours both — so a literal here leaves the traffic lights
/// top-aligned above the two styles that followed the layout, on the one arm
/// nothing else in the row can show.
#[test]
fn the_miniplayer_caption_cell_is_the_titlebars() {
    let cell = binding_after(
        &strip_line_comments(MINI_CAPTIONS),
        "private property <length> cell-height:",
    );

    assert_eq!(
        cell, "MiniPlayer.captions-height",
        "the miniplayer's cell must be the row's own height rather than a number beside it, or \
         the macOS lights cap where the drawn captions no longer do"
    );
}

/// The row lands on the window's top edge, as the titlebar's does.
///
/// Two halves of one landing: the cell is the full row, and it reaches back over
/// the host's edge inset once open. Either alone leaves the captions short or
/// low, which a centred glyph hides and a fill running to three of four window
/// edges does not.
#[test]
fn the_miniplayer_captions_reach_over_the_hosts_inset() {
    let row = row_layout_body();

    let (height, y) = (binding(&row, "height"), binding(&row, "y"));

    assert_eq!(
        height, "MiniPlayer.captions-height",
        "the row's layout must be the whole captions height, and is `{height}`"
    );
    assert!(
        y.contains("root.edge-pad * root.open-t"),
        "the row must reach back over the host's inset as it opens, and places itself at `{y}`: \
         without it the cells sit a pad below the window's top edge"
    );
}

/// A caption keeps a fill on the backdrop, where the round controls give theirs up.
///
/// `MiniGlyphs.hover-bg` goes transparent there because a disc answers with
/// `hover-lift` instead, and a caption has no lift. Left on that brush, minimize
/// and the full-player button answer a pointer with nothing while close beside
/// them still flashes red — under a setting that is off by default, so nothing
/// but turning it on ever shows it.
#[test]
fn the_miniplayer_caption_fill_survives_the_backdrop() {
    let hover = binding_after(
        &strip_line_comments(MINI_CAPTIONS),
        "private property <brush> caption-hover-bg:",
    );

    assert!(
        hover.contains("Player.np-accent-disc-hover"),
        "the standard caption's hover fill must take the backdrop's own tier, and is `{hover}`: \
         `MiniGlyphs.hover-bg` alone paints nothing once the backdrop is up"
    );
}
