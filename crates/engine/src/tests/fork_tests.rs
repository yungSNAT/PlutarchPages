// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// The Document Open password matrix (#603) for every algorithm: no password and an explicit
/// empty password behave the same, a non-empty user password always prompts even when the owner
/// password is empty, and an empty password that is both user and owner opens as the user, so
/// the document's restrictions still apply.
#[test]
fn empty_passwords_never_lift_restrictions_or_skip_the_open_prompt() {
    use pdfcraft_cos::Algorithm::{Aes128, Aes256, Rc4_40, Rc4_128};
    let make = |algorithm, user, owner| {
        let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
        doc.set_encryption_legacy(&pdfcraft_cos::NewEncryption {
            algorithm,
            user_password: user,
            owner_password: owner,
            permissions: 0b0100,
            encrypt_metadata: true,
            seed: [3; 32],
        })
        .unwrap();
        Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).unwrap())
    };
    for alg in [Rc4_40, Rc4_128, Aes128, Aes256] {
        let mut s = Session::new();
        // Both empty: opens without a prompt, as the user (print only, no copying).
        let both = make(alg, "", "");
        for pw in [None, Some("")] {
            let id = s.open("both.pdf", None, both.clone(), pw).unwrap_or_else(|e| panic!("{alg:?} {pw:?}: {e}"));
            let d = s.get(id).unwrap();
            assert!(!d.allows_copying() && !d.allows_assembly(), "{alg:?} {pw:?}: the empty password must not open as owner");
        }
        // A Document Open password with an empty owner password: always prompts.
        let open_pw = make(alg, "secret", "");
        for pw in [None, Some("")] {
            assert!(s.open("open.pdf", None, open_pw.clone(), pw).is_err(), "{alg:?} {pw:?}: must ask for the Document Open password");
        }
        assert!(s.open("open.pdf", None, open_pw, Some("secret")).is_ok(), "{alg:?}");
        // An ordinary restricted file: opens as the user without a prompt.
        let id = s.open("r.pdf", None, make(alg, "", "owner"), None).unwrap();
        assert!(!s.get(id).unwrap().allows_copying(), "{alg:?}");
    }
}

#[test]
fn reduce_saves_nothing_for_a_compact_file_whatever_the_time() {
    use std::sync::atomic::{AtomicI64, Ordering};
    // The copy gets a new /ModDate inside a compressed object stream. Made a few seconds after
    // the document, it was sometimes a byte or two smaller and saved (#490).
    static NOW: AtomicI64 = AtomicI64::new(1_760_000_000);
    let mut s = Session::new().with_clock(|| NOW.load(Ordering::Relaxed));
    let text = s.create_from_text("notes", &"lorem ipsum ".repeat(500)).unwrap();
    let id = s.open_new("notes.pdf", text.clone()).unwrap();
    for t in (1..86_400).step_by(863) {
        NOW.store(1_760_000_000 + t, Ordering::Relaxed);
        let (reduced, ..) = s.reduced_bytes(id).unwrap();
        assert!(!optimizer::reduce_saves(text.len(), reduced.len()), "{t}: {} → {}", text.len(), reduced.len());
    }
    assert!(optimizer::reduce_saves(10_000, 9_000));
}

/// Type 3 text as Chrome (Skia) writes it (#2684): a y-flipped font matrix, glyph boxes in `d1`,
/// the space as an all-zero box (code 0x0F, the issue's trigger after a glyph), and a flipped page.
/// Page 1 draws a glyph, a space and a glyph with it; page 2 is Helvetica text; page 3, turned a
/// quarter, draws the glyph from a form inside a form.
fn chrome_type3_pdf() -> Vec<u8> {
    pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200 300] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F9 9 0 R >> >> >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 12 0 R >> >> >>")
        .obj("<< /Type /Page /Parent 2 0 R /Rotate 90 /Contents 8 0 R /Resources << /XObject << /Outer 13 0 R >> >> >>")
        .stream("", "1 0 0 -1 0 300 cm q BT /F9 40 Tf 1 0 0 -1 30 80 Tm <010F01> Tj ET Q")
        .stream("", "BT /F1 24 Tf 20 150 Td (Page 2) Tj ET")
        .stream("", "q /Outer Do Q")
        .obj(
            "<< /Type /Font /Subtype /Type3 /FontMatrix [0.001 0 0 -0.001 0 0] /FontBBox [0 -700 600 0] /FirstChar 0 /LastChar 15 \
             /Widths [0 600 0 0 0 0 0 0 0 0 0 0 0 0 0 250] /Encoding << /Type /Encoding /Differences [1 /g3B 15 /g3] >> \
             /CharProcs << /g3B 10 0 R /g3 11 0 R >> /Resources << /ProcSet [/PDF] >> >>",
        )
        .stream("", "600 0 0 -700 600 0 d1\n0 -700 600 700 re f")
        .stream("", "250 0 0 0 0 0 d1")
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .stream("/Type /XObject /Subtype /Form /BBox [0 0 200 300] /Resources << /XObject << /Inner 14 0 R >> >>", "/Inner Do")
        .stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 200 300] /Resources << /Font << /F9 9 0 R >> >>",
            "BT /F9 40 Tf 1 0 0 -1 30 250 Tm <01> Tj ET",
        )
        .build()
}

/// The box (pixels at 72 dpi: left, top, right, bottom) around what is drawn dark on `page`.
fn ink_box(pdf: &[u8], page: usize) -> [u32; 4] {
    let mut r = pdfcraft_render::PageRenderer::new(Arc::new(pdf.to_vec()), pdfcraft_render::RenderConfig::default());
    let p = r.render(pdfcraft_render::RenderRequest { page, scale: 1.0, ..Default::default() });
    assert!(p.error.is_none(), "{:?}", p.error);
    let mut b = [u32::MAX, u32::MAX, 0, 0];
    for (i, px) in p.rgba.as_chunks::<4>().0.iter().enumerate() {
        let (x, y) = (i as u32 % p.width, i as u32 / p.width);
        if px[3] > 128 && px[0] < 100 {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
    }
    b
}

/// Pages drawn with Type 3 fonts go to the printer as pictures when `type3_raster` asks for it
/// (the Windows print component can crash the app on such text, #2684), drawn where the vector
/// page prints, while pages with ordinary fonts stay vector text. Without it nothing changes.
#[test]
fn pages_with_type3_fonts_print_as_pictures_and_the_others_stay_vector() {
    let mut s = Session::new();
    let id = s.open("chrome.pdf", None, Arc::new(chrome_type3_pdf()), None).unwrap();
    assert_eq!(s.print_source(id).unwrap().type3_pages(&[0, 1, 2], print::Content::DocumentAndMarkups), [0, 2]);
    let vector_settings = print::Settings { pages: vec![0, 1, 2], layout: print::Layout::Size(print::SizeMode::Actual), ..Default::default() };
    let vector = s.print_pdf(id, &vector_settings).unwrap();
    let raster = print::Raster { dpi: 144.0, grayscale: false, colour: None };
    let mixed = s.print_pdf(id, &print::Settings { type3_raster: Some(raster.clone()), ..vector_settings.clone() }).unwrap();
    let count = |pdf: &[u8], subtype: &[u8]| {
        let doc = pdfcraft_cos::Document::open(Arc::new(pdf.to_vec())).unwrap();
        doc.scan_objects().filter(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Subtype") == Some(subtype))).count()
    };
    assert_eq!((count(&vector, b"Type3"), count(&vector, b"Image")), (1, 0), "as it is, the Type 3 font goes to the printer");
    assert_eq!((count(&mixed, b"Type3"), count(&mixed, b"Image")), (0, 2), "two pictures, and no Type 3 font left to send");
    // Page 2 is still text; pages 1 and 3 are pictures, with no text to find.
    let mut s2 = Session::new();
    let id2 = s2.open("mixed.pdf", None, Arc::new(mixed.clone()), None).unwrap();
    let texts = page_texts(&s2, id2);
    assert!(texts[0].is_empty() && texts[1].contains("Page 2") && texts[2].is_empty(), "{texts:?}");
    // The pictures sit where the pages print, the turned page too.
    for sheet in [0, 2] {
        let (want, got) = (ink_box(&vector, sheet), ink_box(&mixed, sheet));
        assert!(want[0] < want[2] && want[1] < want[3], "sheet {sheet} draws something: {want:?}");
        assert!(want.iter().zip(got).all(|(w, g)| w.abs_diff(g) <= 2), "sheet {sheet}: vector {want:?}, picture {got:?}");
    }
    // Print as image already makes every page a picture; then the Type 3 setting changes nothing.
    let all = print::Settings { raster: Some(raster.clone()), ..vector_settings.clone() };
    assert_eq!(count(&s.print_pdf(id, &all).unwrap(), b"Image"), 3);
    assert_eq!(count(&s.print_pdf(id, &print::Settings { type3_raster: Some(raster), ..all }).unwrap(), b"Image"), 3);
}

/// A print is built from what the document held when it was asked for, also on another thread
/// and after the document has changed or closed (#2684 review: the UI hands it to a worker).
#[test]
fn a_print_source_keeps_the_document_as_it_stood() {
    let mut s = Session::new();
    let id = s.open("chrome.pdf", None, Arc::new(chrome_type3_pdf()), None).unwrap();
    let settings = print::Settings {
        pages: vec![0, 1],
        layout: print::Layout::Size(print::SizeMode::Actual),
        type3_raster: Some(print::Raster { dpi: 72.0, grayscale: false, colour: None }),
        ..Default::default()
    };
    let before = s.print_pdf(id, &settings).unwrap();
    let source = s.print_source(id).unwrap();
    s.apply(id, Edit::DeletePages { pages: vec![1] }).unwrap();
    assert_eq!(s.get(id).unwrap().info.pages.len(), 2, "the document has changed");
    s.close(id);
    let printed = std::thread::spawn(move || source.print(&settings).unwrap()).join().unwrap();
    assert_eq!(printed.type3_pages, [0], "what was found travels with the print, so nobody reads the pages again");
    assert_eq!(printed.bytes, before);
    assert!(matches!(s.print_source(id), Err(EditError::NoDocument)));
}

/// A password-protected document's Type 3 pages print as pictures like any other's: the print
/// copy they are drawn from stays readable.
#[test]
fn type3_pages_of_an_encrypted_document_print_as_pictures() {
    let mut s = Session::new();
    let id = s.open("chrome.pdf", None, Arc::new(chrome_type3_pdf()), None).unwrap();
    s.apply(id, Edit::Protect(protection(Some("open"), None))).unwrap();
    let saved = s.save_bytes(id).unwrap();
    let mut s = Session::new();
    let id = s.open("locked.pdf", None, saved, Some("open")).unwrap();
    let settings = print::Settings {
        pages: vec![0, 1],
        layout: print::Layout::Size(print::SizeMode::Actual),
        type3_raster: Some(print::Raster { dpi: 72.0, grayscale: false, colour: None }),
        ..Default::default()
    };
    let vector = s.print_pdf(id, &print::Settings { type3_raster: None, ..settings.clone() }).unwrap();
    let printed = s.print_source(id).unwrap().print(&settings).unwrap();
    assert_eq!(printed.type3_pages, [0]);
    let (want, got) = (ink_box(&vector, 0), ink_box(&printed.bytes, 0));
    assert!(want[0] < want[2] && want.iter().zip(got).all(|(w, g)| w.abs_diff(g) <= 2), "vector {want:?}, picture {got:?}");
}

/// Printing with `type3_raster` reads every printed page's resources: broken, cyclic and missing
/// ones are not Type 3 fonts, and never stop the print.
#[test]
fn printing_type3_pages_as_pictures_survives_broken_resources() {
    let pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200 300] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources 42 >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font 7 0 R /XObject << /A 8 0 R >> /ExtGState << /G << /SMask << /G 2 0 R >> >> >> >> >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F 99 0 R >> >> /Annots [1 null << /Subtype /Square /F 4 /AP 5 >>] >>")
        .stream("", "0 0 1 rg 10 10 50 50 re f")
        .obj("[/Not /A /Dict]")
        .stream("/Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /A 8 0 R >> >>", "/A Do")
        .build();
    let mut s = Session::new();
    let id = s.open("broken.pdf", None, Arc::new(pdf), None).unwrap();
    assert!(s.print_source(id).unwrap().type3_pages(&[0, 1, 2], print::Content::DocumentAndMarkups).is_empty());
    let settings = print::Settings { pages: vec![0, 1, 2], type3_raster: Some(print::Raster::default()), ..Default::default() };
    let printed = pdfcraft_cos::Document::open(Arc::new(s.print_pdf(id, &settings).unwrap())).unwrap();
    assert_eq!(pdfcraft_model::pages(&printed).len(), 3);
    assert!(!printed.scan_objects().any(|(_, o)| o.as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Image"))), "nothing to picture");
}

/// A page printed as a picture shows what the chosen Comments & Forms prints, as the vector page
/// would: a comment only with the comments, the page content not with form fields only.
#[test]
fn type3_pictures_print_what_the_content_choice_prints() {
    // Type 3 text in the lower left, a black comment in the upper right, and a form field drawn
    // with the Type 3 font in the lower right.
    let pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F9 5 0 R >> >> /Annots [8 0 R 10 0 R] >>")
        .stream("", "BT /F9 40 Tf 1 0 0 -1 30 80 Tm <01> Tj ET")
        .obj(
            "<< /Type /Font /Subtype /Type3 /FontMatrix [0.001 0 0 -0.001 0 0] /FontBBox [0 -700 600 0] /FirstChar 1 /LastChar 1              /Widths [600] /Encoding << /Type /Encoding /Differences [1 /g3B] >> /CharProcs << /g3B 6 0 R >> /Resources << >> >>",
        )
        .stream("", "600 0 0 -700 600 0 d1
0 -700 600 700 re f")
        .stream("/Subtype /Form /BBox [0 0 40 40]", "0 g 0 0 40 40 re f")
        .obj("<< /Type /Annot /Subtype /Square /F 4 /Rect [150 250 190 290] /AP << /N 7 0 R >> >>")
        .stream("/Subtype /Form /BBox [0 0 30 30] /Resources << /Font << /F9 5 0 R >> >>", "BT /F9 30 Tf 1 0 0 -1 0 25 Tm <01> Tj ET")
        .obj("<< /Type /Annot /Subtype /Widget /FT /Tx /T (f) /F 4 /Rect [150 20 180 50] /AP << /N 9 0 R >> >>")
        .build();
    let mut s = Session::new();
    let id = s.open("choices.pdf", None, Arc::new(pdf), None).unwrap();
    let print = |content: print::Content| {
        let settings = print::Settings {
            pages: vec![0],
            layout: print::Layout::Size(print::SizeMode::Actual),
            content,
            type3_raster: Some(print::Raster { dpi: 144.0, grayscale: false, colour: None }),
            ..Default::default()
        };
        ink_box(&s.print_pdf(id, &settings).unwrap(), 0)
    };
    // With the comments, the ink reaches the comment at the top; with the document alone, or
    // with stamps (the comment is no stamp), it stops at the text 210 points lower.
    let markups = print(print::Content::DocumentAndMarkups);
    let document = print(print::Content::Document);
    let stamps = print(print::Content::DocumentAndStamps);
    assert!(document[1] > markups[1] + 150, "comments {markups:?}, document {document:?}");
    assert_eq!(stamps, document);
    // Form fields only: the field on the right, not the page's text 120 points to its left.
    let fields = print(print::Content::FormFieldsOnly);
    assert!(fields[0] > document[0] + 100, "fields {fields:?}, document {document:?}");
}

/// text_paragraphs takes a page's lines and blocks from one interpretation, and both are cached
/// together, so the hidden-text classes describe the lines the blocks index.
#[test]
fn text_lines_and_blocks_come_from_one_cached_interpretation() {
    let (session, id) = session_with(1);
    let editor = session.get(id).unwrap().editor.as_ref().unwrap();
    let blocks = editor.text_blocks(0).unwrap();
    let (lines, cached_blocks) = editor.page_text(0).unwrap();
    assert!(Arc::ptr_eq(&blocks, &cached_blocks), "the blocks come from the cache");
    assert!(Arc::ptr_eq(&lines, &editor.page_text(0).unwrap().0), "the lines are cached with the blocks");
    assert_eq!(blocks[0].lines.iter().map(|&i| lines[i].text.as_str()).collect::<String>(), "Page 1");
}

/// Document::text_lines gives the lines text_lines_and_blocks pairs with the blocks.
#[test]
fn document_text_lines_are_the_lines_paired_with_the_blocks() {
    let (session, id) = session_with(1);
    let doc = session.get(id).unwrap();
    let lines = doc.text_lines(0);
    assert!(!lines.is_empty());
    assert_eq!(lines, doc.text_lines_and_blocks(0).0);
}
