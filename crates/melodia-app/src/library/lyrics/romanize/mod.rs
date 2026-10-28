//! The sung words in Latin letters, for a reader who cannot sound out the script they are in.
//!
//! **Three engines, chosen per run** (`docs/adr/0039-romanization-runs-three-engines.md`): our
//! Revised Romanization for Hangul, `kakasi` for a Japanese sheet and `uroman` for the rest.
//! Whether Han is Japanese is a question about the whole sheet, kana being the only evidence and a
//! line able to carry none while its sheet does, which is why [`apply`] takes all of it.
//!
//! Runs inside the resolver's `spawn_blocking`. The result is stored per line and filtered at draw
//! time, which is what makes the toggle instant.

mod hangul;
mod japanese;

use hangul::{is_hangul_syllable, push_hangul};
use kakasi::IsJapanese;
use melodia_core::entities::lyrics::LyricLine;
use unicode_script::{Script, UnicodeScript};
use uroman::{Uroman, rom_format};

/// Cyrillic letters drawn like ASCII ones, both cases, which uploaded sheets sometimes type English
/// with.
const HOMOGLYPHS: [char; 32] = [
    'а', 'в', 'е', 'к', 'м', 'н', 'о', 'р', 'с', 'т', 'у', 'х', 'і', 'ј', 'ѕ', 'ԁ', 'А', 'В', 'Е',
    'К', 'М', 'Н', 'О', 'Р', 'С', 'Т', 'У', 'Х', 'І', 'Ј', 'Ѕ', 'Ԁ',
];

/// Fill in each line's romanization, where its script wants one.
pub(super) fn apply(lines: &mut [LyricLine]) {
    // The resolver asks this before the hop. Kept for a direct caller: past it every line walks
    // kakasi's sniffer.
    if lines.iter().all(|line| line.text.is_ascii()) {
        return;
    }

    let japanese = lines.iter().any(|line| kakasi::is_japanese(&line.text) == IsJapanese::True);
    for line in lines {
        line.romanization = romanize_line(&line.text, japanese);
    }
}

/// The romanization to draw under `line`, or `None` where it would add nothing, an engine handing
/// the line back unchanged included.
fn romanize_line(line: &str, sheet_is_japanese: bool) -> Option<String> {
    if !needs_romanization(line) || is_latin_in_disguise(line) {
        return None;
    }

    let romanized = if sheet_is_japanese { japanese::romanize(line) } else { other_scripts(line) };
    let romanized = romanized.trim();
    (!romanized.is_empty() && romanized != line.trim()).then(|| romanized.to_owned())
}

/// Whether the line is written in a script worth sounding out, by the Unicode `Script` property
/// rather than a range table that goes stale each release. An accented Latin line is Latin,
/// `Common` and `Inherited` throughout, so its `Cómo` grows no `Como`.
fn needs_romanization(line: &str) -> bool {
    !line.is_ascii()
        && line.chars().any(|letter| {
            !matches!(
                letter.script(),
                Script::Latin | Script::Common | Script::Inherited | Script::Unknown
            )
        })
}

/// Whether the line is ASCII text typed with lookalike letters. Every non-ASCII letter has to be a
/// lookalike and a real ASCII one has to sit beside them, so a Russian line is untouched.
fn is_latin_in_disguise(line: &str) -> bool {
    let mut ascii = false;
    let mut disguised = false;
    for letter in line.chars().filter(|letter| letter.is_alphabetic()) {
        if letter.is_ascii_alphabetic() {
            ascii = true;
        } else if HOMOGLYPHS.contains(&letter) {
            disguised = true;
        } else {
            return false;
        }
    }
    ascii && disguised
}

/// Everything that is not a Japanese sheet: our Hangul, and uroman for the runs between. Splitting
/// costs uroman no context, the spaces, punctuation and Latin between Hangul runs reading the same
/// without a syllable beside them.
fn other_scripts(line: &str) -> String {
    let mut out = String::with_capacity(line.len() * 2);
    let mut rest = line;

    while !rest.is_empty() {
        let hangul_at = rest.find(is_hangul_syllable);
        let (before, from_hangul) = rest.split_at(hangul_at.unwrap_or(rest.len()));
        if !before.is_empty() {
            push_between_hangul(before, &mut out);
        }
        if from_hangul.is_empty() {
            break;
        }
        let run_len =
            from_hangul.find(|letter| !is_hangul_syllable(letter)).unwrap_or(from_hangul.len());
        let (run, after) = from_hangul.split_at(run_len);
        push_hangul(run, &mut out);
        rest = after;
    }
    out
}

/// What sits between two Hangul runs. **An ASCII run never reaches uroman**: every space between
/// Korean words is one, and asking pages uroman's archive in to be told a space is a space.
fn push_between_hangul(run: &str, out: &mut String) {
    if run.is_ascii() {
        out.push_str(run);
        return;
    }
    push_uroman(run, out);
}

/// uroman over one run, with Han spaced a syllable at a time: Chinese writes no spaces, so uroman
/// joins a run of it into one blob, and the `Edges` format's source spans are what find the
/// syllables. No language hint, nothing upstream knowing the sheet's and a wrong one being worse.
fn push_uroman(run: &str, out: &mut String) {
    // A handle over a process-wide archive, which the crate documents as free to make.
    let edges = Uroman::new().romanize_string::<rom_format::Edges>(run, None).to_edges();
    let letters: Vec<char> = run.chars().collect();

    let mut previous_ended_in_han = false;
    for edge in &edges {
        if previous_ended_in_han && is_han_at(&letters, edge.start()) {
            out.push(' ');
        }
        out.push_str(edge.txt());
        previous_ended_in_han = edge.end().checked_sub(1).is_some_and(|at| is_han_at(&letters, at));
    }
}

/// Whether the character at `at` is Han. Past either end it isn't, so a caller can step off the
/// front with `wrapping_sub`.
fn is_han_at(letters: &[char], at: usize) -> bool {
    letters.get(at).is_some_and(|letter| letter.script() == Script::Han)
}

#[cfg(test)]
#[path = "tests/romanize_tests.rs"]
mod tests;
