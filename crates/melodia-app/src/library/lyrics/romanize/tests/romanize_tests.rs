//! What the romanizer owns, and what it only passes through.
//!
//! **Goldens are pinned for the parts written here and not for the parts uroman answers.** Hangul,
//! the Japanese pipeline and the Han spacing are this module's, so a change to any of them should
//! fail a test. Cyrillic, Greek, Arabic and the rest are uroman's spelling, and pinning it would
//! make an upstream bump look like a regression here; those are checked for the properties the
//! panel actually depends on instead.

use super::*;

fn sheet(texts: &[&str]) -> Vec<LyricLine> {
    texts
        .iter()
        .map(|text| LyricLine {
            at_ms: None,
            end_ms: None,
            text: (*text).to_owned(),
            romanization: None,
            translation: None,
        })
        .collect()
}

/// Every line's answer, in order.
fn romanized(texts: &[&str]) -> Vec<Option<String>> {
    let mut lines = sheet(texts);
    apply(&mut lines);
    lines.into_iter().map(|line| line.romanization).collect()
}

/// One line's answer, for the cases where the sheet is the line.
fn one(text: &str) -> Option<String> {
    romanized(&[text]).into_iter().next().flatten()
}

#[test]
fn hangul_is_revised_romanization() {
    let words = [
        ("사랑", "sarang"),
        ("한국", "hanguk"),
        ("난 너를 원해", "nan neoreul wonhae"),
        ("꽃", "kkot"),
        ("첫사랑", "cheotsarang"),
        ("추억", "chueok"),
        ("괜찮아", "gwaenchana"),
        ("의미", "uimi"),
        ("아름다운", "areumdaun"),
        ("지금 이 순간", "jigeum i sungan"),
    ];
    for (hangul, revised) in words {
        assert_eq!(one(hangul).as_deref(), Some(revised), "{hangul}");
    }
}

/// A final consonant moves onto a following silent ㅇ, which is what makes the spelling match how
/// the word is said. Every arm of `HANGUL_LINKED` that behaves differently from the others.
#[test]
fn a_final_consonant_carries_onto_the_next_syllable() {
    let words = [
        ("있어", "isseo"),     // plain move
        ("좋은", "joeun"),     // ㅎ elides and leaves nothing
        ("많아", "mana"),      // two-consonant final, the ㅎ half elides
        ("싫어", "sireo"),     // the same, with ㄹ moving
        ("없어", "eopseo"),    // two-consonant final splits across the boundary
        ("읽어", "ilgeo"),     // the same, with ㄹ staying
        ("강아지", "gangaji"), // ㅇ stays put, nothing moves
        ("꽃이", "kkochi"),    // ㅊ moves and is spelled as an initial again
        ("같이", "gachi"),     // ㅌ palatalizes before ㅣ
        ("굳이", "guji"),      // ㄷ does the same
    ];
    for (hangul, revised) in words {
        assert_eq!(one(hangul).as_deref(), Some(revised), "{hangul}");
    }
}

/// **Assimilation between syllables is the one rule not implemented**, and this pins the trade
/// rather than the bug: 신라 is said `silla`, and spelling it `sinra` is what every romanizer
/// without a dictionary does. A test that asserted `silla` would be asserting a feature.
#[test]
fn hangul_does_not_assimilate_across_a_syllable_boundary() {
    assert_eq!(one("신라").as_deref(), Some("sinra"));
}

#[test]
fn a_topic_particle_is_said_rather_than_spelled() {
    // kakasi splits 君は into two tokens and 僕は into one, so both shapes have to be covered.
    assert_eq!(one("君は笑う").as_deref(), Some("kimi wa warau"));
    assert_eq!(one("僕は歩く").as_deref(), Some("boku wa aruku"));
    assert_eq!(one("世界は僕のもの").as_deref(), Some("sekai wa boku nomono"));
    // へと is two particles, which is why the boundary is inserted on both sides.
    assert_eq!(one("空へと向かう").as_deref(), Some("sora e to muka u"));
}

/// The particle rule may only fire where the character in front says the kana stands alone, or it
/// takes the は out of an ordinary word.
#[test]
fn a_particle_inside_a_word_is_left_alone() {
    for word in ["やはり", "ははおや", "いろは", "はなびら"] {
        let romanized = one(word).unwrap_or_default();
        assert!(!romanized.contains("wa"), "{word} became {romanized}");
    }
}

/// An English laugh inside a Japanese sheet carries no は, which is the guard that saves it.
#[test]
fn an_ascii_line_in_a_japanese_sheet_keeps_its_laugh() {
    let answers = romanized(&["ha ha ha", "君は笑う"]);
    assert_eq!(answers.first().and_then(Option::as_deref), None);
    assert_eq!(answers.get(1).and_then(Option::as_deref), Some("kimi wa warau"));
}

/// The overridden readings are for a kanji standing on its own; a compound keeps kakasi's.
///
/// Both lines need the kana line beside them: on its own a sheet of bare kanji is Chinese, which
/// is [`the_sheet_decides_whether_its_kanji_are_japanese`]'s subject rather than this one's.
#[test]
fn a_lyric_reading_replaces_only_a_kanji_on_its_own() {
    let answers = romanized(&["君の名前を呼ぶよ", "君主", "こんにちは"]);
    assert_eq!(answers.first().and_then(Option::as_deref), Some("kimi no namae wo yobu yo"));
    assert_eq!(answers.get(1).and_then(Option::as_deref), Some("kunshu"));
}

/// は fused into a greeting has no boundary for the particle rule to find.
#[test]
fn a_fused_particle_is_substituted_whole() {
    assert_eq!(one("こんにちは").as_deref(), Some("konnichiwa"));
    assert_eq!(one("こんばんは").as_deref(), Some("konbanwa"));
}

/// **Kana anywhere in the sheet decides for every line in it**, because kanji and hanzi are the
/// same codepoints and a line of kanji carries no evidence of its own.
#[test]
fn the_sheet_decides_whether_its_kanji_are_japanese() {
    let japanese = romanized(&["世界", "こんにちは"]);
    assert_eq!(japanese.first().and_then(Option::as_deref), Some("sekai"));

    let chinese = romanized(&["世界", "我爱你"]);
    assert_eq!(chinese.first().and_then(Option::as_deref), Some("shi jie"));
}

/// Chinese writes no spaces, so uroman returns one word for a whole line and the syllables are
/// split back apart off the source spans.
#[test]
fn han_is_spaced_one_syllable_at_a_time() {
    assert_eq!(one("我爱你").as_deref(), Some("wo ai ni"));
    assert_eq!(
        one("加拿大在一万四千年前即有原住民在此生活。").as_deref(),
        Some("jia na da zai 14000 nian qian ji you yuan zhu min zai ci sheng huo.")
    );
}

/// uroman's own answer, so the assertion is what the panel depends on rather than the spelling.
#[test]
fn every_other_script_comes_back_readable() {
    for line in [
        "Привет, мир! Как дела?",
        "Καλημέρα, κόσμε.",
        "مرحبا بالعالم",
        "שלום עולם",
        "สวัสดีชาวโลก",
        "नमस्ते दुनिया",
        "გამარჯობა",
        "Բարև աշխարհ",
    ] {
        let romanized = one(line).unwrap_or_default();
        assert!(!romanized.is_empty(), "{line} romanized to nothing");
        assert!(romanized.is_ascii(), "{line} romanized to non-Latin {romanized}");
        assert_ne!(romanized, line);
    }
}

/// A Latin line has nothing to add, accents included: folding `Cómo` to `Como` would put a second
/// line under a lyric that is already readable.
#[test]
fn a_latin_line_gets_no_second_line() {
    for line in [
        "I want you",
        "Cómo estás",
        "Déjà vu",
        "Grüße aus Bordeaux",
        "",
        "   ",
    ] {
        assert_eq!(one(line), None, "{line}");
    }
}

/// English typed with Cyrillic lookalikes carries a Cyrillic script and is still English.
#[test]
fn english_in_disguise_is_not_romanized() {
    // "все" here is а/е Cyrillic inside an otherwise ASCII word.
    assert_eq!(one("I lovе уou аll"), None);
    // A genuinely Cyrillic line is untouched by the guard.
    assert!(one("Как дела").is_some());
}

/// An all-ASCII sheet takes the early return, so nothing is asked of either engine.
#[test]
fn an_ascii_sheet_is_left_entirely_alone() {
    let answers = romanized(&["Hello world", "Goodbye", ""]);
    assert!(answers.iter().all(Option::is_none));
}

/// A blank line inside a sheet spaces its verses and has nothing to romanize.
#[test]
fn a_blank_line_keeps_its_place_and_gains_nothing() {
    let answers = romanized(&["사랑", "", "밤"]);
    assert_eq!(answers.first().and_then(Option::as_deref), Some("sarang"));
    assert_eq!(answers.get(1).and_then(Option::as_deref), None);
    assert_eq!(answers.get(2).and_then(Option::as_deref), Some("bam"));
}

/// **The gaps between Korean words are runs of their own**, and handing each to uroman pages a
/// multi-megabyte archive in to be told that a space is a space. Latin romanizes to itself, so the
/// bail has to be invisible in the output: a sheet of Hangul and English comes out the same either
/// way, and never asks uroman anything.
#[test]
fn latin_between_hangul_is_passed_through() {
    assert_eq!(
        one("\u{b09c} \u{b108}\u{b97c} \u{c6d0}\u{d574} BLACKPINK in your area").as_deref(),
        Some("nan neoreul wonhae BLACKPINK in your area")
    );
}
