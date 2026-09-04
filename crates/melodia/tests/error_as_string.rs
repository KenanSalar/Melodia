//! Nothing in the tree carries a failure as a `String`.
//!
//! An error carried as one keeps its message and drops its cause, which is the whole of what
//! `error::describe` walks, and it flattens at the throw rather than at the read. The rule is
//! violable from any file and costs nothing to break, so a walk is what holds it — and it has to
//! be a walk over every crate, no one of them being able to answer for the others.

use melodia_testkit::rust_sources;

/// An error carried as a `String` keeps its message and drops its cause, which is the whole of
/// what `error::describe` walks. Nothing in the Rust tree needs to: `AppError` reaches
/// everywhere, and where it cannot (`radio_blocklist::source` is `include!`d into `build.rs`, so
/// it may name no `crate::` path) the answer is a local type implementing `std::error::Error`.
/// The rule is violable from any file and costs nothing to break, so a walk is what holds it.
#[test]
fn no_result_carries_its_error_as_a_string() {
    let offenders: Vec<String> = rust_sources()
        .into_iter()
        .filter(|(_, code)| string_errored_results(code) > 0)
        .map(|(path, _)| path)
        .collect();

    assert!(
        offenders.is_empty(),
        "{offenders:?} carry a failure as a plain `String`. Keep it typed: `AppError`, or a \
         local type implementing `std::error::Error` where `AppError` cannot be named, and \
         flatten to text at the point the error is read"
    );
}

/// How many of `code`'s `Result`s close on a bare `String` error argument.
fn string_errored_results(code: &str) -> usize {
    const OPENER: &str = "Result<";

    let mut found = 0;
    let mut rest = code;
    while let Some(start) = rest.find(OPENER) {
        rest = &rest[start + OPENER.len()..];
        let closed = generic_arguments(rest);
        if closed.and_then(error_argument) == Some("String") {
            found += 1;
        }
    }
    found
}

/// The text up to the `>` closing an already-opened generic list.
///
/// A `>` behind a `-` is a return arrow rather than a close, which a `Fn` bound inside the list
/// would otherwise use to end it early.
fn generic_arguments(after_opener: &str) -> Option<&str> {
    let mut depth = 1_usize;
    let mut previous = ' ';
    for (offset, character) in after_opener.char_indices() {
        match character {
            '<' => depth += 1,
            '>' if previous != '-' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&after_opener[..offset]);
                }
            }
            _ => {}
        }
        previous = character;
    }
    None
}

/// The last of `arguments`' top-level parameters, or `None` when there is only one.
///
/// A single-parameter `Result` is an alias carrying its own error (`io::Result<T>`), which has
/// no error position to judge.
fn error_argument(arguments: &str) -> Option<&str> {
    let mut depth = 0_usize;
    let mut last = None;
    for (offset, character) in arguments.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => last = Some(arguments[offset + 1..].trim()),
            _ => {}
        }
    }
    last
}
