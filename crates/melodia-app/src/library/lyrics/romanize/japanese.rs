//! Japanese through kakasi, and the preparation a lyric needs before it sees one.

use unicode_script::{Script, UnicodeScript};

use super::is_han_at;

/// Readings kakasi gets wrong in a lyric, substituted into the source before it sees them. Each was
/// measured wrong rather than guessed: a standalone 君 reads `kun`, 月 `gatsu` and 人 `nin`, the
/// counter and compound readings rather than the words.
const LYRIC_READINGS: [(char, &str); 3] = [('君', "きみ"), ('月', "つき"), ('人', "ひと")];

/// Greetings whose written は is part of the word rather than a particle a boundary could find.
const FUSED_PARTICLES: [(&str, &str); 2] =
    [("こんにちは", "こんにちわ"), ("こんばんは", "こんばんわ")];

/// kakasi over the line, after nudging the source where kakasi alone reads it wrong. The spaces the
/// nudges insert tell kakasi where a word ends and it keeps them, so they are collapsed on the way
/// out.
pub(super) fn romanize(line: &str) -> String {
    let prepared = prepare_japanese(line);
    let romaji = respell_particles(&kakasi::convert(&prepared).romaji, &prepared);
    collapse_spaces(&romaji)
}

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

/// Rewrite the source into the form kakasi reads correctly. The particle split runs before
/// [`LYRIC_READINGS`], while the kanji in front of a particle is still kanji to find it by.
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

/// Stand a は or へ that is acting as a particle off on its own, kakasi splitting 君は into two
/// tokens and 僕は into one. **The test is the character in front**: a written noun phrase usually
/// ends in kanji or katakana, where a は inside a hiragana word like やはり never follows one.
/// Fenced on both sides, so the compound へと comes apart.
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

/// Replace a kanji standing alone with the reading a lyric wants, never inside a compound (君主 is
/// `kunshu`). Fenced like the particles, or 君の would come back `kimino`.
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

/// Respell a standalone `ha` or `he` as the particle it almost always is, a lone 葉 being the rare
/// miss. **Guarded on the source**, so an English `ha ha ha` in a Japanese sheet keeps its laugh.
/// を stays `wo`, the lyric sites' spelling.
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
