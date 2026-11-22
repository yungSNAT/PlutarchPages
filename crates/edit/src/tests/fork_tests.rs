// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// Upstream #4861: a variation selector (U+FE0F after a digit or symbol, as emoji keyboards type
/// it) has no glyph in any fallback face, so the whole edit was refused. It is written as an empty
/// glyph and reads back with the text. A paragraph keeps the letters its font has in that font
/// (upstream #2750), so it reads back as a paragraph of several runs.
#[test]
fn a_variation_selector_survives_line_and_block_edits() {
    for replacement in ["1\u{FE0F}", "a1\u{FE0F}b", "қ\u{FE0F}"] {
        for paragraph in [false, true] {
            let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
            if paragraph { text::replace_block(&mut doc, 0, 0, replacement) } else { text::replace_line(&mut doc, 0, 0, replacement) }
                .unwrap_or_else(|e| panic!("{replacement:?}: {e}"));
            let doc = reopen(&doc);
            let read =
                if paragraph { text::text_blocks(&doc, 0).unwrap()[0].text.clone() } else { text::text_lines(&doc, 0).unwrap()[0].text.clone() };
            assert_eq!(read, replacement, "{replacement:?}, paragraph {paragraph}");
        }
    }
}

/// The supplementary selectors (U+E0100 to U+E01EF, used for CJK ideograph variants) take no
/// room under character spacing either, as upstream's test checks for U+FE0F.
#[test]
fn a_supplementary_variation_selector_does_not_shift_centred_text_with_character_spacing() {
    for resource in ["F1", "F2"] {
        let mut plain = text_page(&format!("BT /{resource} 12 Tf 72 700 Td (ab) Tj ET"));
        let mut selected = plain.clone();
        let style = text::BlockStyle { width: Some(100.0), align: Some(added::Align::Center), char_spacing: Some(2.0), ..Default::default() };
        text::rewrite_block(&mut plain, 0, 0, Some("1қ"), &style).unwrap();
        text::rewrite_block(&mut selected, 0, 0, Some("1\u{E0100}қ"), &style).unwrap();
        let a = text::text_lines(&reopen(&plain), 0).unwrap();
        let b = text::text_lines(&reopen(&selected), 0).unwrap();
        assert!(close(a[0].rect, b[0].rect), "{resource}: plain {:?}, selector {:?}", a[0].rect, b[0].rect);
    }
}

/// More unique fallback glyphs than one Type 3 font holds (240) are split across several generated
/// fonts, each within that limit, and the text still reads back as one line.
#[test]
fn fallback_glyph_procedures_and_their_to_unicode_maps_are_compressed() {
    // A Chinese glyph's outline was about 10 KB uncompressed, 69 KB for a 7-character line.
    let replacement = covered_japanese_chars(7);
    let mut doc = text_page("BT /F2 12 Tf 72 700 Td (ab) Tj ET");
    if without_craft_fonts("fallback_glyph_procedures_and_their_to_unicode_maps_are_compressed") {
        return;
    }
    text::replace_line(&mut doc, 0, 0, &replacement).unwrap();
    let reopened = reopen(&doc);
    assert_eq!(text::text_lines(&reopened, 0).unwrap()[0].text, replacement);
    let fonts = fallback_fonts(&reopened);
    assert!(!fonts.is_empty());
    let (mut stored, mut decoded) = (0, 0);
    for (name, font) in &fonts {
        let procs = reopened.dict(font.get(b"CharProcs").unwrap()).unwrap();
        let streams = procs.iter().map(|(_, r)| r.clone()).chain(font.get(b"ToUnicode").cloned());
        for r in streams {
            let pdfcraft_cos::Object::Stream(s) = &*reopened.resolve(&r) else { panic!("{name:?}: not a stream") };
            assert_eq!(s.dict.name(b"Filter"), Some(&b"FlateDecode"[..]), "{name:?}");
            stored += s.raw.len();
            decoded += s.decoded().unwrap().len();
        }
    }
    assert!(stored * 2 < decoded, "{stored} bytes stored for {decoded} decoded");
}

/// A line edit adding characters its font lacks draws only those in Type 3 outlines: the rest
/// keep the line's font, and the line reads back as one, so the next edit replaces all of it.
#[test]
fn a_line_edit_draws_in_type3_only_what_its_font_lacks() {
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
    if without_craft_fonts("a_line_edit_draws_in_type3_only_what_its_font_lacks") {
        return;
    }
    let typed = format!("Hello {}", covered_japanese_chars(2));
    text::replace_line(&mut doc, 0, 0, &typed).unwrap();
    let reopened = reopen(&doc);
    let lines = text::text_lines(&reopened, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), [typed.as_str()]);
    let content = String::from_utf8_lossy(&page_content_bytes(&reopened, 0)).into_owned();
    assert!(content.contains("/F1 12 Tf") && content.contains("(Hello )"), "{content}");
    let p = pdfcraft_model::pages(&reopened).swap_remove(0);
    let res = reopened.resolve(p.dict.get(b"Resources").unwrap());
    let fonts = reopened.resolve(res.as_dict().unwrap().get(b"Font").unwrap());
    let outlines: usize = fonts
        .as_dict()
        .unwrap()
        .iter()
        .map(|(_, f)| reopened.resolve(f).as_dict().unwrap().clone())
        .filter(|f| f.name(b"Subtype") == Some(&b"Type3"[..]))
        .map(|f| reopened.dict(f.get(b"CharProcs").unwrap()).unwrap().iter().count())
        .sum();
    assert_eq!(outlines, 2, "only the 2 Japanese characters are outlines");
    text::replace_line(&mut doc, 0, 0, "Goodbye").unwrap();
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap().iter().map(|l| l.text.clone()).collect::<Vec<_>>(), ["Goodbye"]);

    // A line that starts with characters its font lacks reads back in that font all the same,
    // so the next edit draws in it again.
    let mut doc = text_page("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
    text::replace_line(&mut doc, 0, 0, &format!("{} Hello", covered_japanese_chars(2))).unwrap();
    assert_eq!(text::text_lines(&reopen(&doc), 0).unwrap()[0].font, "F1");
    text::replace_line(&mut doc, 0, 0, "Hello again").unwrap();
    let content = String::from_utf8_lossy(&page_content_bytes(&reopen(&doc), 0)).into_owned();
    assert!(content.contains("(Hello again)"), "{content}");
}

/// A right-to-left line edit keeps the line's right edge (its end along the baseline) however
/// the line is drawn: scaled, sheared or turned by its text matrix, or in several showing
/// operators, and the text after it in the same text object stays where it was.
#[test]
fn hebrew_line_edits_keep_the_right_edge_under_any_text_matrix() {
    // The end of a line's baseline in page space fixes the box's right edge (index 2), its top (3)
    // when turned a quarter, or both when turned part of the way.
    for (content, edges) in [
        ("BT /F1 12 Tf 300 700 Td (EDCBA) Tj ET", &[2][..]),
        ("BT /F1 12 Tf 2 0 0 1.5 200 700 Tm 110 Tz (EDCBA) Tj ET", &[2]),
        ("BT /F1 12 Tf 0 1 -1 0 300 300 Tm (EDCBA) Tj ET", &[3]),
        ("BT /F1 12 Tf 1 0 0.4 1 200 700 Tm (EDCBA) Tj ET", &[2]),
        ("BT /F1 12 Tf 0.866 0.5 -0.5 0.866 200 400 Tm (EDCBA) Tj ET", &[2, 3]),
        ("BT /F1 12 Tf 300 700 Td (EDC) Tj (BA) Tj ET", &[2]),
    ] {
        for typed in ["שלום בדיוק", "דוב"] {
            let mut doc = hebrew_page(content);
            let before = text::text_lines(&doc, 0).unwrap().swap_remove(0);
            assert_eq!(text::replace_line(&mut doc, 0, 0, typed).unwrap().substituted, None, "{content}");
            let after = text::text_lines(&reopen(&doc), 0).unwrap().swap_remove(0);
            assert_eq!(after.text, typed, "{content}");
            for &i in edges {
                assert!((after.rect[i] - before.rect[i]).abs() < 0.5, "{content}, {typed}: {:?} → {:?}", before.rect, after.rect);
            }
        }
    }
    // The next line moves from where the edited line started, not from where its text ended.
    let mut doc = hebrew_page("BT /F1 12 Tf 300 700 Td (EDCBA) Tj 0 -20 Td (ADB) Tj ET");
    let next = text::text_lines(&doc, 0).unwrap()[1].rect;
    text::replace_line(&mut doc, 0, 0, "שלום בדיוק").unwrap();
    let lines = text::text_lines(&reopen(&doc), 0).unwrap();
    assert_eq!(lines[1].text, "דוב");
    assert!(lines[1].rect.iter().zip(next).all(|(a, b)| (a - b).abs() < 0.01), "{:?} vs {next:?}", lines[1].rect);
}

/// A form drawn many times reads each of its fonts once a page, not once a visit: a font can hold
/// CMaps and a program of 16 MiB each. A font object two forms share is read once too, and forms
/// that give `/F1` different fonts, by reference or written out, each keep their own.
#[test]
fn a_form_drawn_many_times_reads_each_font_once() {
    use std::collections::BTreeSet;
    let page = stream("", &"/A Do /B Do /C Do /D Do ".repeat(200));
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /XObject << /A 5 0 R /B 6 0 R /C 7 0 R /D 9 0 R >> >> /Contents 4 0 R >>",
        &page,
        &stream("/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /Font << /F1 8 0 R >> >>", "BT /F1 12 Tf 72 700 Td (a) Tj ET"),
        &stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Times-Bold >> >> >>",
            "BT /F1 12 Tf 72 600 Td (b) Tj ET",
        ),
        &stream("/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /Font << /F2 8 0 R >> >>", "BT /F2 12 Tf 72 500 Td (c) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        &stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Courier >> >> >>",
            "BT /F1 12 Tf 72 400 Td (d) Tj ET",
        ),
    ]);
    let expected =
        BTreeSet::from([("a", "Helvetica"), ("b", "Times-Bold"), ("c", "Helvetica"), ("d", "Courier")].map(|(t, f)| (t.to_owned(), f.to_owned())));

    text::FONT_READS.with(|c| c.set(0));
    let glyphs = text::glyph_text(&doc, &pdfcraft_model::pages(&doc)[0], false).unwrap();
    assert_eq!(text::FONT_READS.with(std::cell::Cell::get), 3, "the audit's walk reads Helvetica and the direct Times-Bold and Courier once each");
    assert!(!glyphs.unread_forms);
    assert_eq!(glyphs.lines.len(), 800);
    assert_eq!(glyphs.lines.into_iter().map(|l| (l.text, l.base_font)).collect::<BTreeSet<_>>(), expected);

    text::FONT_READS.with(|c| c.set(0));
    let blocks = text::reading_blocks(&doc, 0).unwrap();
    assert_eq!(text::FONT_READS.with(std::cell::Cell::get), 3, "export's walk reads them once each too");
    assert_eq!(blocks.into_iter().map(|b| (b.text, b.base_font)).collect::<BTreeSet<_>>(), expected);
}

/// The clip hides text from reading only: the editor edits what the page's own streams hold, a
/// clipped word included, and the redaction audit's glyph reading must still find words a clip
/// hides, inside a form or on the page. The editor reads no form's text, clipped or not.
#[test]
fn clipped_text_stays_in_the_editor_and_glyph_readings() {
    let page = stream("", "q 0 0 50 50 re W n BT /F1 12 Tf 100 100 Td (HIDDEN) Tj ET /Fm Do Q");
    let fm =
        stream("/Type /XObject /Subtype /Form /BBox [0 0 50 50] /Resources << /Font << /F1 6 0 R >> >>", "BT /F1 12 Tf 100 100 Td (OUTSIDE) Tj ET");
    let doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 6 0 R >> /XObject << /Fm 5 0 R >> >> /Contents 4 0 R >>",
        &page,
        &fm,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]);
    assert!(text::reading_blocks(&doc, 0).unwrap().is_empty());
    let editor: Vec<String> = text::text_lines(&doc, 0).unwrap().into_iter().map(|l| l.text).collect();
    let glyphs = text::glyph_text(&doc, &pdfcraft_model::pages(&doc)[0], false).unwrap();
    let glyphs: Vec<&str> = glyphs.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(editor, ["HIDDEN"]);
    assert_eq!(glyphs, ["HIDDEN", "OUTSIDE"]);
}

#[test]
fn vertical_advance_audit_records_forward_gaps() {
    let doc = vertical_page("BT /V1 20 Tf 500 720 Td [<0041> 1000 <0042> -1000 <0043>] TJ ET");
    let page = &pdfcraft_model::pages(&doc)[0];
    let gaps = text::glyph_text(&doc, page, true).unwrap().gaps;
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].from, (500.0, 700.0));
    assert_eq!(gaps[0].to, (500.0, 680.0));
}

/// Text in a font a graphics state's `/Font` sets (`gs`, no `Tf`) is listed in that font, before
/// any `Tf` or after one, and only its own lines are refused for editing: a rewrite names fonts by
/// `Tf`, which would replace the state's font.
#[test]
fn text_in_a_font_a_graphics_state_sets_is_listed_and_not_edited() {
    for content in
        ["BT /GS1 gs 40 500 Td (A) Tj ET BT /F1 12 Tf 40 400 Td (Plain) Tj ET", "BT /F1 12 Tf 40 400 Td (Plain) Tj ET BT /GS1 gs 40 500 Td (A) Tj ET"]
    {
        let mut doc = text_page(content);
        // Helvetica that shows code 65 as B, so the line reads B only in the state's font.
        let differences = Dict::from_iter([(b"Differences".to_vec(), Object::Array(vec![Object::Int(65), Object::name("B")]))]);
        let font = doc.add(Object::Dict(Dict::from_iter([
            (b"Type".to_vec(), Object::name("Font")),
            (b"Subtype".to_vec(), Object::name("Type1")),
            (b"BaseFont".to_vec(), Object::name("Helvetica")),
            (b"Encoding".to_vec(), Object::Dict(differences)),
        ])));
        let page = &pdfcraft_model::pages(&doc)[0];
        let (p, mut res) = (page.obj, page.dict.get(b"Resources").and_then(Object::as_dict).cloned().unwrap());
        let state = Dict::from_iter([(b"Font".to_vec(), Object::Array(vec![Object::Ref(font), Object::Int(12)]))]);
        res.set(b"ExtGState".to_vec(), Object::Dict(Dict::from_iter([(b"GS1".to_vec(), Object::Dict(state))])));
        doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Dict(res))).unwrap();
        let texts = |doc: &Document| {
            let mut texts: Vec<String> = text::text_lines(doc, 0).unwrap().into_iter().map(|l| l.text).collect();
            texts.sort();
            texts
        };
        assert_eq!(texts(&doc), ["B", "Plain"], "{content}");
        let line = |doc: &Document, t: &str| text::text_lines(doc, 0).unwrap().iter().position(|l| l.text == t).unwrap();
        let before = pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap();
        let stated = line(&doc, "B");
        assert!(text::replace_line(&mut doc, 0, stated, "C").is_err(), "{content}");
        let block = text::text_blocks(&doc, 0).unwrap().iter().position(|b| b.text == "B").unwrap();
        assert!(text::replace_block(&mut doc, 0, block, "C").is_err(), "{content}");
        assert!(move_objects(&mut doc, 0, &[ObjectTarget { kind: ObjectKind::Text, index: block }], [5.0, 5.0]).is_err(), "{content}");
        assert_eq!(pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap(), before, "{content}");
        let plain = line(&doc, "Plain");
        text::replace_line(&mut doc, 0, plain, "Changed").unwrap();
        assert_eq!(texts(&doc), ["B", "Changed"], "{content}");
    }
}

/// More than 4,096 graphics states saved at once fail text reading, as they fail the redaction
/// audit: each saved state copies the text state, so a run of a million `q` would otherwise hold a
/// million copies. They are counted across the page and the forms it draws, where each form is
/// drawn; restored ones don't count.
#[test]
fn too_many_saved_graphics_states_fail_text_reading() {
    let text = "BT /F1 12 Tf 40 400 Td (Plain) Tj ET";
    for (content, fails) in [
        (format!("{} {text}", "q ".repeat(1_000_000)), true),
        (format!("{} {text}", "q ".repeat(4097)), true),
        (format!("{} {text}", "q ".repeat(4096)), false),
        (format!("{} {text}", "q Q ".repeat(100_000)), false),
    ] {
        let result = text::text_lines(&text_page(&content), 0);
        assert_eq!(result.is_err(), fails, "{:?}", result.map(|lines| lines.len()));
    }
    let drawn = |page: &str, form: &str| {
        let doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /XObject << /Fm 5 0 R >> >> /Contents 4 0 R >>",
            &stream("", page),
            &stream("/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /Font << /F1 6 0 R >> >>", &format!("{form} {text}")),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        ]);
        text::glyph_text(&doc, &pdfcraft_model::pages(&doc)[0], false).map(|glyphs| glyphs.lines.len())
    };
    assert!(drawn(&format!("{}/Fm Do", "q ".repeat(2000)), &"q ".repeat(2097)).is_err());
    assert_eq!(drawn(&format!("{}/Fm Do", "q ".repeat(2000)), &"q ".repeat(2096)).unwrap(), 1);
    assert_eq!(drawn(&format!("/Fm Do {}", "q ".repeat(2000)), &"q ".repeat(2097)).unwrap(), 1);
}

#[test]
fn nested_optional_content_is_evaluated_once_for_all_runs() {
    let depth = 256;
    let mut content = "/Span << /OC 100 0 R >> BDC ".repeat(depth);
    content.push_str("BT /F1 20 Tf 1 0 0 1 20 200 Tm ");
    content.push_str(&"(x) Tj ".repeat(depth / 2));
    content.push_str("1 0 0 1 20 150 Tm ");
    content.push_str(&"(x) Tj ".repeat(depth / 2));
    content.push_str("ET ");
    content.push_str(&"EMC ".repeat(depth));
    let lines = text_lines(&text_page(&content), 0).unwrap();

    let mut visited = 0;
    let hidden = TextLine::hidden_layer_runs(&lines, |_| {
        visited += 1;
        false
    });
    assert_eq!(visited, depth);
    assert_eq!(hidden, [vec![false; depth / 2], vec![false; depth / 2]]);
}

#[test]
fn text_lines_keep_each_runs_rendering_mode_and_one_opacity_per_line() {
    // A line joins runs in an invisible mode to visible ones, so the mode is kept for each run.
    let doc = text_page("BT /F1 20 Tf 1 0 0 1 20 200 Tm (Hello ) Tj /OCR BMC 3 Tr (secret) Tj EMC 0 Tr ( world) Tj ET");
    let lines = text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Hello secret world"]);
    assert_eq!(lines[0].run_modes().collect::<Vec<_>>(), [(0, false), (3, true), (0, false)]);
    assert_eq!(lines[0].opacity(), (1.0, 1.0));
    // A run at another opacity starts a line of its own, so one opacity holds for a line's runs.
    let doc = row_state_page("BT /F1 20 Tf 1 0 0 1 20 200 Tm (Opaque) Tj ET q /GS1 gs BT /F1 20 Tf 1 0 0 1 92 200 Tm (faint) Tj ET Q");
    let lines = text_lines(&doc, 0).unwrap();
    assert_eq!(lines.iter().map(|l| (l.text.as_str(), l.opacity())).collect::<Vec<_>>(), [("Opaque", (1.0, 1.0)), ("faint", (0.3, 0.3))]);
}
