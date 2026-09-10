//! Revised Romanization of Hangul, by arithmetic on the syllable block.

/// Whether the character is a composed Hangul syllable, the only form [`push_hangul`] reads.
pub(super) fn is_hangul_syllable(letter: char) -> bool {
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
pub(super) fn push_hangul(run: &str, out: &mut String) {
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
