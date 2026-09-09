//! Japanese through kakasi, and the preparation a lyric needs before it sees one.

use unicode_script::{Script, UnicodeScript};

use super::is_han_at;

/// Readings kakasi gets wrong in a lyric, substituted into the source before it sees them.
///
/// Every entry is a character measured as wrong, not a guess at what might be: kakasi reads a
/// standalone 君 as `kun`, 月 as `gatsu` and 人 as `nin`, which are the counter and compound
/// readings rather than the words. It is right about the other twenty-five common lyric kanji
/// tested, so this table stays short by construction.
const LYRIC_READINGS: [(char, &str); 3] = [('君', "きみ"), ('月', "つき"), ('人', "ひと")];

/// Greetings whose written は is part of the word rather than a particle a boundary could find.
const FUSED_PARTICLES: [(&str, &str); 2] =
    [("こんにちは", "こんにちわ"), ("こんばんは", "こんばんわ")];

/// kakasi over the line, after the source has been nudged where kakasi alone reads it wrong.
///
/// The spaces the preparation inserts are how kakasi is told where a word ends, and it keeps them,
/// so they are collapsed on the way out rather than being placed carefully on the way in.
pub(super) fn romanize(line: &str) -> String {
    let prepared = prepare_japanese(line);
    let romaji = respell_particles(&kakasi::convert(&prepared).romaji, &prepared);
    collapse_spaces(&romaji)
}

/// Squeeze runs of spaces down to one.
fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_gap = false;
    for letter in text.chars() {
        if letter == ' ' {
            if !in_gap {
                out.push(' ');
            }
            in_gap = true;
        } else {
            out.push(letter);
            in_gap = false;
        }
    }
    out
}

/// Rewrite the source into the form kakasi reads correctly.
///
/// Three passes, and the order between the last two is the point: splitting at the particle has to
/// happen while the kanji in front of it is still a kanji, because [`LYRIC_READINGS`] replaces it
/// with kana and the boundary would then be invisible.
fn prepare_japanese(line: &str) -> String {
    let mut prepared = line.to_owned();
    for (written, said) in FUSED_PARTICLES {
        if prepared.contains(written) {
            prepared = prepared.replace(written, said);
        }
    }
    prepared = split_before_particles(&prepared);
    substitute_lyric_readings(&prepared)
}

/// Stand a は or へ that is acting as a particle off on its own.
///
/// kakasi segments words but tags no parts of speech, and it splits inconsistently: 君は comes
/// back as two tokens and 僕は as one, so the respelling below reaches only half the particles
/// without this. **The test is the character in front**, because a particle attaches to a noun
/// phrase and a written one ends in kanji or katakana far more often than not, where a は inside a
/// hiragana word never has one before it. That is what keeps やはり and ははおや whole.
///
/// Fenced on both sides rather than just in front, so the compound へと comes apart into the two
/// particles it is; a boundary is only ever inserted where the character in front already said
/// this one stands alone.
fn split_before_particles(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    let mut previous: Option<char> = None;

    for letter in line.chars() {
        let follows_a_word = previous
            .is_some_and(|before| matches!(before.script(), Script::Han | Script::Katakana));
        if matches!(letter, 'は' | 'へ') && follows_a_word {
            out.push(' ');
            out.push(letter);
            out.push(' ');
        } else {
            out.push(letter);
        }
        previous = Some(letter);
    }
    out
}

/// Replace a kanji standing on its own with the reading a lyric wants.
///
/// Only where the character is a run of one. 君 alone is `kimi`, but 君主 is `kunshu`, and a
/// substitution inside a compound would break every one of them. Fenced like the particles above:
/// the kana that replaces it would otherwise merge into the word after it, and 君の would come
/// back `kimino`.
fn substitute_lyric_readings(line: &str) -> String {
    let letters: Vec<char> = line.chars().collect();

    let mut out = String::with_capacity(line.len());
    for (index, letter) in letters.iter().enumerate() {
        let alone = !is_han_at(&letters, index.wrapping_sub(1)) && !is_han_at(&letters, index + 1);
        match LYRIC_READINGS.iter().find(|(kanji, _)| kanji == letter) {
            Some((_, reading)) if alone => {
                out.push(' ');
                out.push_str(reading);
                out.push(' ');
            }
            _ => out.push(*letter),
        }
    }
    out
}

/// Respell a standalone `ha` or `he` as the particle it almost always is.
///
/// Almost, not always: a lone 葉 also reads `ha`, and the trade is one mis-spelled syllable in a
/// rare word against `kimi ha` in every second line. **The guard is the source**, so an English
/// `ha ha ha` inside a Japanese sheet keeps its laugh: those lines carry no は for it to be. を is
/// left as `wo`, which is the spelling the lyric sites use and the one a reader would type.
fn respell_particles(romaji: &str, source: &str) -> String {
    let topic = source.contains('は');
    let direction = source.contains('へ');
    if !topic && !direction {
        return romaji.to_owned();
    }

    let mut out = String::with_capacity(romaji.len());
    for (index, token) in romaji.split(' ').enumerate() {
        if index > 0 {
            out.push(' ');
        }
        // Punctuation rides along on the token, and a trimmed prefix is always a char boundary.
        let (word, trailing) = token.split_at(token.trim_end_matches(is_ascii_punctuation).len());
        out.push_str(match word {
            "ha" if topic => "wa",
            "he" if direction => "e",
            unchanged => unchanged,
        });
        out.push_str(trailing);
    }
    out
}

/// `char::is_ascii_punctuation` takes a reference, which no string pattern will accept.
fn is_ascii_punctuation(letter: char) -> bool {
    letter.is_ascii_punctuation()
}
