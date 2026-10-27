//! How wide a line is set, estimated: measuring means asking Slint, and Rust decides each row's
//! height first so the offset table and the drawn rows agree. Being wrong costs wrap quality,
//! nothing structural.

/// The space, `i l t r f` and the thin punctuation, around half of a Latin line.
const NARROW_EMS: f32 = 0.32;

/// `m w M W @ %`, the only ASCII characters Vazirmatn sets near an em.
const WIDE_EMS: f32 = 0.90;

/// A capital, which runs a fifth wider than the lowercase it sits in.
pub(super) const UPPERCASE_EMS: f32 = 0.68;

/// Everything else drawn on a proportional em, letters of any script included.
const PROPORTIONAL_EMS: f32 = 0.57;

/// Where a lyric line stops being a line and starts being a paragraph. Past this the panel would
/// scroll more than it shows.
pub(super) const MAX_WRAPPED_LINES: u8 = 3;

/// Unicode's East Asian Wide and Fullwidth ranges a lyric sheet reaches. Counted proportionally,
/// a Korean line under-measures by nearly half, enough to elide words there was room for.
pub(super) fn is_full_width(ch: char) -> bool {
    matches!(u32::from(ch),
        0x1100..=0x115F         // Hangul jamo
        | 0x2E80..=0x303E       // CJK radicals and punctuation
        | 0x3041..=0x33FF       // kana, Hangul compatibility jamo, CJK compatibility
        | 0x3400..=0x4DBF       // CJK extension A
        | 0x4E00..=0x9FFF       // CJK unified ideographs
        | 0xA960..=0xA97F       // Hangul jamo extended-A
        | 0xAC00..=0xD7A3       // Hangul syllables
        | 0xF900..=0xFAFF       // CJK compatibility ideographs
        | 0xFE30..=0xFE6F       // CJK compatibility and small forms
        | 0xFF01..=0xFF60       // fullwidth forms
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F     // emoji, drawn square
        | 0x20000..=0x3FFFD     // CJK extensions B and beyond
    )
}

/// A Hangul syllable with no final consonant, which Slint 1.18 sets as two loose jamo rather than
/// one block (`slint-pitfalls.md`). Retires when a Slint release composes the block.
const OPEN_HANGUL_EMS: f32 = 2.0;

/// The block is laid out initial-major, so a syllable's index is a multiple of the finals count
/// exactly when it has none.
pub(super) fn is_open_hangul_syllable(ch: char) -> bool {
    const SYLLABLES: core::ops::RangeInclusive<u32> = 0xAC00..=0xD7A3;
    /// How many syllables share one initial and vowel, the final being what separates them.
    const FINALS: u32 = 28;

    let cp = u32::from(ch);
    SYLLABLES.contains(&cp) && (cp - SYLLABLES.start()).is_multiple_of(FINALS)
}

/// **The error directions aren't symmetric**: over-shooting wraps early and costs a blank half-row,
/// under-shooting elides words. So each bucket sits just above its widest character, and none above
/// the whole alphabet, where ordinary prose would measure wide.
pub(super) fn char_ems(ch: char) -> f32 {
    if is_open_hangul_syllable(ch) {
        return OPEN_HANGUL_EMS;
    }
    if is_full_width(ch) {
        return 1.0;
    }
    match ch {
        'm' | 'w' | 'M' | 'W' | '@' | '%' => WIDE_EMS,
        ' ' | '!' | '"' | '\'' | '(' | ')' | ',' | '.' | ':' | ';' | 'I' | '[' | ']' | '`'
        | 'f' | 'i' | 'j' | 'l' | 'r' | 't' | '{' | '|' | '}' => NARROW_EMS,
        _ if ch.is_ascii_uppercase() => UPPERCASE_EMS,
        _ => PROPORTIONAL_EMS,
    }
}

pub(super) fn em_width(text: &str) -> f32 {
    text.chars().map(char_ems).sum()
}

pub(super) fn wrapped_lines(text: &str, width: f32, font_size: f32) -> u8 {
    if text.trim().is_empty() || width <= 0.0 || font_size <= 0.0 {
        return 1;
    }
    let per_line = (width / font_size).max(1.0);
    let needed = em_width(text) / per_line;

    // Bucketed rather than rounded, so the count comes out of comparisons and never a cast.
    if needed <= 1.0 {
        1
    } else if needed <= 2.0 {
        2
    } else {
        MAX_WRAPPED_LINES
    }
}
