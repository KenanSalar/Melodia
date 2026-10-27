// Shared by `build.rs`, which `include!`s it, and the crate, so the build that bakes fingerprints
// and the run that looks them up can't drift: a drift has no symptom, every lookup just stops
// matching. Nothing here may name `crate::` or a dependency the build script lacks, and no `//!`:
// an inner attribute won't survive expansion inside the script's wrapper module.

/// Changing it invalidates every baked fingerprint, harmlessly since each build rebakes them.
pub const KEY_CONTEXT: &str = "melodia.radio.blocklist.v1";

/// For a source with no `key:` line, where the terms are empty or already as exposed as they can
/// be, so a domain separator rather than a secret.
pub const DEFAULT_KEY: [u8; 32] = [0; 32];

/// A C0 separator, so no normalized value can contain one and forge another axis.
const AXIS_SEPARATOR: u8 = 0x1f;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TermKind {
    Country,
    Language,
    Tag,
    Codec,
    Station,
    Name,
    Url,
    /// The substring axes, matched by hashing windows of the candidate instead.
    TagContains,
    NameContains,
    UrlContains,
}

impl TermKind {
    /// The source-file label, also hashed ahead of the value so one spelling can't block two axes.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Country => "country",
            Self::Language => "language",
            Self::Tag => "tag",
            Self::Codec => "codec",
            Self::Station => "station",
            Self::Name => "name",
            Self::Url => "url",
            Self::TagContains => "tag-contains",
            Self::NameContains => "name-contains",
            Self::UrlContains => "url-contains",
        }
    }

    pub const fn is_pattern(self) -> bool {
        matches!(self, Self::TagContains | Self::NameContains | Self::UrlContains)
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
            Self::TagContains,
            Self::NameContains,
            Self::UrlContains,
        ]
        .into_iter()
        .find(|kind| kind.label() == label)
    }
}

pub struct Terms {
    pub key: [u8; 32],
    /// Sorted and deduplicated, for a binary search.
    pub fingerprints: Vec<u64>,
    /// The substring axes, kept apart because a lookup asks of them whether any window matches.
    pub patterns: Vec<u64>,
    /// Ascending distinct pattern lengths, which size the windows and so have to ship: a far
    /// smaller disclosure than the text they measure.
    pub pattern_lengths: Vec<u32>,
}

/// The hashing key from a source's `key:` line, or [`DEFAULT_KEY`] without one.
pub fn key_from(material: Option<&str>) -> [u8; 32] {
    match material {
        Some(material) => blake3::derive_key(KEY_CONTEXT, material.as_bytes()),
        None => DEFAULT_KEY,
    }
}

/// Truncated to 64 bits, which is ample: a collision needs one of a few hundred terms to meet one
/// of tens of thousands of values, and costs one station hidden.
pub fn fingerprint(key: &[u8; 32], kind: TermKind, value: &str) -> u64 {
    fingerprint_normalized(key, kind, &normalize(value))
}

/// For the window scan, which normalizes once rather than per window.
pub fn fingerprint_normalized(key: &[u8; 32], kind: TermKind, normalized: &str) -> u64 {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(kind.label().as_bytes());
    hasher.update(&[AXIS_SEPARATOR]);
    hasher.update(normalized.as_bytes());

    let digest = hasher.finalize();
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest.as_bytes()[..8]);
    u64::from_le_bytes(head)
}

/// Lowercased with whitespace runs collapsed, the two things the directory is inconsistent about.
/// Applied to every axis, one normalization being the only kind anyone can reason about.
pub fn normalize(value: &str) -> String {
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

/// Why a line was refused. **Fieldless on purpose**: a refusal reaches a public CI log, so the
/// type has nowhere to put the text it was protecting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// Not `kind: value`.
    Shape,
    InlineComment,
    UnknownKind,
    EmptyValue,
    CountryCode,
    ShortPattern,
    SecondKey,
    EmptyKey,
    /// Not `field = value`, in a pre-hashed source.
    HashedShape,
    UnknownField,
    NotANumber,
    KeyDigits,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape => f.write_str("expected `kind: value`"),
            Self::InlineComment => f.write_str("comments go on their own line"),
            Self::UnknownKind => f.write_str("unknown kind"),
            Self::EmptyValue => f.write_str("empty value"),
            Self::CountryCode => f.write_str("country wants a two-letter ISO 3166-1 code"),
            Self::ShortPattern => {
                write!(f, "a contains pattern needs at least {MIN_PATTERN_CHARS} characters")
            }
            Self::SecondKey => f.write_str("a second key line"),
            Self::EmptyKey => f.write_str("empty key"),
            Self::HashedShape => f.write_str("expected `field = value`"),
            Self::UnknownField => f.write_str("unknown field"),
            Self::NotANumber => f.write_str("not a number"),
            Self::KeyDigits => f.write_str("key wants 64 hex characters"),
        }
    }
}

/// A [`Refusal`] and the line it happened on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParseError {
    Line {
        number: usize,
        refusal: Refusal,
    },
    /// A pre-hashed source missing a field names no line to blame.
    MissingField,
}

impl ParseError {
    const fn at(number: usize, refusal: Refusal) -> Self {
        Self::Line { number, refusal }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Line { number, refusal } => write!(f, "line {number}: {refusal}"),
            Self::MissingField => f.write_str("pre-hashed source is missing a field"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses a source into the key and the fingerprints it names.
///
/// # Errors
///
/// Every malformed line is refused rather than skipped, a dropped entry unblocking a station
/// silently.
pub fn parse_source(text: &str) -> Result<Terms, ParseError> {
    let key = key_from(key_material(text)?);

    let mut fingerprints = Vec::new();
    let mut patterns = Vec::new();
    let mut pattern_lengths = Vec::new();

    for (number, line) in entries(text) {
        let (label, value) = split_entry(line, number)?;
        if label == KEY_LABEL {
            continue;
        }
        let Some(kind) = TermKind::from_label(label) else {
            return Err(ParseError::at(number, Refusal::UnknownKind));
        };
        if value.is_empty() {
            return Err(ParseError::at(number, Refusal::EmptyValue));
        }
        if kind == TermKind::Country && !is_country_code(value) {
            return Err(ParseError::at(number, Refusal::CountryCode));
        }

        if !kind.is_pattern() {
            fingerprints.push(fingerprint(&key, kind, value));
            continue;
        }

        // A pattern's length in characters sizes its windows.
        let normalized = normalize(value);
        let length = normalized.chars().count();
        if length < MIN_PATTERN_CHARS {
            return Err(ParseError::at(number, Refusal::ShortPattern));
        }
        patterns.push(fingerprint_normalized(&key, kind, &normalized));
        pattern_lengths.push(u32::try_from(length).unwrap_or(u32::MAX));
    }

    fingerprints.sort_unstable();
    fingerprints.dedup();
    patterns.sort_unstable();
    patterns.dedup();
    // Ascending, so the scan can stop at the first length the candidate is too short for.
    pattern_lengths.sort_unstable();
    pattern_lengths.dedup();

    Ok(Terms { key, fingerprints, patterns, pattern_lengths })
}

/// First line of a pre-hashed source, telling the two shapes apart. Versioned because something
/// outside this repo holds one.
pub const HASHED_MARKER: &str = "melodia-blocklist-hashed-v1";

/// Parses either shape, dispatching on [`HASHED_MARKER`].
pub fn parse_any(text: &str) -> Result<Terms, ParseError> {
    if entries(text).next().is_some_and(|(_, line)| line == HASHED_MARKER) {
        return parse_hashed(text);
    }
    parse_source(text)
}

/// Terms as a pre-hashed source, which is what CI is handed: it carries exactly what a released
/// binary does, so the plaintext never leaves the machine that wrote it.
pub fn render_hashed(terms: &Terms) -> String {
    let key = hex(&terms.key);
    let numbers = |values: &[u64]| values.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    let fingerprints = numbers(&terms.fingerprints);
    let patterns = numbers(&terms.patterns);
    let lengths = terms.pattern_lengths.iter().map(u32::to_string).collect::<Vec<_>>().join(",");

    format!(
        "{HASHED_MARKER}\n\
         key = {key}\n\
         terms = {fingerprints}\n\
         patterns = {patterns}\n\
         lengths = {lengths}\n"
    )
}

/// Reads back [`render_hashed`]. `field = value` rather than `kind: value`, so a file fed to the
/// wrong parser fails on its first line.
pub fn parse_hashed(text: &str) -> Result<Terms, ParseError> {
    let (mut key, mut fingerprints, mut patterns, mut pattern_lengths) = (None, None, None, None);

    for (number, line) in entries(text) {
        if line == HASHED_MARKER {
            continue;
        }
        let Some((field, value)) = line.split_once('=') else {
            return Err(ParseError::at(number, Refusal::HashedShape));
        };
        let value = value.trim();
        match field.trim() {
            "key" => key = Some(hex_key(value, number)?),
            "terms" => fingerprints = Some(numbers(value, number)?),
            "patterns" => patterns = Some(numbers(value, number)?),
            "lengths" => pattern_lengths = Some(numbers::<u32>(value, number)?),
            _ => return Err(ParseError::at(number, Refusal::UnknownField)),
        }
    }

    let mut terms = Terms {
        key: key.ok_or(ParseError::MissingField)?,
        fingerprints: fingerprints.ok_or(ParseError::MissingField)?,
        patterns: patterns.ok_or(ParseError::MissingField)?,
        pattern_lengths: pattern_lengths.ok_or(ParseError::MissingField)?,
    };

    // Sorted rather than trusted: an out-of-order list would miss silently.
    terms.fingerprints.sort_unstable();
    terms.patterns.sort_unstable();
    terms.pattern_lengths.sort_unstable();
    Ok(terms)
}

fn numbers<T: std::str::FromStr>(value: &str, number: usize) -> Result<Vec<T>, ParseError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let not_a_number = ParseError::at(number, Refusal::NotANumber);
    value.split(',').map(|item| item.trim().parse::<T>().map_err(|_| not_a_number)).collect()
}

/// By hand, `format_collect` and `format_push_string` denying both idiomatic spellings.
fn hex(key: &[u8; 32]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(key.len() * 2);
    for byte in key {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn hex_key(value: &str, number: usize) -> Result<[u8; 32], ParseError> {
    let invalid = ParseError::at(number, Refusal::KeyDigits);
    if value.len() != 64 {
        return Err(invalid);
    }
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        let pair = value.get(index * 2..index * 2 + 2).ok_or(invalid)?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| invalid)?;
    }
    Ok(key)
}

/// A shorter substring is in nearly every tag, so a typo would blank most of the directory and
/// turn every position into a window.
pub const MIN_PATTERN_CHARS: usize = 3;

/// The label reserved for the hashing key rather than naming an axis.
const KEY_LABEL: &str = "key";

/// The `key:` line's value, in a pass of its own because a key may sit below the terms it
/// applies to.
fn key_material(text: &str) -> Result<Option<&str>, ParseError> {
    let mut material = None;
    for (number, line) in entries(text) {
        let (label, value) = split_entry(line, number)?;
        if label != KEY_LABEL {
            continue;
        }
        if material.is_some() {
            return Err(ParseError::at(number, Refusal::SecondKey));
        }
        if value.is_empty() {
            return Err(ParseError::at(number, Refusal::EmptyKey));
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

/// Comments are whole-line only and a `#` in a value is refused: trimming from ` #` would
/// truncate a name or URL into a term that matches nothing. Such a station is reachable by its
/// uuid or stream URL instead.
fn split_entry(line: &str, number: usize) -> Result<(&str, &str), ParseError> {
    let Some((label, value)) = line.split_once(':') else {
        return Err(ParseError::at(number, Refusal::Shape));
    };
    let value = value.trim();
    if value.contains('#') {
        return Err(ParseError::at(number, Refusal::InlineComment));
    }
    Ok((label.trim(), value))
}

fn is_country_code(value: &str) -> bool {
    value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_alphabetic())
}
