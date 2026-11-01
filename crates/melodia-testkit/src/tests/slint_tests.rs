use super::{head_words, mounted_type, own_mounts};

/// The `{` a sample ends on, the block each case asks about.
fn last_open(sample: &str) -> usize {
    sample.rfind('{').unwrap_or_default()
}

#[test]
fn head_words_read_back_to_the_previous_statement_or_block_edge() {
    let samples = [
        "export component Foo inherits Rectangle {",
        "id := Foo {",
        "width: 4px; Foo {",
        "Rectangle { Foo {",
        "Text { } Foo {",
    ];

    let heads: Vec<Vec<&str>> =
        samples.iter().map(|sample| head_words(sample, last_open(sample))).collect();

    assert_eq!(
        heads,
        [
            vec!["export", "component", "Foo", "inherits", "Rectangle"],
            vec!["id", ":=", "Foo"],
            vec!["Foo"],
            vec!["Foo"],
            vec!["Foo"],
        ]
    );
}

#[test]
fn mounted_type_names_the_type_each_mount_spelling_mounts() {
    let samples =
        ["Rectangle {", "label := Text {", "if shown: Image {", "for row in rows: TrackRow {"];

    let types: Vec<Option<&str>> =
        samples.iter().map(|sample| mounted_type(sample, last_open(sample))).collect();

    assert_eq!(types, [Some("Rectangle"), Some("Text"), Some("Image"), Some("TrackRow")]);
}

/// A head read as a mount hands a walk children that aren't there, so an `opacity` beside it
/// reads as drawing through a texture.
#[test]
fn mounted_type_reads_no_type_off_a_block_that_mounts_nothing() {
    let samples = [
        "export component Card inherits Rectangle {",
        "struct Row {",
        "enum Kind {",
        "global Theme {",
        "pure function tint() -> Color {",
        "clicked => {",
        "states [ hover when touch.has-hover : {",
        "animate opacity {",
        "property <Point> origin: {",
    ];

    let misread: Vec<(&str, &str)> = samples
        .iter()
        .filter_map(|sample| Some((*sample, mounted_type(sample, last_open(sample))?)))
        .collect();

    assert!(misread.is_empty(), "read as mounting a type: {misread:?}");
}

#[test]
fn own_mounts_leaves_out_what_a_child_mounts() {
    let body = "Rectangle { Text { } } clicked => { } Image { }";

    let mounts: Vec<(usize, &str)> = own_mounts(body).collect();

    assert_eq!(mounts, [(10, "Rectangle"), (44, "Image")]);
}
