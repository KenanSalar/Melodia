//! Reads `AutoEq`'s recommended list, one `- [<name>](./<path>)` line per model.

use melodia_core::entities::correction::AutoEqEntry;
use percent_encoding::percent_decode_str;

const ENTRY_OPEN: &str = "- [";

const LINK_OPEN: &str = "](./";

/// A source, then the model; the rig between is optional.
const MIN_SEGMENTS: usize = 2;

/// Every entry the list holds. A line shaped like one that doesn't read is left out, and the count
/// logged with the first, which shows how the layout moved.
pub fn parse_list(text: &str) -> Vec<AutoEqEntry> {
    let mut entries = Vec::new();
    let mut unread = 0usize;
    let mut first_unread = None;
    for line in text.lines().map(str::trim_end).filter(|line| line.starts_with(ENTRY_OPEN)) {
        if let Some(entry) = parse_entry(line) {
            entries.push(entry);
        } else {
            unread += 1;
            first_unread.get_or_insert(line);
        }
    }
    if let Some(first) = first_unread {
        log::debug!("AutoEq list: {unread} entry lines unread, the first {first:?}");
    }
    entries
}

/// The link runs to the line's last `)`, since names and paths both carry parentheses.
fn parse_entry(line: &str) -> Option<AutoEqEntry> {
    let (name, link) = line.strip_prefix(ENTRY_OPEN)?.split_once(LINK_OPEN)?;
    let segments =
        link.strip_suffix(')')?.split('/').map(decoded_segment).collect::<Option<Vec<String>>>()?;
    if segments.len() < MIN_SEGMENTS {
        return None;
    }
    Some(AutoEqEntry {
        name: name.into(),
        source: segments[0].as_str().into(),
        path: segments.join("/").into(),
    })
}

/// Refuses what would move the URL built from it: an empty, `.` or `..` segment, or one decoding
/// to a `/`.
fn decoded_segment(raw: &str) -> Option<String> {
    let segment = percent_decode_str(raw).decode_utf8().ok()?;
    let movable = matches!(&*segment, "" | "." | "..") || segment.contains('/');
    (!movable).then(|| segment.into_owned())
}
