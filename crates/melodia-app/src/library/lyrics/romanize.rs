//! The sung words in Latin letters, for a reader who cannot sound out the script they are in.
//!
//! **Three engines, and which one answers is decided per run rather than per crate.** `uroman`
//! reads about a hundred scripts and is the general case. The two exceptions are the two scripts
//! this feature exists for, and both were measured against the spelling the lyric sites publish
//! before being written:
//!
//! - **Hangul is ours**, because uroman spells ㅊ `c` and the tense consonants `gg`/`dd`/`bb`, so
//!   꽃 came back `ggoc` against Revised Romanization's `kkot`. Ten of eighteen common words were
//!   wrong. RR is arithmetic on the syllable block and needs no dictionary, so there is nothing to
//!   trade for it.
//! - **Japanese is `kakasi`**, because uroman carries no Japanese readings at all and romanizes
//!   kanji as Mandarin: 世界 comes back `shijie` where the sheet wants `sekai`. Kana is the only
//!   thing that tells kanji from hanzi, and a line can carry none while its sheet does, which is
//!   why [`apply`] takes the whole sheet and settles the question once for all of it.
//!
//! **A Latin sheet costs one `is_ascii` pass and no engine call.** The rest runs inside the
//! resolver's `spawn_blocking`, once per track change, so nothing here is on the UI thread. What
//! it produces is stored per line and filtered at draw time, which is what makes the toggle
//! instant.

use kakasi::IsJapanese;
use melodia_core::entities::lyrics::LyricLine;
use unicode_script::{Script, UnicodeScript};
use uroman::{Uroman, rom_format};

/// Cyrillic letters drawn like ASCII ones, both cases.
///
/// Uploaded sheets sometimes type English with these, and the line then carries a Cyrillic script
/// for [`needs_romanization`] to find. Without the guard a reader gets a nonsense second line
/// under a lyric they could already read.
const HOMOGLYPHS: [char; 32] = [
    'а', 'в', 'е', 'к', 'м', 'н', 'о', 'р', 'с', 'т', 'у', 'х', 'і', 'ј', 'ѕ', 'ԁ', 'А', 'В', 'Е',
    'К', 'М', 'Н', 'О', 'Р', 'С', 'Т', 'У', 'Х', 'І', 'Ј', 'Ѕ', 'Ԁ',
];

/// Fill in each line's romanization, where its script wants one.
///
/// Takes the sheet rather than a line because whether its Han characters are Japanese is a
/// question about the sheet: kana is the only evidence, and the line carrying it need not be the
/// line being romanized.
pub(super) fn apply(lines: &mut [LyricLine]) {
    // The common case, and the whole of what it costs.
    if lines.iter().all(|line| line.text.is_ascii()) {
        return;
    }

    let japanese = lines.iter().any(|line| kakasi::is_japanese(&line.text) == IsJapanese::True);
    for line in lines {
        line.romanization = romanize_line(&line.text, japanese);
    }
}

/// The romanization to draw under `line`, or `None` where there is nothing to add.
///
/// The equality at the end is the second net under [`needs_romanization`]: a line an engine hands
/// back unchanged has nothing to say, and drawing it twice is worse than not drawing it.
fn romanize_line(line: &str, sheet_is_japanese: bool) -> Option<String> {
    if !needs_romanization(line) || is_latin_in_disguise(line) {
        return None;
    }

    let romanized = if sheet_is_japanese {
        japanese(line)
    } else {
        other_scripts(line)
    };
    let romanized = romanized.trim();
    (!romanized.is_empty() && romanized != line.trim()).then(|| romanized.to_owned())
}

/// Whether the line is written in a script worth sounding out.
///
/// The Unicode `Script` property rather than a range table of our own, which is the copy that goes
/// stale each Unicode release. `Common` and `Inherited` are punctuation, digits and combining
/// marks, and `Unknown` is a codepoint nothing can read, so an accented Latin line answers `false`
/// and grows no pointless `Como` under its `Cómo`.
fn needs_romanization(line: &str) -> bool {
    !line.is_ascii()
        && line.chars().any(|letter| {
            !matches!(
                letter.script(),
                Script::Latin | Script::Common | Script::Inherited | Script::Unknown
            )
        })
}

/// Whether the line is ASCII text typed with lookalike letters rather than a script of its own.
///
/// Both halves are load-bearing: every non-ASCII letter has to be a lookalike, and a real ASCII
/// letter has to sit beside them, so a genuinely Russian line is untouched.
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

/// Everything that is not a Japanese sheet: our Hangul, and uroman for the rest.
///
/// Split into runs rather than handed over whole because Hangul is the one script here with a
/// better answer than uroman's. Splitting costs uroman no context: what separates two Hangul runs
/// is spaces, punctuation and the occasional Latin word, none of which reads differently for
/// having a syllable beside it.
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

/// What sits between two Hangul runs, romanized only if there is anything to romanize.
///
/// **The gaps between Korean words are runs**, so without the ASCII bail a Korean sheet walks
/// uroman's lattice once per space and pages a multi-megabyte archive in to be told that a space
/// is a space. Latin romanizes to itself, so skipping is the same answer for less: a sheet with
/// nothing but Hangul and English in it never touches uroman at all, and its dictionary stays
/// where it was compiled.
fn push_between_hangul(run: &str, out: &mut String) {
    if run.is_ascii() {
        out.push_str(run);
        return;
    }
    push_uroman(run, out);
}

/// uroman over one run, with a stretch of Han spaced a syllable at a time.
///
/// Chinese writes no spaces, so uroman joins a whole run of Han into one word and a line of it
/// comes back as an unreadable blob. The `Edges` format is the same answer carrying each piece's
/// source span, which is what lets the syllables be split apart without a second pass over the
/// text or a second engine to do it with.
///
/// Deliberately no language hint. uroman takes an ISO 639-3 code and carries rules for twenty-odd
/// of them, but nothing upstream of here knows what language a sheet is in, and a wrong hint is
/// worse than none.
fn push_uroman(run: &str, out: &mut String) {
    // A handle over a process-wide archive rather than a construction; the crate documents it as
    // free, and threading one through would read as though it were not.
    let edges = Uroman::new().romanize_string::<rom_format::Edges>(run, None).to_edges();
    let letters: Vec<char> = run.chars().collect();
    let is_han = |at: usize| letters.get(at).is_some_and(|c| c.script() == Script::Han);

    let mut previous_ended_in_han = false;
    for edge in &edges {
        if previous_ended_in_han && is_han(edge.start()) {
            out.push(' ');
        }
        out.push_str(edge.txt());
        previous_ended_in_han = edge.end().checked_sub(1).is_some_and(is_han);
    }
}

/// Whether the character is a composed Hangul syllable, the only form [`push_hangul`] reads.
fn is_hangul_syllable(letter: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&letter)
}

/// The first codepoint of the Hangul syllables block, which the arithmetic below is relative to.
const HANGUL_BASE: u32 = 0xAC00;

/// How many syllables share one initial, and how many share one initial and vowel.
const HANGUL_VOWEL_SPAN: u32 = 28;
const HANGUL_INITIAL_SPAN: u32 = HANGUL_VOWEL_SPAN * 21;

/// The index of ㅇ, which is silent as an initial and is what a moved final lands on.
const HANGUL_SILENT_INITIAL: usize = 11;

/// The index of ㅣ, the vowel that palatalizes a moved ㄷ or ㅌ.
const HANGUL_VOWEL_I: usize = 20;

/// Revised Romanization of each initial jamo, in jamo order.
const HANGUL_INITIALS: [&str; 19] = [
    "g", "kk", "n", "d", "tt", "r", "m", "b", "pp", "s", "ss", "", "j", "jj", "ch", "k", "t", "p",
    "h",
];

/// Revised Romanization of each vowel jamo, in jamo order.
const HANGUL_VOWELS: [&str; 21] = [
    "a", "ae", "ya", "yae", "eo", "e", "yeo", "ye", "o", "wa", "wae", "oe", "yo", "u", "wo", "we",
    "wi", "yu", "eu", "ui", "i",
];

/// Revised Romanization of each final jamo when it ends the syllable, index 0 being no final.
///
/// A final consonant is unreleased, so these are not the initials' spellings: 한국 ends `k` rather
/// than the `g` that opens 국.
///
/// **A ㄹ compound need not sound the half [`HANGUL_LINKED`] leaves behind.** The two tables agree
/// at ㄼ ㄽ ㄾ and differ at ㄺ ㄻ ㄿ, where the ㄹ is what goes: 닭 is `dak` beside 읽어's
/// `ilgeo`. ㅀ is neither, the ㄹ surviving as the half that *moves* and nothing staying behind
/// it: 싫어 is `sireo`. Neither table is derivable from the other.
const HANGUL_FINALS: [&str; 28] = [
    "", "k", "k", "k", "n", "n", "n", "t", "l", "k", "m", "l", "l", "l", "p", "l", "m", "p", "p",
    "t", "t", "ng", "t", "t", "k", "t", "p", "t",
];

/// What a final leaves behind and what it hands forward when the next syllable opens on ㅇ.
///
/// **This is what makes the spelling match how the word is said**: 있어 is `isseo` rather than
/// `iteo`, and 좋은 is `joeun` rather than `joheun`, ㅎ being silent in the move. A two-consonant
/// final splits, so 없어 is `eopseo`; where one of the two is ㅎ it elides and the other moves.
const HANGUL_LINKED: [(&str, &str); 28] = [
    ("", ""),
    ("", "g"),
    ("", "kk"),
    ("k", "s"),
    ("", "n"),
    ("n", "j"),
    ("", "n"),
    ("", "d"),
    ("", "r"),
    ("l", "g"),
    ("l", "m"),
    ("l", "b"),
    ("l", "s"),
    ("l", "t"),
    ("l", "p"),
    ("", "r"),
    ("", "m"),
    ("", "b"),
    ("p", "s"),
    ("", "s"),
    ("", "ss"),
    ("ng", ""),
    ("", "j"),
    ("", "ch"),
    ("", "k"),
    ("", "t"),
    ("", "p"),
    ("", ""),
];

/// One Hangul syllable, as the three jamo indices the block encodes.
struct Syllable {
    initial: usize,
    vowel: usize,
    final_jamo: usize,
}

/// Revised Romanization of a run of Hangul, resyllabified across it.
///
/// **No assimilation between syllables**, so 신라 comes out `sinra` where the standard says
/// `silla`. That rule needs to know where the words are, which needs a dictionary; every other
/// romanizer without one makes the same trade, and it is the spelling a reader gets from typing
/// the letters they see.
fn push_hangul(run: &str, out: &mut String) {
    let syllables: Vec<Syllable> = run.chars().filter_map(decompose_hangul).collect();

    // What the previous syllable's final handed forward, waiting for a silent initial to land on.
    let mut carried = "";
    for (index, syllable) in syllables.iter().enumerate() {
        if syllable.initial == HANGUL_SILENT_INITIAL {
            out.push_str(palatalized(carried, syllable.vowel));
        } else {
            out.push_str(HANGUL_INITIALS[syllable.initial]);
        }
        out.push_str(HANGUL_VOWELS[syllable.vowel]);

        let next_opens_silent =
            syllables.get(index + 1).is_some_and(|next| next.initial == HANGUL_SILENT_INITIAL);
        if next_opens_silent {
            let (stays, moves) = HANGUL_LINKED[syllable.final_jamo];
            out.push_str(stays);
            carried = moves;
        } else {
            out.push_str(HANGUL_FINALS[syllable.final_jamo]);
            carried = "";
        }
    }
}

/// A moved ㄷ or ㅌ softens before ㅣ, which is why 같이 is `gachi` and not `gati`.
fn palatalized(carried: &str, vowel: usize) -> &str {
    match (carried, vowel) {
        ("d", HANGUL_VOWEL_I) => "j",
        ("t", HANGUL_VOWEL_I) => "ch",
        _ => carried,
    }
}

/// Split a composed syllable into its three jamo indices.
fn decompose_hangul(letter: char) -> Option<Syllable> {
    if !is_hangul_syllable(letter) {
        return None;
    }
    let offset = u32::from(letter) - HANGUL_BASE;
    Some(Syllable {
        initial: (offset / HANGUL_INITIAL_SPAN) as usize,
        vowel: ((offset / HANGUL_VOWEL_SPAN) % 21) as usize,
        final_jamo: (offset % HANGUL_VOWEL_SPAN) as usize,
    })
}

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
fn japanese(line: &str) -> String {
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
    let is_han = |at: usize| letters.get(at).is_some_and(|c| c.script() == Script::Han);

    let mut out = String::with_capacity(line.len());
    for (index, letter) in letters.iter().enumerate() {
        let alone = !is_han(index.wrapping_sub(1)) && !is_han(index + 1);
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

#[cfg(test)]
#[path = "tests/romanize_tests.rs"]
mod tests;
