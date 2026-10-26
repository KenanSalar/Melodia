//! Nothing in the tree carries a failure as a `String`.
//!
//! An error carried as one keeps its message and drops its cause, which is the whole of what
//! `error::describe` walks, and it flattens at the throw rather than at the read. The rule is
//! violable from any file and costs nothing to break, so a walk is what holds it — and it has to
//! be a walk over every crate, no one of them being able to answer for the others.
//!
//! The same loss has a second spelling: an error interpolated into a message-only `AppError`
//! constructor. Every variant but the three `#[from]` ones and `NotFound` takes a typed cause, so
//! that call is the one place left to flatten one.

use melodia_testkit::{inline_names, is_error_name, rust_sources};

/// Loose on purpose: below it the walk has stopped recognising calls, not the tree making them.
const MIN_MESSAGE_ONLY_CALLS: usize = 150;

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

/// `x_msg(format!("…: {e}"))` reads like a context message with its reason attached, and leaves
/// `describe` nothing to walk: the reason is text, so a log can't tell a full disk from a
/// permissions failure by anything but its words.
#[test]
fn no_cause_is_flattened_into_an_error_message() {
    let offenders: Vec<String> = message_only_calls()
        .into_iter()
        .filter(|call| call.names_an_error)
        .map(|call| call.site)
        .collect();

    assert!(
        offenders.is_empty(),
        "{offenders:?} hand an error to a message-only constructor as text. Build it with \
         `AppError::x(msg, e)` so the cause stays typed on `.source()`"
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

/// A call to a message-only `AppError` constructor, named `path:line`.
struct MessageOnlyCall {
    site: String,
    names_an_error: bool,
}

/// Every message-only `AppError` call in the tree. An argument list the walk can't close is
/// asserted on rather than skipped, being one it can't vouch for.
fn message_only_calls() -> Vec<MessageOnlyCall> {
    let mut calls = Vec::new();
    let mut unclosed = Vec::new();
    for (path, code) in rust_sources() {
        for (start, arguments) in message_only_arguments(&code) {
            let site = format!("{path}:{}", line_of(&code, start));
            match arguments {
                Some(arguments) => {
                    calls.push(MessageOnlyCall { site, names_an_error: names_an_error(arguments) });
                }
                None => unclosed.push(site),
            }
        }
    }

    assert!(
        unclosed.is_empty(),
        "{unclosed:?} open a message-only constructor whose arguments never close"
    );
    assert!(
        calls.len() >= MIN_MESSAGE_ONLY_CALLS,
        "only {} message-only constructor calls found, expected at least \
         {MIN_MESSAGE_ONLY_CALLS}: a walk that finds none passes everything",
        calls.len()
    );
    calls
}

/// Where each `AppError::*_msg(` or `AppError::io_other(` call starts in `code`, beside its
/// argument list, or `None` for a list that never closes.
fn message_only_arguments(code: &str) -> Vec<(usize, Option<&str>)> {
    const PATH: &str = "AppError::";

    let mut found = Vec::new();
    for (start, _) in code.match_indices(PATH) {
        if code[..start].ends_with(continues_identifier) {
            continue;
        }
        let rest = &code[start + PATH.len()..];
        let name_len =
            rest.find(|character| !continues_identifier(character)).unwrap_or(rest.len());
        let name = &rest[..name_len];
        if !(name.ends_with("_msg") || name == "io_other") {
            continue;
        }
        let Some(after_open) = rest[name_len..].trim_start().strip_prefix('(') else {
            continue;
        };
        found.push((start, call_arguments(after_open)));
    }
    found
}

/// The text up to the `)` closing an already-opened argument list. Literals are skipped, so a
/// paren inside a message can't end the call early.
fn call_arguments(after_open: &str) -> Option<&str> {
    let bytes = after_open.as_bytes();
    let mut depth = 1_usize;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'"' => at += literal_end(&after_open[at + 1..]) + 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&after_open[..at]);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// Whether `arguments` interpolate an error binding: captured inline by a literal (`{e}`,
/// `{err:?}`), or named in the code between the literals, which covers a positional format
/// argument, an `e.to_string()` and a `describe(&e)` alike.
fn names_an_error(arguments: &str) -> bool {
    let (literals, code) = split_literals(arguments);
    let captured = literals.iter().flat_map(|literal| inline_names(literal)).any(is_error_name);
    let named = code.split(|character| !continues_identifier(character)).any(is_error_name);
    captured || named
}

/// The bodies of the string literals in `arguments`, and the code around them.
fn split_literals(arguments: &str) -> (Vec<&str>, String) {
    let mut literals = Vec::new();
    let mut code = String::with_capacity(arguments.len());
    let mut rest = arguments;
    while let Some(open) = rest.find('"') {
        code.push_str(&rest[..open]);
        code.push(' ');
        let body = &rest[open + 1..];
        let close = literal_end(body);
        literals.push(&body[..close]);
        rest = body.get(close + 1..).unwrap_or_default();
    }
    code.push_str(rest);
    (literals, code)
}

/// Where the literal whose body opens `body` closes: its first unescaped `"`, or the end.
fn literal_end(body: &str) -> usize {
    let mut escaped = false;
    for (offset, character) in body.char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return offset,
            _ => {}
        }
    }
    body.len()
}

fn continues_identifier(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn line_of(code: &str, at: usize) -> usize {
    code[..at].matches('\n').count() + 1
}
