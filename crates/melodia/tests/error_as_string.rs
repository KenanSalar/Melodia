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

#[test]
fn a_string_in_the_error_position_is_caught() {
    let cases = [
        ("unit ok", "fn f() -> Result<(), String> {"),
        ("nested ok", "fn f() -> Result<HashMap<String, Vec<u8>>, String> {"),
        ("a Fn bound", "fn f() -> Result<Box<dyn Fn(u8) -> u8>, String> {"),
        ("a path", "fn f() -> std::result::Result<u8, String> {"),
    ];

    for (case, src) in cases {
        assert_eq!(string_errored_results(src), 1, "{case}: a String error went unseen");
    }
}

#[test]
fn a_string_elsewhere_passes() {
    let cases = [
        ("in the ok position", "fn f() -> Result<String, AppError> {"),
        ("an alias carrying its own error", "fn f() -> io::Result<String> {"),
        ("a boxed error", "fn f() -> Result<(), Box<dyn Error>> {"),
        ("a type named after it", "fn f() -> Result<(), StringError> {"),
        ("nested inside the error", "fn f() -> Result<(), Vec<String>> {"),
    ];

    for (case, src) in cases {
        assert_eq!(string_errored_results(src), 0, "{case}: flagged as a String error");
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Hands the constructor no error binding.
    Clean,
    /// Interpolates or names an error binding.
    Flattened,
    /// An argument list the walk can't close.
    Unclosed,
}

struct MessageOnlyCall {
    line: usize,
    verdict: Verdict,
}

/// The tree's message-only calls, each named `path:line` under the verdict it got.
struct Walk {
    flattened: Vec<String>,
    unclosed: Vec<String>,
    read: usize,
}

fn walk_tree() -> Walk {
    let mut walk = Walk { flattened: Vec::new(), unclosed: Vec::new(), read: 0 };
    for (path, code) in rust_sources() {
        for call in message_only_calls(&code) {
            let site = format!("{path}:{}", call.line);
            match call.verdict {
                Verdict::Clean => walk.read += 1,
                Verdict::Flattened => {
                    walk.read += 1;
                    walk.flattened.push(site);
                }
                Verdict::Unclosed => walk.unclosed.push(site),
            }
        }
    }
    walk
}

/// `x_msg(format!("…: {e}"))` reads like a context message with its reason attached, and leaves
/// `describe` nothing to walk: the reason is text, so a log can't tell a full disk from a
/// permissions failure by anything but its words.
#[test]
fn no_cause_is_flattened_into_an_error_message() {
    let walk = walk_tree();

    assert!(
        walk.flattened.is_empty(),
        "{:?} hand an error to a message-only constructor as text. Build it with \
         `AppError::x(msg, e)` so the cause stays typed on `.source()`",
        walk.flattened
    );
}

/// An argument list the walk can't close is one it can't vouch for, so it fails the walk rather
/// than being skipped.
#[test]
fn every_message_only_call_in_the_tree_is_read() {
    let walk = walk_tree();

    assert!(
        walk.unclosed.is_empty(),
        "{:?} open a message-only constructor whose arguments never close",
        walk.unclosed
    );
}

#[test]
fn the_walk_finds_the_trees_message_only_calls() {
    let walk = walk_tree();

    assert!(
        walk.read >= MIN_MESSAGE_ONLY_CALLS,
        "only {} message-only constructor calls read, expected at least \
         {MIN_MESSAGE_ONLY_CALLS}: a walk that finds none passes everything",
        walk.read
    );
}

/// `describe(&e)` and `e.kind()` pass the log walk and are caught here: handed to a message, each
/// is still the cause as text.
#[test]
fn an_error_handed_over_as_text_is_caught() {
    let cases = [
        ("inline", r#"AppError::validation_msg(format!("bad path: {e}"))"#),
        ("debug spec", r#"AppError::queue_msg(format!("restore failed: {err:?}"))"#),
        ("suffix", r#"AppError::player_msg(format!("open failed: {last_error}"))"#),
        ("positional", r#"AppError::validation_msg(format!("bad path: {}", e))"#),
        ("after other args", r#"AppError::validation_msg(format!("{} is bad: {}", path, e))"#),
        ("to_string", "AppError::settings_msg(e.to_string())"),
        ("describe", "AppError::queue_msg(describe(&e))"),
        ("a field of the error", r#"AppError::io_other(format!("open failed: {:?}", e.kind()))"#),
        ("io_other", r#"AppError::io_other(format!("open failed: {e}"))"#),
        ("full path", r#"melodia_core::error::AppError::window_msg(format!("pin failed: {e}"))"#),
        ("space before paren", r#"AppError::validation_msg (format!("bad path: {e}"))"#),
        ("closing parens in literal", r#"AppError::player_msg(format!("stream closed :)) {e}"))"#),
        ("escaped quote", r#"AppError::player_msg(format!("say \") {e}"))"#),
    ];

    for (case, src) in cases {
        assert_eq!(verdicts(src), [Verdict::Flattened], "{case}: an error handed over went unseen");
    }
}

#[test]
fn a_message_holding_no_error_passes() {
    let cases = [
        ("bare literal", r#"AppError::validation_msg("Path is not a directory")"#),
        ("another value", r#"AppError::validation_msg(format!("Unsupported locale: {code}"))"#),
        ("name starting with e", r#"AppError::updater_msg(format!("timed out: {elapsed:?}"))"#),
        ("plural", r#"AppError::scanner_msg(format!("{errors} files failed"))"#),
        ("counter", r#"AppError::scanner_msg(format!("{err_count} files failed"))"#),
        ("escaped braces", r#"AppError::validation_msg(format!("literal {{e}} in {path}"))"#),
        ("string arg", r#"AppError::validation_msg(format!("{}", "e"))"#),
        ("method on a value", r#"AppError::validation_msg(format!("{} is bad", path.display()))"#),
        ("macro variable", r#"AppError::queue_msg(format!("initial {} fetch", $label))"#),
    ];

    for (case, src) in cases {
        assert_eq!(verdicts(src), [Verdict::Clean], "{case}: flagged with no error in it");
    }
}

#[test]
fn a_call_that_is_not_message_only_is_not_read() {
    let cases = [
        ("cause constructor", r#"AppError::validation("bad path", e)"#),
        ("io with a cause", r#"AppError::io("open failed", e)"#),
        ("io_source", "AppError::io_source(e)"),
        ("another type", r#"MyAppError::validation_msg(format!("bad path: {e}"))"#),
        ("a variant literal", "AppError::NotFound { msg }"),
        ("point-free", ".map_err(AppError::queue_msg)"),
    ];

    for (case, src) in cases {
        assert!(verdicts(src).is_empty(), "{case}: read as a message-only call");
    }
}

#[test]
fn a_call_spread_over_several_lines_is_read_whole() {
    let src = "AppError::validation_msg(format!(\n    \"{} is not a folder: {}\",\n    \
               path.display(),\n    e\n))";

    assert_eq!(verdicts(src), [Verdict::Flattened]);
}

#[test]
fn each_call_in_a_source_gets_its_own_verdict() {
    let src = r#"
        return Err(AppError::validation_msg("Path is not a directory"));
        return Err(AppError::validation_msg(format!("bad path: {e}")));
        return Err(AppError::validation("Cannot resolve the path", e));
        return Err(AppError::queue_msg("No valid tracks provided"));
    "#;

    assert_eq!(verdicts(src), [Verdict::Clean, Verdict::Flattened, Verdict::Clean]);
}

#[test]
fn a_finding_names_the_line_its_call_starts_on() {
    let src = "fn f() {\n    other();\n    Err(AppError::queue_msg(\n        \
               format!(\"restore failed: {e}\")\n    ))\n}";

    let lines: Vec<usize> = message_only_calls(src).iter().map(|call| call.line).collect();

    assert_eq!(lines, [3]);
}

#[test]
fn an_argument_list_that_never_closes_is_reported_rather_than_passed() {
    let cases = [
        ("unclosed", r#"AppError::validation_msg(format!("bad path: {e}")"#),
        ("unterminated literal", r#"AppError::validation_msg("bad path: {e})"#),
    ];

    for (case, src) in cases {
        assert_eq!(verdicts(src), [Verdict::Unclosed], "{case}: an unclosed call passed");
    }
}

fn verdicts(src: &str) -> Vec<Verdict> {
    message_only_calls(src).iter().map(|call| call.verdict).collect()
}

/// Every message-only `AppError` call in `code`, under the verdict it got.
fn message_only_calls(code: &str) -> Vec<MessageOnlyCall> {
    message_only_arguments(code)
        .into_iter()
        .map(|(start, arguments)| {
            let verdict = match arguments {
                None => Verdict::Unclosed,
                Some(arguments) if names_an_error(arguments) => Verdict::Flattened,
                Some(_) => Verdict::Clean,
            };
            MessageOnlyCall { line: line_of(code, start), verdict }
        })
        .collect()
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
