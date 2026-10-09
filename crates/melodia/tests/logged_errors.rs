//! Every error a `log::` call prints goes through `error::describe`.
//!
//! The four I/O-boundary `AppError` variants print their operation and keep the reason on
//! `.source()`, so a bare `{e}` reports a full disk and a permissions failure in the same words,
//! and any other error loses the chain below it. Nothing fails when the rule is broken, and it can
//! be broken from any file, so every crate is walked for it.
//!
//! The reader below is a small lexer rather than a substring search: a log call spans lines,
//! nests parentheses and sits beside strings, chars and comments that only look like one, and a
//! reader that loses its place either flags code that is fine or passes code that is not.

use melodia_testkit::raw_rust_sources;

/// Loose on purpose: below it the reader has stopped finding calls, not the tree making them.
const MIN_LOG_CALLS: usize = 300;

const LOG_PATH: &[u8] = b"log::";

const LOG_MACROS: [&str; 6] = ["error", "warn", "info", "debug", "trace", "log"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Prints no error, or hands every one it prints to `describe`.
    Clean,
    /// Prints an error through its own `Display` or `Debug`.
    Bare,
    /// A call the reader could not take apart, which it reports rather than passes.
    Unreadable,
}

struct LogCall {
    line: usize,
    verdict: Verdict,
}

/// The tree's log calls, each named `path:line` under the verdict it got.
struct Walk {
    bare: Vec<String>,
    unreadable: Vec<String>,
    read: usize,
}

fn walk_tree() -> Walk {
    let mut walk = Walk { bare: Vec::new(), unreadable: Vec::new(), read: 0 };
    // Raw: the lexer skips comments itself, and the shared stripper can cut a literal short.
    for (path, src) in raw_rust_sources() {
        for call in log_calls(&src) {
            let site = format!("{path}:{}", call.line);
            match call.verdict {
                Verdict::Clean => walk.read += 1,
                Verdict::Bare => {
                    walk.read += 1;
                    walk.bare.push(site);
                }
                Verdict::Unreadable => walk.unreadable.push(site),
            }
        }
    }
    walk
}

#[test]
fn no_log_call_prints_an_error_without_describe() {
    let walk = walk_tree();

    assert!(
        walk.bare.is_empty(),
        "{:?} print an error through its own `Display` or `Debug`. Hand it to \
         `error::describe` as `{{}}` plus `describe(&e)`, or rename a binding that is not an \
         error for what it is",
        walk.bare
    );
}

/// A call the reader cannot take apart is one it cannot vouch for, so it fails the walk instead
/// of being skipped, which is the walk's own version of an unreadable path.
#[test]
fn every_log_call_in_the_tree_is_read() {
    let walk = walk_tree();

    assert!(
        walk.unreadable.is_empty(),
        "{:?} could not be read: an unclosed call, or a format string that is not a literal. \
         Spell it as a literal, or teach the reader the new shape",
        walk.unreadable
    );
}

#[test]
fn the_walk_finds_the_trees_log_calls() {
    let walk = walk_tree();

    assert!(
        walk.read >= MIN_LOG_CALLS,
        "only {} log calls read, expected at least {MIN_LOG_CALLS}: the reader has stopped \
         recognising calls, and a walk that finds none passes everything",
        walk.read
    );
}

#[test]
fn an_error_printed_bare_is_caught() {
    let cases = [
        ("inline", r#"log::warn!("save failed: {e}");"#),
        ("inline err", r#"log::warn!("save failed: {err}");"#),
        ("inline error", r#"log::warn!("save failed: {error}");"#),
        ("suffix _err", r#"log::warn!("rollback failed: {restore_err}");"#),
        ("suffix _error", r#"log::warn!("rollback failed: {last_error}");"#),
        ("debug spec", r#"log::warn!("watch error: {e:?}");"#),
        ("alternate spec", r#"log::warn!("watch error: {err:#}");"#),
        ("positional", r#"log::warn!("save failed: {}", e);"#),
        ("positional borrow", r#"log::warn!("save failed: {}", &e);"#),
        ("positional deref", r#"log::warn!("save failed: {}", &*err);"#),
        ("to_string", r#"log::warn!("save failed: {}", e.to_string());"#),
        ("named arg", r#"log::warn!("save failed: {why}", why = e);"#),
        ("beside a describe", r#"log::warn!("{}: {e}", describe(&e));"#),
        ("after other args", r#"log::warn!("{} of {}: {}", done, total, e);"#),
        ("paren in literal", r#"log::warn!("closed (shutdown?): {e}");"#),
        ("escaped quote", r#"log::warn!("\"{name}\" failed: {e}");"#),
        ("target", r#"log::warn!(target: "audio", "open failed: {e}");"#),
        ("log! with level", r#"log::log!(Level::Warn, "open failed: {e}");"#),
        ("raw literal", r#"log::warn!(r"save failed: {e}");"#),
        ("hashed raw literal", r##"log::warn!(r#"save "x" failed: {e}"#);"##),
        ("error level", r#"log::error!("save failed: {e}");"#),
        ("info level", r#"log::info!("save failed: {e}");"#),
        ("debug level", r#"log::debug!("save failed: {e}");"#),
        ("trace level", r#"log::trace!("save failed: {e}");"#),
        ("absolute path", r#"::log::warn!("save failed: {e}");"#),
        ("space before paren", r#"log::warn! ("save failed: {e}");"#),
        ("char literal before", r#"let q = '"'; log::warn!("save failed: {e}");"#),
        ("lifetime before", r#"fn f<'a>(e: &'a E) { log::warn!("save failed: {e}") }"#),
    ];

    for (case, src) in cases {
        assert_eq!(verdicts(src), [Verdict::Bare], "{case}: an error printed bare went unseen");
    }
}

#[test]
fn an_error_handed_to_describe_passes() {
    let cases = [
        ("imported", r#"log::warn!("save failed: {}", describe(&e));"#),
        ("full path", r#"log::warn!("save failed: {}", melodia_core::error::describe(&e));"#),
        ("by reference", r#"log::warn!("save failed: {}", describe(e));"#),
        ("boxed", r#"log::warn!("save failed: {}", describe(&*e));"#),
        ("among other args", r#"log::warn!("{} failed: {}", path.display(), describe(&e));"#),
        ("named describe", r#"log::warn!("save failed: {e}", e = describe(&err));"#),
    ];

    for (case, src) in cases {
        assert_eq!(verdicts(src), [Verdict::Clean], "{case}: a described error was flagged");
    }
}

#[test]
fn a_value_that_is_not_an_error_passes() {
    let cases = [
        ("no args", r#"log::info!("scan started");"#),
        ("reason", r#"log::warn!("restart: staying up: {reason}");"#),
        ("name starting with e", r#"log::info!("took {elapsed:?}");"#),
        ("plural", r#"log::info!("{errors} errors");"#),
        ("counter", r#"log::info!("{err_count} skipped");"#),
        ("escaped braces", r#"log::info!("literal {{e}} kept");"#),
        ("string arg", r#"log::info!("{}", "e");"#),
        ("a field of the error", r#"log::debug!("kind: {:?}", e.kind());"#),
        ("macro variable", r#"log::warn!("initial {} fetch", $label);"#),
    ];

    for (case, src) in cases {
        assert_eq!(verdicts(src), [Verdict::Clean], "{case}: a value that is no error was flagged");
    }
}

#[test]
fn text_that_only_looks_like_a_log_call_is_not_read() {
    let cases = [
        ("another crate's macro", r#"dialog::warn!("save failed: {e}");"#),
        ("a log item, not a macro", "let filter = log::LevelFilter::Warn;"),
        ("an unlisted log macro", "if log::log_enabled!(Level::Debug) {}"),
        ("inside a string", r#"let needle = "log::warn!(\"{e}\")";"#),
        ("inside a raw string", r##"let needle = r#"log::warn!("{e}")"#;"##),
        ("inside a line comment", r#"// log::warn!("{e}");"#),
        ("inside a block comment", r#"/* log::warn!("{e}"); */"#),
        ("inside a nested block comment", r#"/* outer /* inner */ log::warn!("{e}"); */"#),
        ("user-facing text", r#"toast::notify(kind, format!("save failed: {e}"));"#),
    ];

    for (case, src) in cases {
        assert!(verdicts(src).is_empty(), "{case}: read as a log call");
    }
}

#[test]
fn a_call_spread_over_several_lines_is_read_whole() {
    let src = "log::warn!(\n    \"respawn failed for {}: {}\",\n    exe.display(),\n    e\n);";

    assert_eq!(verdicts(src), [Verdict::Bare]);
}

#[test]
fn each_call_in_a_source_gets_its_own_verdict() {
    let src = r#"
        log::info!("scan started");
        log::warn!("scan failed: {e}");
        log::warn!("scan failed: {}", describe(&e));
    "#;

    assert_eq!(verdicts(src), [Verdict::Clean, Verdict::Bare, Verdict::Clean]);
}

#[test]
fn a_finding_names_the_line_its_call_starts_on() {
    let src = "fn f() {\n    other();\n    log::warn!(\n        \"save failed: {e}\"\n    );\n}";

    let lines: Vec<usize> = log_calls(src).iter().map(|call| call.line).collect();

    assert_eq!(lines, [3]);
}

#[test]
fn a_call_the_walk_cannot_read_is_reported_rather_than_passed() {
    let cases = [
        ("unclosed", r#"log::warn!("save failed: {e}""#),
        ("no literal", r#"log::warn!(concat!("save failed: ", "{}"), e);"#),
        ("square brackets", r#"log::warn!["save failed: {e}"];"#),
        ("braces", r#"log::warn! {"save failed: {e}"}"#),
    ];

    for (case, src) in cases {
        assert_eq!(verdicts(src), [Verdict::Unreadable], "{case}: an unreadable call passed");
    }
}

fn verdicts(src: &str) -> Vec<Verdict> {
    log_calls(src).iter().map(|call| call.verdict).collect()
}

fn log_calls(src: &str) -> Vec<LogCall> {
    log_call_starts(src)
        .into_iter()
        .filter_map(|start| {
            let verdict = match invocation(src, start)? {
                Delimiter::Paren(open) => {
                    split_args(src, open).map_or(Verdict::Unreadable, |args| judge(&args))
                }
                Delimiter::Other => Verdict::Unreadable,
            };
            Some(LogCall { line: line_of(src, start), verdict })
        })
        .collect()
}

/// Where each `log::` path starts in code, skipping strings, chars and comments, and paths that
/// merely end in `log` (`dialog::`).
fn log_call_starts(src: &str) -> Vec<usize> {
    let bytes = src.as_bytes();
    let mut starts = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if let Some(end) = skipped_end(src, at) {
            at = end;
            continue;
        }
        if bytes[at..].starts_with(LOG_PATH) && !identifier_before(bytes, at) {
            starts.push(at);
        }
        at += 1;
    }
    starts
}

/// How a log macro's arguments open.
enum Delimiter {
    /// Just past the `(`.
    Paren(usize),
    /// `[` or `{`, which the reader does not take apart.
    Other,
}

/// The delimiter of a `log::<level>!` invocation starting at `start`, or `None` where the path
/// names something else.
fn invocation(src: &str, start: usize) -> Option<Delimiter> {
    let rest = src.get(start + LOG_PATH.len()..)?;
    let name_len = rest.bytes().take_while(|&byte| continues_identifier(byte)).count();
    let (name, rest) = rest.split_at(name_len);
    if !LOG_MACROS.contains(&name) {
        return None;
    }
    let rest = rest.strip_prefix('!')?.trim_start();
    Some(match rest.strip_prefix('(') {
        Some(args) => Delimiter::Paren(src.len() - args.len()),
        None => Delimiter::Other,
    })
}

/// The call's top-level arguments, trimmed, or `None` when the call never closes.
fn split_args(src: &str, from: usize) -> Option<Vec<&str>> {
    let bytes = src.as_bytes();
    let mut args = Vec::new();
    let mut depth = 0_usize;
    let mut arg_start = from;
    let mut at = from;
    while at < bytes.len() {
        if let Some(end) = skipped_end(src, at) {
            at = end;
            continue;
        }
        match bytes[at] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' if depth == 0 => {
                push_arg(&mut args, &src[arg_start..at]);
                return Some(args);
            }
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                push_arg(&mut args, &src[arg_start..at]);
                arg_start = at + 1;
            }
            _ => {}
        }
        at += 1;
    }
    None
}

fn push_arg<'a>(args: &mut Vec<&'a str>, arg: &'a str) {
    let arg = arg.trim();
    if !arg.is_empty() {
        args.push(arg);
    }
}

fn judge(args: &[&str]) -> Verdict {
    let format = args.iter().enumerate().find_map(|(at, arg)| Some((at, literal_text(arg)?)));
    let Some((at, literal)) = format else {
        return Verdict::Unreadable;
    };

    let mut named = Vec::new();
    for arg in &args[at + 1..] {
        let (name, value) = split_named(arg);
        named.extend(name);
        if prints_bare(value) {
            return Verdict::Bare;
        }
    }

    let captures_an_error =
        inline_names(literal).into_iter().any(|name| !named.contains(&name) && is_error_name(name));
    if captures_an_error { Verdict::Bare } else { Verdict::Clean }
}

/// The text inside `arg` when it is a string literal and nothing else, raw or not.
fn literal_text(arg: &str) -> Option<&str> {
    if literal_end(arg, 0)? != arg.len() {
        return None;
    }
    let quoted = arg.strip_prefix('r').unwrap_or(arg).trim_matches('#');
    quoted.strip_prefix('"')?.strip_suffix('"')
}

/// `(Some(name), value)` for a `name = value` argument, `(None, arg)` for a positional one.
fn split_named(arg: &str) -> (Option<&str>, &str) {
    let name_len = arg.bytes().take_while(|&byte| continues_identifier(byte)).count();
    let (name, rest) = arg.split_at(name_len);
    match rest.trim_start().strip_prefix('=') {
        Some(value) if name_len > 0 && !value.starts_with(['=', '>']) => (Some(name), value.trim()),
        _ => (None, arg),
    }
}

/// Whether `expr` hands the formatter the error itself, which prints through its own `Display`.
fn prints_bare(expr: &str) -> bool {
    let value = expr.trim_start_matches(['&', '*']);
    let value = value.strip_suffix(".to_string()").unwrap_or(value);
    is_error_name(value)
}

fn is_error_name(name: &str) -> bool {
    matches!(name, "e" | "err" | "error") || name.ends_with("_err") || name.ends_with("_error")
}

/// The identifiers a format string captures inline, `{name}` or `{name:spec}`.
fn inline_names(literal: &str) -> Vec<&str> {
    let bytes = literal.as_bytes();
    let mut names = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'{' if bytes.get(at + 1) == Some(&b'{') => at += 2,
            b'{' => {
                let close = literal[at..].find('}').map_or(literal.len(), |offset| at + offset);
                let spec = &literal[at + 1..close];
                let name = spec.split(':').next().unwrap_or(spec).trim();
                if name.starts_with(|first: char| first.is_alphabetic() || first == '_') {
                    names.push(name);
                }
                at = close + 1;
            }
            _ => at += 1,
        }
    }
    names
}

/// Just past the comment or literal opening at `at`, or `None` where none opens there.
fn skipped_end(src: &str, at: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let rest = &bytes[at..];
    if rest.starts_with(b"//") {
        return Some(
            bytes[at..].iter().position(|&byte| byte == b'\n').map_or(bytes.len(), |n| at + n),
        );
    }
    if rest.starts_with(b"/*") {
        return Some(block_comment_end(bytes, at));
    }
    literal_end(src, at)
}

/// Just past the literal opening at `at`, or `None` where none opens there: a `'` that starts
/// a lifetime or a label rather than a char.
fn literal_end(src: &str, at: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    match bytes.get(at)? {
        b'"' => Some(quoted_end(bytes, at + 1)),
        b'\'' => char_end(src, at),
        b'r' if !identifier_before(bytes, at) || raw_byte_prefix(bytes, at) => {
            raw_end(bytes, at + 1)
        }
        _ => None,
    }
}

fn quoted_end(bytes: &[u8], from: usize) -> usize {
    let mut at = from;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// The end of a raw string whose `r` precedes `from`, or `None` where the `r` opens no string:
/// an identifier, or a raw identifier such as `r#type`.
fn raw_end(bytes: &[u8], from: usize) -> Option<usize> {
    let hashes = bytes[from..].iter().take_while(|&&byte| byte == b'#').count();
    let body = from + hashes;
    if bytes.get(body) != Some(&b'"') {
        return None;
    }
    let mut close = Vec::with_capacity(hashes + 1);
    close.push(b'"');
    close.resize(hashes + 1, b'#');
    let end = bytes[body + 1..].windows(close.len()).position(|window| window == close.as_slice());
    Some(end.map_or(bytes.len(), |offset| body + 1 + offset + close.len()))
}

/// The end of a char literal opening at `at`, or `None` where the `'` is a lifetime or a label.
fn char_end(src: &str, at: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    if bytes.get(at + 1) == Some(&b'\\') {
        let close = bytes.get(at + 3..)?.iter().position(|&byte| byte == b'\'')?;
        return Some(at + 3 + close + 1);
    }
    let quoted = src.get(at + 1..)?.chars().next()?;
    let close = at + 1 + quoted.len_utf8();
    (bytes.get(close) == Some(&b'\'')).then_some(close + 1)
}

fn block_comment_end(bytes: &[u8], at: usize) -> usize {
    let mut depth = 0_usize;
    let mut cursor = at;
    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"/*") {
            depth += 1;
            cursor += 2;
        } else if bytes[cursor..].starts_with(b"*/") {
            depth -= 1;
            cursor += 2;
            if depth == 0 {
                return cursor;
            }
        } else {
            cursor += 1;
        }
    }
    bytes.len()
}

/// A `br"…"` byte string, whose `r` follows a `b` that itself starts the token.
fn raw_byte_prefix(bytes: &[u8], at: usize) -> bool {
    at.checked_sub(1)
        .is_some_and(|prefix| bytes[prefix] == b'b' && !identifier_before(bytes, prefix))
}

fn identifier_before(bytes: &[u8], at: usize) -> bool {
    at.checked_sub(1).is_some_and(|previous| continues_identifier(bytes[previous]))
}

fn continues_identifier(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}
