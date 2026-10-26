//! Source read as text: comments dropped, whitespace collapsed, braces matched outside string
//! literals.

/// Brace-depth walk over `src[from..to]`, quote-aware, returning the **lowest** depth the
/// range reaches, or `None` where it closes a scope it never opened.
///
/// The lowest rather than the last, because `crates/melodia/tests/index_persist.rs`' `inside_block`
/// is asking whether a scope was ever left: a write hoisted out of the ordering closure and into
/// any block after it ends the range back above zero and reads as though it never moved.
///
/// `crates/melodia/tests/scrollbars.rs`' walk asks the other way round: that one lifts a block's
/// body, this one asks whether two offsets share one. Comments are stripped by the caller for
/// the same reason the quotes are handled — a brace inside either unbalances the count.
pub fn depth_between(src: &str, from: usize, to: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut floor = usize::MAX;
    for (_, byte) in outside_strings(src.get(from..to)?.as_bytes()) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                floor = floor.min(depth);
            }
            _ => {}
        }
    }
    Some(floor.min(depth))
}

/// Whether byte `at` of a block's `body` sits at the body's own depth rather than inside a child,
/// for a pin on what the block itself binds or handles. From inside a child the walk to the end
/// closes a scope it never opened.
pub fn at_own_depth(body: &str, at: usize) -> bool {
    depth_between(body, at, body.len()) == Some(0)
}

/// `src` with everything after an unquoted `//` dropped on each line, keeping the line
/// structure.
///
/// Prose about the code reads exactly like the code to any pin grepping for a construct: the
/// translation pin would collect a msgid off an `@tr("…")` inside a comment, and the
/// scrollbar pin's brace walk is thrown by any comment quoting an unbalanced `{`.
pub fn strip_line_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let bytes = line.as_bytes();
        let cut = outside_strings(bytes)
            .find(|&(at, byte)| byte == b'/' && bytes.get(at + 1) == Some(&b'/'))
            .map_or(line.len(), |(at, _)| at);
        out.push_str(&line[..cut]);
        out.push('\n');
    }
    out
}

/// Whether `name` is how the tree spells a binding holding an error. One definition for every
/// walk that reads for one, so two walks can't disagree about which names count.
pub fn is_error_name(name: &str) -> bool {
    matches!(name, "e" | "err" | "error") || name.ends_with("_err") || name.ends_with("_error")
}

/// The identifiers a format string captures inline, `{name}` or `{name:spec}`.
pub fn inline_names(literal: &str) -> Vec<&str> {
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

/// The body of the block whose `{` sits at `open`, braces excluded.
///
/// Quote-aware, and pair it with [`strip_line_comments`] — an unbalanced `{` throws the count
/// whether it sits in a string or in prose. A pin that greps flat instead lets a nested block
/// answer for its parent.
pub fn block_body(src: &str, open: usize) -> Option<&str> {
    let mut depth = 0usize;
    for (at, byte) in outside_strings(src.as_bytes().get(open..)?) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&src[open + 1..open + at]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The body of the first block opening after `needle`, braces excluded, for a pin on one named
/// function, arm or global. `""` when the needle or its block is missing,
/// [`binding_value`](crate::binding_value)'s convention: no pin expects an empty block, so the
/// caller asserts that apart as a broken walk.
pub fn block_after<'a>(src: &'a str, needle: &str) -> &'a str {
    src.find(needle)
        .and_then(|at| src[at..].find('{').map(|rel| at + rel))
        .and_then(|open| block_body(src, open))
        .unwrap_or_default()
}

/// The `{` of every block open at byte `at` of `src`, outermost first. Pair it with
/// [`strip_line_comments`] for [`block_body`]'s reason.
pub fn open_braces(src: &str, at: usize) -> Vec<usize> {
    let bytes = src.as_bytes();
    let mut open = Vec::new();
    for (offset, byte) in outside_strings(&bytes[..at.min(bytes.len())]) {
        match byte {
            b'{' => open.push(offset),
            b'}' => {
                open.pop();
            }
            _ => {}
        }
    }
    open
}

/// Runs of whitespace collapsed to one space, so a pin reads a token sequence rather than one
/// file's indentation. Pair it with [`strip_line_comments`] — this joins lines, so a trailing
/// comment would otherwise run into the code after it.
pub fn normalize_ws(src: &str) -> String {
    src.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `src` as a token sequence with its comments gone: [`strip_line_comments`] then
/// [`normalize_ws`], the pairing each of them asks for.
pub fn code_tokens(src: &str) -> String {
    normalize_ws(&strip_line_comments(src))
}

/// Every byte of `bytes` outside a `"…"` string literal, with its offset, the quotes left out.
/// Inside a string a `\` escapes the byte after it.
///
/// The one place a walk decides what is quoted, so a brace or a `//` inside a string never
/// reads as structure. Bytes rather than chars because continuation bytes are all `>= 0x80`,
/// so none can pass for a brace or a quote. Char literals and raw strings aren't recognised:
/// a `'"'`, or an `r#"…"#` holding a quote, throws it.
pub fn outside_strings(bytes: &[u8]) -> impl Iterator<Item = (usize, u8)> {
    let mut in_string = false;
    let mut next = 0;
    std::iter::from_fn(move || {
        while let Some(&byte) = bytes.get(next) {
            let at = next;
            next += 1;
            match byte {
                b'\\' if in_string => next += 1,
                b'"' => in_string = !in_string,
                _ if !in_string => return Some((at, byte)),
                _ => {}
            }
        }
        None
    })
}
