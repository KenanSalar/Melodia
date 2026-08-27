// The half `build.rs` and the crate share, `include!`d verbatim by the former and
// declared as a module by the latter. Nothing here may name `crate::` or pull a
// dependency the build script doesn't have, and it deliberately carries no `//!`
// docs — an inner attribute would have to survive being expanded inside the build
// script's wrapper module. The module docs live in `mod.rs`.
//
// One file rather than two so the normalization, the key derivation and the hash
// cannot drift apart between the build that bakes the fingerprints and the run
// that looks them up. That drift has no symptom: every lookup would simply stop
// matching, and nothing anywhere would report it.

/// The context string [`key_from`] derives under. Fixed and never reused — changing
/// it invalidates every baked fingerprint, which is harmless (they are regenerated
/// from the source on each build) but pointless.
pub const KEY_CONTEXT: &str = "melodia.radio.blocklist.v1";

/// The key a source with no `key:` line hashes under.
///
/// Only reachable when there is no source at all or the author left the key out, and
/// in both cases the term list is either empty or already as exposed as it can be —
/// so this is a domain separator, not a secret.
pub const DEFAULT_KEY: [u8; 32] = [0; 32];

/// Bytes between the axis tag and the value, chosen from the C0 separators so no
/// normalized value can contain one and forge a different axis.
const AXIS_SEPARATOR: u8 = 0x1f;

/// Which axis a blocked term names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TermKind {
    Country,
    Language,
    Tag,
    Codec,
    Station,
    Name,
    Url,
}

impl TermKind {
    /// The label this axis is spelled with in a source file, and the tag hashed
    /// ahead of the value so one spelling cannot block two axes.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Country => "country",
            Self::Language => "language",
            Self::Tag => "tag",
            Self::Codec => "codec",
            Self::Station => "station",
            Self::Name => "name",
            Self::Url => "url",
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        [
            Self::Country,
            Self::Language,
            Self::Tag,
            Self::Codec,
            Self::Station,
            Self::Name,
            Self::Url,
        ]
        .into_iter()
        .find(|kind| kind.label() == label)
    }
}

/// A parsed source: the key its terms were hashed under, and those terms.
pub struct Terms {
    pub key: [u8; 32],
    /// Sorted and deduplicated, which is what lets a lookup binary-search.
    pub fingerprints: Vec<u64>,
}

/// Derive the hashing key from a source's `key:` line, or [`DEFAULT_KEY`] without one.
pub fn key_from(material: Option<&str>) -> [u8; 32] {
    match material {
        Some(material) => blake3::derive_key(KEY_CONTEXT, material.as_bytes()),
        None => DEFAULT_KEY,
    }
}

/// The fingerprint one term hashes to.
///
/// Truncated to 64 bits, which is ample: a collision would have to land between one
/// of a few hundred blocked terms and one of the directory's few tens of thousands
/// of values, and the cost of one is a station hidden that shouldn't be.
pub fn fingerprint(key: &[u8; 32], kind: TermKind, value: &str) -> u64 {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(kind.label().as_bytes());
    hasher.update(&[AXIS_SEPARATOR]);
    hasher.update(normalize(value).as_bytes());

    let digest = hasher.finalize();
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(head)
}

/// Fold a value to the one spelling both sides of a comparison agree on.
///
/// Case and whitespace are the two things the directory is inconsistent about — its
/// tags are free-form and its station names are whatever an operator typed — so a
/// run of spaces collapses to one and everything lowercases. Applied to the
/// code-shaped axes too, where it changes nothing, because one normalization is the
/// only kind anybody can reason about.
fn normalize(value: &str) -> String {
    let mut folded = String::with_capacity(value.len());
    let mut break_pending = false;

    for character in value.trim().chars() {
        if character.is_whitespace() {
            break_pending = true;
            continue;
        }
        if break_pending && !folded.is_empty() {
            folded.push(' ');
        }
        break_pending = false;
        folded.extend(character.to_lowercase());
    }
    folded
}

/// Parse a blocklist source into the key and the fingerprints it names.
///
/// # Errors
///
/// Every malformed line is refused rather than skipped: a dropped entry unblocks a
/// station with nothing anywhere to notice. **No error carries the offending text**,
/// only its line number — these surface in a public CI log, and a message quoting
/// the line would hand over the entry it was protecting.
pub fn parse_source(text: &str) -> Result<Terms, String> {
    let key = key_from(key_material(text)?);

    let mut fingerprints = Vec::new();
    for (number, line) in entries(text) {
        let (label, value) = split_entry(line, number)?;
        if label == KEY_LABEL {
            continue;
        }
        let Some(kind) = TermKind::from_label(label) else {
            return Err(format!("line {number}: unknown kind"));
        };
        if value.is_empty() {
            return Err(format!("line {number}: empty value"));
        }
        if kind == TermKind::Country && !is_country_code(value) {
            return Err(format!("line {number}: country wants a two-letter ISO 3166-1 code"));
        }
        fingerprints.push(fingerprint(&key, kind, value));
    }

    fingerprints.sort_unstable();
    fingerprints.dedup();
    Ok(Terms { key, fingerprints })
}

/// The label reserved for the hashing key rather than naming an axis.
const KEY_LABEL: &str = "key";

/// The `key:` line's value, refusing a second one.
///
/// Its own pass because a key may legally sit below the terms it applies to, and a
/// term hashed under the default before the real key was read would be a fingerprint
/// nothing ever matches.
fn key_material(text: &str) -> Result<Option<&str>, String> {
    let mut material = None;
    for (number, line) in entries(text) {
        let (label, value) = split_entry(line, number)?;
        if label != KEY_LABEL {
            continue;
        }
        if material.is_some() {
            return Err(format!("line {number}: a second key line"));
        }
        if value.is_empty() {
            return Err(format!("line {number}: empty key"));
        }
        material = Some(value);
    }
    Ok(material)
}

/// The lines that say something, with their 1-based numbers for error reporting.
fn entries(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

/// Split one entry into its label and its value.
///
/// **Comments are whole-line only, and a `#` anywhere in a value is refused.** The
/// alternative, trimming from the first ` #`, silently truncates a name or a URL
/// that legitimately contains one, and a truncated term matches nothing — the exact
/// quiet failure this parser exists to prevent. A station whose name needs a `#` is
/// reachable by its uuid or its stream URL instead.
fn split_entry(line: &str, number: usize) -> Result<(&str, &str), String> {
    let Some((label, value)) = line.split_once(':') else {
        return Err(format!("line {number}: expected `kind: value`"));
    };
    let value = value.trim();
    if value.contains('#') {
        return Err(format!("line {number}: comments go on their own line"));
    }
    Ok((label.trim(), value))
}

fn is_country_code(value: &str) -> bool {
    value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_alphabetic())
}
