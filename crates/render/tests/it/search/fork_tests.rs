// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

use unicode_normalization::UnicodeNormalization;

/// A query and text that must find each other, whichever is typed and in either case mode.
fn assert_same_letters(a: &str, b: &str) {
    for sensitive in [false, true] {
        assert_eq!(page(&[a]).find_opts(b, sensitive, false), vec![0..1], "{b:?} in {a:?}, sensitive={sensitive}");
        assert_eq!(page(&[b]).find_opts(a, sensitive, false), vec![0..1], "{a:?} in {b:?}, sensitive={sensitive}");
    }
}

#[test]
fn kangxi_radicals_match_their_ideographs() {
    assert_same_letters("\u{2F92}", "見"); // ⾒
    assert_same_letters("\u{2F00}", "一"); // first in the block
    assert_same_letters("\u{2FD5}", "龠"); // last in the block
    for c in (0x2F00..=0x2FD5).filter_map(char::from_u32) {
        let ideograph: String = c.nfkc().collect();
        assert_ne!(ideograph, c.to_string());
        assert_same_letters(&c.to_string(), &ideograph);
    }
    // The radicals supplement: the two forms Unicode maps, and only those.
    assert_same_letters("\u{2E9F}", "母");
    assert_same_letters("\u{2EF3}", "龟");
    assert!(page(&["\u{2E80}"]).find("\u{2E81}").is_empty());
    assert_eq!(page(&["\u{2E80}"]).find("\u{2E80}"), vec![0..1]);
}

#[test]
fn arabic_presentation_forms_match_base_letters() {
    assert_same_letters("\u{FEB3}", "س"); // initial seen
    assert_same_letters("\u{FEF2}", "ي"); // final yeh
    assert_same_letters("\u{FB8E}", "ك"); // isolated Persian kaf, through ک
    assert_same_letters("\u{FE81}", "آ"); // alef with madda stays composed
    assert_same_letters("\u{FB50}", "\u{671}"); // start of the first block
    assert_same_letters("\u{FEFC}", "لا"); // end of the second block's letters
    // Ligatures with marks are their letters, so the word is found as it is typed.
    assert_same_letters("\u{FC5B}", "ذ");
    assert_eq!(page(&["\u{FC5B}", "ل", "ك"]).find("ذلك"), vec![0..3]);
    // A form of marks alone has no letter, so it matches its mark.
    assert_same_letters("\u{FE70}", "\u{64B}");
    // Every code point in the two blocks still finds itself, assigned or not, but U+FEFF: ZERO
    // WIDTH NO-BREAK SPACE draws nothing, and Find skips it in the query as in the page.
    for c in (0xFB50..=0xFDFF).chain(0xFE70..=0xFEFE).filter_map(char::from_u32) {
        let s = c.to_string();
        assert_eq!(page(&[s.as_str()]).find(&s), vec![0..1], "U+{:04X}", u32::from(c));
    }
}

#[test]
fn arabic_and_persian_letter_variants_match() {
    assert_same_letters("\u{64A}", "\u{6CC}");
    assert_same_letters("\u{643}", "\u{6A9}");
    assert_eq!(page(&["ک", "ت", "ا", "ب"]).find("كتاب"), vec![0..4]);
    // Other letters stay apart.
    assert!(page(&["\u{64A}"]).find("\u{643}").is_empty());
    assert!(page(&["\u{627}"]).find("\u{649}").is_empty());
}

#[test]
fn arabic_indic_digits_match_ascii() {
    for (i, ascii) in ('0'..='9').enumerate() {
        let offset = u32::try_from(i).unwrap();
        for digit in [0x660, 0x6F0].map(|base| char::from_u32(base + offset).unwrap()) {
            assert_same_letters(&digit.to_string(), &ascii.to_string());
        }
    }
    assert_eq!(page(&["۱", "۲", "۳"]).find("123"), vec![0..3]);
    assert_eq!(page(&["1", "2", "3"]).find("١٢٣"), vec![0..3]);
    // Digits are still word characters, so the word boundary holds across the scripts.
    assert!(page(&["۱", "۲", "۳"]).find_opts("12", false, true).is_empty());
    assert_eq!(page(&["۱", "۲", "۳"]).find_opts("123", false, true), vec![0..3]);
}

#[test]
fn matches_map_to_the_right_glyphs_through_expansions() {
    // ﻻ is 2 letters, ﷲ is 4 and ﷺ is 4 words with spaces in one glyph.
    let text = laid_out(&["x", "\u{FEFB}", "y", "\u{FDF2}", "z", "\u{FDFA}", "w"]);
    for (query, hit) in [
        ("x", 0..1),
        ("لا", 1..2),
        ("ل", 1..2),
        ("xل", 0..2),
        ("لاy", 1..3),
        ("y", 2..3),
        ("الله", 3..4),
        ("yالله", 2..4),
        ("اللهz", 3..5),
        ("z", 4..5),
        ("عليه", 5..6),
        ("وسلمw", 5..7),
        ("w", 6..7),
    ] {
        assert_eq!(text.find_opts_limited(query, false, false, 1), vec![hit.clone()], "{query}");
        assert_eq!(text.line_rects(hit.clone()), vec![[10.0 * hit.start as f32, 0.0, 10.0 * hit.end as f32, 10.0]], "{query}");
    }
    // The text reported for a match stays the document's own.
    assert_eq!(text.text_of(1..3), "\u{FEFB}y");
    // Words inside an expansion have word boundaries of their own.
    assert_eq!(page(&["\u{FDFA}"]).find_opts("عليه", false, true), vec![0..1]);
    assert!(page(&["\u{FDFA}"]).find_opts("علي", false, true).is_empty());
}

#[test]
fn match_counts_stay_right_through_expansions() {
    let text = page(&["\u{FEFB}", "\u{FEFB}", "ل", "ا"]);
    assert_eq!(text.find("لا"), vec![0..1, 1..2, 2..4]);
    assert_eq!(text.find("\u{FEFB}"), vec![0..1, 1..2, 2..4]);
    // A match ending inside one glyph and starting the next spans both, without overlap.
    assert_eq!(text.find("ال"), vec![0..2, 1..3]);
    assert_eq!(text.find_opts_limited("لا", false, false, 2), vec![0..1, 1..2]);
    // Two matches inside one glyph count as 2, as they do for any multi-character glyph.
    assert_eq!(page(&["\u{FDF2}"]).find("ل"), vec![0..1, 0..1]);
}

#[test]
fn latin_search_is_not_folded() {
    // Normalising the whole text would break each of these.
    assert_eq!(page(&["e\u{301}"]).find("e"), vec![0..1]);
    assert!(page(&["\u{E9}"]).find("e").is_empty());
    // A ligature is read as its letters, which is not normalising the text.
    assert_eq!(page(&["\u{FB01}"]).find("fi"), vec![0..1]);
    assert!(page(&["\u{BD}"]).find("1").is_empty());
    assert!(page(&["\u{2122}"]).find("tm").is_empty());
    assert!(page(&["\u{B2}"]).find("2").is_empty());
    // Latin digits and case behave as before.
    assert_eq!(page(&["A", "1"]).find_opts("a1", false, false), vec![0..2]);
    assert!(page(&["A", "1"]).find_opts("a1", true, false).is_empty());
}

#[test]
fn redaction_search_finds_the_literal_matches_folding_would_lose() {
    // Folded, the first match takes the variant digit and would leave the last glyph unmarked.
    let shifted = page(&["۱", "1", "1"]);
    assert_eq!(shifted.find("11"), vec![0..2]);
    assert_eq!(shifted.find_for_redaction("11", false, false), vec![0..2, 1..3]);
    // Folded, the rial sign becomes its letters and closes the word boundary before the letter
    // after it.
    let joined = page(&["\u{FDFC}", "ب"]);
    assert!(joined.find_opts("ب", false, true).is_empty());
    assert_eq!(joined.find_for_redaction("ب", false, true), vec![1..2]);
    // Han runs its words together, so the radical folded to an ideograph bounds no word.
    assert_eq!(page(&["\u{2F92}", "見"]).find_for_redaction("見", false, true), vec![0..1, 1..2]);
    // Folding still adds what the literal text alone wouldn't find.
    assert_eq!(page(&["\u{2F92}", " ", "۱"]).find_for_redaction("見", false, false), vec![0..1]);
    assert_eq!(page(&["\u{2F92}", " ", "۱"]).find_for_redaction("1", false, false), vec![2..3]);
    // Nothing is counted twice, and a document with no variants is marked as it always was.
    assert_eq!(page(&["a", "b", "a", "b"]).find_for_redaction("ab", false, false), vec![0..2, 2..4]);
    assert_eq!(page(&["aaaaa"]).find_for_redaction("aa", false, false), vec![0..1, 0..1]);
}

#[test]
fn redaction_search_marks_everything_the_literal_search_did() {
    let alphabet = ["a", "1", "۱", "١", " ", "-", "見", "\u{2F92}", "\u{FEFB}", "ل", "ا", "ی", "ي", "ى", "ک", "ك", "\u{FDF2}", "\u{FE70}", "İ", "\n"];
    let mut seed = 7654321u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 32) as usize
    };
    for case in 0..2000 {
        let parts: Vec<_> = (0..5 + case % 30).map(|_| alphabet[next() % alphabet.len()]).collect();
        let query: String = (0..1 + case % 4).map(|_| alphabet[next() % alphabet.len()]).collect();
        let mut text = page(&parts);
        for i in 1..parts.len() {
            text.space_before[i] = next() % 11 == 0;
            text.line_of[i] = text.line_of[i - 1] + u32::from(next() % 17 == 0);
        }
        for sensitive in [false, true] {
            for whole in [false, true] {
                let marked = text.find_for_redaction(&query, sensitive, whole);
                for hit in reference(&text, &query, sensitive, whole, usize::MAX) {
                    assert!(marked.contains(&hit), "case {case}: {query:?}, {hit:?} lost, sensitive={sensitive}, whole={whole}");
                }
            }
        }
    }
}

/// A ligature with a vowel mark matches its letters typed with or without the mark, and so does
/// the same word spelled out with the mark, in both case modes.
#[test]
fn arabic_marks_match_either_way() {
    for sensitive in [false, true] {
        for query in ["\u{630}\u{670}", "\u{630}"] {
            assert_eq!(page(&["\u{FC5B}"]).find_opts(query, sensitive, false), vec![0..1], "{query}");
            assert_eq!(page(&["\u{630}\u{670}"]).find_opts(query, sensitive, false), vec![0..1], "{query}");
        }
        assert_eq!(page(&["\u{FD3C}"]).find_opts("\u{627}\u{64B}", sensitive, false), vec![0..1]);
    }
    // Marks and kashida drawn as glyphs of their own don't split a word.
    assert_eq!(page(&["\u{643}", "\u{64E}", "\u{62A}", "\u{640}", "\u{628}"]).find("\u{643}\u{62A}\u{628}"), vec![0..5]);
    // A query of marks alone still finds them.
    assert_eq!(page(&["\u{628}", "\u{64B}"]).find("\u{64B}"), vec![1..2]);
}

#[test]
fn alef_maksura_stays_apart_from_yeh() {
    assert!(page(&["\u{639}", "\u{644}", "\u{649}"]).find("\u{639}\u{644}\u{64A}").is_empty());
    assert!(page(&["\u{639}", "\u{644}", "\u{64A}"]).find("\u{639}\u{644}\u{649}").is_empty());
    assert_eq!(page(&["\u{639}", "\u{644}", "\u{6CC}"]).find("\u{639}\u{644}\u{64A}"), vec![0..3]);
}

/// Madda and hamza change the letter, so they aren't dropped as vowel marks are.
#[test]
fn hamza_and_madda_stay_part_of_the_letter() {
    assert_eq!(page(&["\u{627}", "\u{654}"]).find("\u{627}\u{654}"), vec![0..2]);
    assert!(page(&["\u{627}"]).find("\u{627}\u{654}").is_empty());
    assert!(page(&["\u{627}"]).find("\u{627}\u{653}").is_empty());
}

/// A hit takes in a vowel mark drawn as its own glyph after it, so a redaction covers it too.
#[test]
fn a_hit_includes_the_marks_drawn_after_it() {
    assert_eq!(page(&["\u{6A9}", "\u{64E}"]).find("\u{643}\u{64E}"), vec![0..2]);
    assert_eq!(page(&["\u{643}", "\u{64E}", "\u{627}"]).find("\u{643}"), vec![0..2]);
    assert!(page(&["\u{643}", "\u{64E}", "\u{650}", "\u{627}"]).find_for_redaction("\u{643}", false, false).contains(&(0..3)));
    // Nor from the next line.
    let mut lines = page(&["\u{643}", "\u{64E}"]);
    lines.line_of = vec![0, 1];
    assert_eq!(lines.find("\u{643}"), vec![0..1]);
    // Only an Arabic letter takes in what follows it.
    assert_eq!(page(&["a", "\u{640}"]).find("a"), vec![0..1]);
    assert_eq!(page(&["\u{60C}", "\u{640}"]).find("\u{60C}"), vec![0..1]);
    assert_eq!(page(&["\u{886}", "\u{64E}"]).find("\u{886}"), vec![0..2]);
    // Latin marks aren't folded, so they stay out of a hit, as before.
    assert_eq!(page(&["e", "\u{301}"]).find("e"), vec![0..1]);
}

/// A query of marks alone also finds them inside a ligature.
#[test]
fn a_mark_query_finds_marks_in_a_ligature() {
    assert_eq!(page(&["\u{FC5B}"]).find("\u{670}"), vec![0..1]);
}

/// Horizontal glyphs of `size` points, one per character, each line `lead` points below the
/// last and starting at `x`: (x, first line's top, text) per line.
fn laid_out_at(lines: &[(f32, f32, &str)], size: f32) -> PageText {
    let mut glyphs = Vec::new();
    for &(x0, top, text) in lines {
        for (k, c) in text.chars().enumerate() {
            let x = x0 + size * k as f32;
            let (y0, y1) = (top, top + size);
            glyphs.push(TextGlyph {
                text: c.into(),
                rect: [x, y0, x + size, y1],
                quad: [[x, y1], [x + size, y1], [x + size, y0], [x, y0]],
                direction: [1.0, 0.0],
            });
        }
    }
    pdfcraft_render::text::layout(glyphs)
}
