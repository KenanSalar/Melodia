//! Source pins for `melodia-ui/ui/components/icon-button.slint` — the tree's
//! most-mounted control, and one with no Rust module of its own.

use crate::test_support::strip_line_comments;

const BUTTON: &str = include_str!("../../../melodia-ui/ui/components/icon-button.slint");

/// The glyph places itself, and a centring layout would be a real regression.
///
/// `gen_layout_info_prop` (`i-slint-compiler/passes/default_geometry.rs`) folds a
/// child's layout info into its parent's **unless the child binds `x` or `y`** —
/// which is the only reason `disc` has ever stayed out of it. A layout here binds
/// neither, so it folds, and with it the glyph's size and the animated
/// `press-scale` behind it. Nothing moves, the root pinning `min == max ==
/// diameter`; what it costs is that every press dirties this root's `layout_info`
/// and re-solves the *host* layout once per frame, at the twenty `press-shrink`
/// mounts. It shipped that way once and no test saw it — the tell is a
/// `+ …_layoutinfo_h` term in the generated `layout_info`.
#[test]
fn the_glyph_places_itself_rather_than_taking_a_layout() {
    let code = strip_line_comments(BUTTON);
    assert!(
        !code.contains("Layout {"),
        "`icon-button.slint` must declare no layout — a centring one folds its child's \
         layout info into the root and puts `press-scale` on the host's layout cache"
    );

    let glyph = code
        .split_once("MaterialIcon {")
        .and_then(|(_, rest)| rest.split_once("\n    }"))
        .map_or(String::new(), |(body, _)| body.to_owned());
    for axis in ["x:", "y:"] {
        assert!(
            glyph.contains(axis),
            "the glyph must bind `{axis}` — that binding is what keeps it out of the fold, so \
             dropping it is the same regression as reintroducing the layout"
        );
    }
}
