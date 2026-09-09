//! How wide a line is set, without asking the layout.
//!
//! An estimate by construction: measuring means asking Slint, and the panel's design is that Rust
//! decides each row's height first so the offset table and the drawn rows agree. Being wrong costs
//! wrap quality and nothing structural.

/// The space, the `i l t r f` family and the thin punctuation, which are around half of what a
/// Latin line is made of. See [`char_ems`] for what the four buckets are measured against.
const NARROW_EMS: f32 = 0.32;

/// `m w M W @ %`, the only characters Vazirmatn sets near an em.
const WIDE_EMS: f32 = 0.90;

/// A capital, which runs a fifth wider than the lowercase it sits in.
pub(super) const UPPERCASE_EMS: f32 = 0.68;

/// Everything else drawn on a proportional em, letters of any script included.
const PROPORTIONAL_EMS: f32 = 0.57;

/// Where a lyric line stops being a line and starts being a paragraph. Past this the panel would
/// scroll more than it shows.
pub(super) const MAX_WRAPPED_LINES: u8 = 3;

/// Whether a character is drawn on a square em rather than a proportional one.
///
/// Unicode's East Asian Wide and Fullwidth ranges, trimmed to what a lyric sheet reaches. Worth
/// the ranges rather than one averaged width: Hangul and CJK are half of what this panel is for,
/// and counting them proportionally under-estimates a Korean line by nearly half, which is enough
/// to elide words the panel had the room for.
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

/// How wide a Hangul syllable that carries no final consonant is drawn: two ems, not one.
///
/// **Slint 1.16 sets those as two loose jamo rather than one block**, so `안` comes out square and
/// `아` comes out twice as wide as it is written. Charging both a square em under-measured a
/// Korean line by half, which is a wrap the panel never allowed room for: the words rode up over
/// the row above and the tail of the line was dropped. Retires when a Slint release shapes Hangul
/// through the composed glyph, and costs an early wrap in the meantime.
const OPEN_HANGUL_EMS: f32 = 2.0;

/// Whether the character is a Hangul syllable written without a final consonant.
///
/// The block is laid out initial-major, so a syllable's own index is a multiple of the number of
/// finals exactly when it has none.
pub(super) fn is_open_hangul_syllable(ch: char) -> bool {
    /// The Hangul syllables block.
    const SYLLABLES: core::ops::RangeInclusive<u32> = 0xAC00..=0xD7A3;
    /// How many syllables share one initial and vowel, the final being what separates them.
    const FINALS: u32 = 28;

    let cp = u32::from(ch);
    SYLLABLES.contains(&cp) && (cp - SYLLABLES.start()).is_multiple_of(FINALS)
}

/// How wide one character is set, in ems.
///
/// **The two error directions are not symmetric.** Over-shooting wraps early and costs a blank
/// half-row; under-shooting elides the tail of a line, which loses words, so each bucket sits a
/// few percent above the widest character in it. What a bucket may not do is sit above the whole
/// alphabet, which the single averaged width it replaced did: set at the digit width, ordinary
/// prose measured a sixth wide and got a second row's worth of blank space under it.
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

/// How wide a line is set, in ems.
pub(super) fn em_width(text: &str) -> f32 {
    text.chars().map(char_ems).sum()
}

/// How many wrapped lines a run of text takes at the current width and size.
///
/// An estimate, and deliberately so: measuring would mean asking the layout, which is the thing
/// that cannot answer. Being wrong costs nothing structural, because the panel draws whatever comes
/// back and the offset table is built from the same number.
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
