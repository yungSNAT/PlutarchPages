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

/// A page showing "Codename BLUEFIN report", with `catalog` and `page` entries added and `extra`
/// objects numbered from 6.
fn leaky_page(catalog: &str, page: &str, extra: &[&str]) -> Vec<u8> {
    page_showing("Codename BLUEFIN report", "/Encoding /WinAnsiEncoding", catalog, page, extra)
}

/// A page showing `text` in Helvetica with `encoding`, as [`leaky_page`].
fn page_showing(text: &str, encoding: &str, catalog: &str, page: &str, extra: &[&str]) -> Vec<u8> {
    let body = format!("BT /F1 10 Tf 20 150 Td ({text}) Tj ET");
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 300] >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R {page} >>"),
        format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica {encoding} >>"),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
    ];
    objs.extend(extra.iter().map(ToString::to_string));
    pdf_of(&objs)
}

/// Open `pdf`, run `before`, redact BLUEFIN where the page shows it, then try to save: the
/// bytes a save would write, or why it refused.
fn redact_then_save(pdf: Vec<u8>, before: impl FnOnce(&mut Session, DocId)) -> (Session, DocId, Result<Arc<Vec<u8>>, EditError>) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("leaky.pdf", None, Arc::new(pdf), None).unwrap();
    before(&mut s, id);
    mark_shown_text(&mut s, id, "BLUEFIN");
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let saved = saved_or_refused(&s, id);
    (s, id, saved)
}

fn saved_or_refused(s: &Session, id: DocId) -> Result<Arc<Vec<u8>>, EditError> {
    s.save_bytes(id).and_then(|b| s.check_save(id, &b).map(|()| b))
}

/// The refusal of a save whose file still holds the redacted word in `catalog`, `page` or
/// `extra` (see [`leaky_page`]); it must name `place`.
fn refusal(catalog: &str, page: &str, extra: &[&str], place: &str) -> String {
    let (_, _, saved) = redact_then_save(leaky_page(catalog, page, extra), |_, _| {});
    let said = saved.expect_err("the save went ahead with the redacted word in the file").to_string();
    assert!(said.contains(place), "{said}");
    said
}

#[test]
fn a_save_refuses_while_a_link_address_holds_redacted_text() {
    let link = "<< /Type /Annot /Subtype /Link /Rect [300 10 400 30] /A << /S /URI /URI (https://example.com/bluefin) >> >>";
    let said = refusal("", "/Annots [6 0 R]", &[link], "a link, action or script on page 1");
    assert!(said.contains("Links, actions and scripts"), "{said}");
}

#[test]
fn a_save_refuses_while_a_document_script_holds_redacted_text() {
    let names = "/Names << /JavaScript << /Names [(start) 6 0 R] >> >>";
    refusal(names, "", &["<< /S /JavaScript /JS (app.alert('Bluefin')) >>"], "link, action or script");
}

#[test]
fn a_save_refuses_while_a_script_stream_holds_redacted_text() {
    let js = "app.alert('BLUEFIN');";
    let stream = format!("<< /Length {} >>\nstream\n{js}\nendstream", js.len());
    refusal("/OpenAction 6 0 R", "", &["<< /S /JavaScript /JS 7 0 R >>", &stream], "link, action or script");
}

/// A named destination's name is told, not refused: nothing removes one name (review B2).
#[test]
fn a_named_destination_holding_redacted_text_is_told_and_saved() {
    let (s, id, saved) = redact_then_save(leaky_page("/Names << /Dests << /Names [(Bluefin plans) [3 0 R /Fit]] >> >>", "", &[]), |_, _| {});
    saved.expect("a destination's name alone doesn't refuse the save");
    assert_eq!(s.get(id).unwrap().redaction_elsewhere().map(|e| e.other), Some(1));
}

#[test]
fn a_save_refuses_while_a_file_an_action_opens_is_named_with_redacted_text() {
    let link = "<< /Type /Annot /Subtype /Link /Rect [300 10 400 30] /A << /S /Launch /F 7 0 R >> >>";
    refusal("", "/Annots [6 0 R]", &[link, "<< /Type /Filespec /F (bluefin.pdf) >>"], "link, action or script");
}

#[test]
fn a_save_refuses_while_a_form_field_value_holds_redacted_text() {
    let said = refusal("/AcroForm << /Fields [6 0 R] >>", "", &["<< /FT /Tx /T (codename) /V (Bluefin) >>"], "a form field (object 6)");
    assert!(said.contains("Form fields"), "{said}");
}

#[test]
fn a_save_refuses_while_an_attached_file_holds_redacted_text() {
    let data = "draft: Codename BLUEFIN report";
    let file = format!("<< /Type /EmbeddedFile /Length {} >>\nstream\n{data}\nendstream", data.len());
    let names = "/Names << /EmbeddedFiles << /Names [(notes.txt) 6 0 R] >> >>";
    refusal(names, "", &["<< /Type /Filespec /F (notes.txt) /EF << /F 7 0 R >> >>", &file], "attachment 1 in the attachments list");
}

#[test]
fn a_save_refuses_while_a_bookmark_holds_redacted_text_and_goes_ahead_once_it_is_removed() {
    let (mut s, id, saved) = redact_then_save(leaky_page("", "", &[]), |s, id| {
        let title = "Bluefin plans".into();
        s.apply(id, Edit::AddBookmark { parent: vec![], index: 0, title, page: 0, left: None, top: None }).unwrap();
    });
    let said = saved.expect_err("the bookmark title still holds the word").to_string();
    assert!(said.contains("bookmark 1 in the outline") && said.contains("Bookmarks"), "{said}");
    s.apply(id, Edit::RemoveHidden { which: vec![crate::Hidden::Bookmarks] }).unwrap();
    let saved = saved_or_refused(&s, id).expect("nothing holds the word now");
    s.mark_saved(id, saved, None).unwrap();
}

#[test]
fn an_undone_redaction_holds_nothing_back() {
    let link = "<< /Type /Annot /Subtype /Link /Rect [300 10 400 30] /A << /S /URI /URI (https://example.com/bluefin) >> >>";
    let (mut s, id, saved) = redact_then_save(leaky_page("", "/Annots [6 0 R]", &[link]), |_, _| {});
    assert!(saved.is_err());
    s.undo(id).unwrap();
    saved_or_refused(&s, id).expect("the redaction was undone, so the link is the user's to keep");
    s.redo(id).unwrap();
    assert!(saved_or_refused(&s, id).is_err(), "redoing it holds the text again");
}

#[test]
fn a_document_without_applied_redactions_saves_whatever_its_links_say() {
    let link = "<< /Type /Annot /Subtype /Link /Rect [300 10 400 30] /A << /S /URI /URI (https://example.com/bluefin) >> >>";
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("plain.pdf", None, Arc::new(leaky_page("", "/Annots [6 0 R]", &[link])), None).unwrap();
    mark_shown_text(&mut s, id, "BLUEFIN");
    saved_or_refused(&s, id).expect("a mark that isn't applied holds nothing back");
}

/// A page label no Remove hidden information category removes is told, not refused: a refusal
/// would leave no way to save.
#[test]
fn text_in_a_place_nothing_removes_is_told_and_saved() {
    let pdf = leaky_page("/PageLabels << /Nums [0 << /S /D /P (Bluefin-) >>] >>", "", &[]);
    let (s, id, saved) = redact_then_save(pdf, |_, _| {});
    saved.expect("a page label alone doesn't refuse the save");
    let left = s.get(id).unwrap().redaction_elsewhere().expect("applying tells where the text is");
    assert_eq!(left.other, 1);
}

/// An edit after undoing an apply drops what the undone apply removed: it can't be redone.
#[test]
fn an_edit_after_undoing_a_redaction_drops_it_for_good() {
    let link = "<< /Type /Annot /Subtype /Link /Rect [300 10 400 30] /A << /S /URI /URI (https://example.com/bluefin) >> >>";
    let (mut s, id, saved) = redact_then_save(leaky_page("", "/Annots [6 0 R]", &[link]), |_, _| {});
    assert!(saved.is_err());
    s.undo(id).unwrap();
    s.apply(id, Edit::AddBookmark { parent: vec![], index: 0, title: "Contents".into(), page: 0, left: None, top: None }).unwrap();
    saved_or_refused(&s, id).expect("the undone redaction is gone with the redo history");
}

/// Undoing the later of 2 applies keeps the earlier one's text held.
#[test]
fn undoing_a_later_redaction_keeps_the_earlier_one_held() {
    let link = "<< /Type /Annot /Subtype /Link /Rect [300 10 400 30] /A << /S /URI /URI (https://example.com/bluefin) >> >>";
    let (mut s, id, saved) = redact_then_save(leaky_page("", "/Annots [6 0 R]", &[link]), |_, _| {});
    assert!(saved.is_err());
    mark_shown_text(&mut s, id, "report");
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    s.undo(id).unwrap();
    assert!(saved_or_refused(&s, id).is_err(), "the first apply's text is still in the link");
}

const BLUEFIN_LINK: &str = "<< /Type /Annot /Subtype /Link /Rect [300 10 400 30] /A << /S /URI /URI (https://example.com/bluefin) >> >>";

/// A redaction mark over `rect` (user space) on the first page.
fn mark_rect(s: &mut Session, id: DocId, rect: [f64; 4]) {
    let shape = Shape::Redact { quads: vec![pdfcraft_annot::rect_quad(rect)], overlay: String::new(), look: Default::default() };
    let mark = NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: String::new(), author: "Ada".into() };
    s.apply(id, Edit::AddAnnotation(mark)).unwrap();
}

/// Open `pdf`, redact `needle` where the page shows it, then try to save.
fn redact_shown_then_save(pdf: Vec<u8>, needle: &str) -> (Session, DocId, Result<Arc<Vec<u8>>, EditError>) {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("leaky.pdf", None, Arc::new(pdf), None).unwrap();
    mark_shown_text(&mut s, id, needle);
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    let saved = saved_or_refused(&s, id);
    (s, id, saved)
}

/// The date a save stamps is never searched (review B1): when the removed text couldn't be known,
/// every place holding text refuses, and that must not include what the save itself writes, or no
/// save could ever succeed.
#[test]
fn a_save_after_redacting_text_that_couldnt_be_known_goes_ahead() {
    let names: String = (1..=26).map(|g| format!(" /g{g}")).collect();
    let pdf = page_showing("Codename BLUEFIN report", &format!("/Encoding << /Differences [65{names}] >>"), "", "", &[]);
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("unknown.pdf", None, Arc::new(pdf), None).unwrap();
    mark_rect(&mut s, id, [10.0, 140.0, 400.0, 170.0]);
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    assert!(s.get(id).unwrap().redaction_elsewhere().is_none_or(|e| e.unsearched), "the removed text isn't known");
    saved_or_refused(&s, id).expect("nothing in the file but what the save writes holds text");
}

/// Redacting a year doesn't refuse the save whose date stamp holds it (review B1).
#[test]
fn a_save_after_redacting_this_year_goes_ahead() {
    let (_, _, saved) = redact_shown_then_save(page_showing("Invoice dated 14/11/2023", "/Encoding /WinAnsiEncoding", "", "", &[]), "2023");
    saved.expect("the save's own date holds 2023 (the test clock is 14 November 2023)");
}

/// A short removed run in a field's default appearance doesn't refuse (review B3).
#[test]
fn a_save_after_redacting_a_number_in_a_field_appearance_goes_ahead() {
    let field = "<< /FT /Tx /T (total) /DA (/Helv 12 Tf 0 g) >>";
    let pdf = page_showing("Total 12 items", "/Encoding /WinAnsiEncoding", "/AcroForm << /Fields [6 0 R] >>", "", &[field]);
    let (_, _, saved) = redact_shown_then_save(pdf, "12");
    saved.expect("/DA holds the font size, not the redacted number");
}

/// Reduce File Size and Optimize PDF write the document too: they refuse as a save does (review B5).
#[test]
fn optimizing_refuses_while_redacted_text_survives() {
    let (s, id, saved) = redact_then_save(leaky_page("", "/Annots [6 0 R]", &[BLUEFIN_LINK]), |_, _| {});
    assert!(saved.is_err());
    assert!(s.reduced_bytes(id).is_err(), "Reduce File Size wrote the link");
    assert!(s.optimized_bytes(id, &optimize::Settings::default(), &[]).is_err(), "Optimize wrote the link");
}

/// Extract and Split write new files from the document: they refuse as a save does (review S1).
#[test]
fn extracting_and_splitting_refuse_while_redacted_text_survives() {
    let (s, id, _) = redact_then_save(leaky_page("", "/Annots [6 0 R]", &[BLUEFIN_LINK]), |_, _| {});
    assert!(s.extract(id, &[0]).is_err(), "Extract wrote the link");
    assert!(s.split(id, &pdfcraft_organize::SplitBy::PageCount(1)).is_err(), "Split wrote the link");
}

/// The refusal says where each place is, so the user can clear that one (review S12), and
/// clearing it lets the save go ahead.
#[test]
fn a_refusal_names_each_place_and_clearing_it_saves() {
    let (mut s, id, saved) = redact_then_save(leaky_page("", "/Annots [6 0 R]", &[BLUEFIN_LINK]), |_, _| {});
    let said = saved.expect_err("the link still holds the word").to_string();
    assert!(said.contains("link, action or script on page 1 (object 6)"), "{said}");
    assert!(!said.to_lowercase().contains("bluefin"), "the refusal never repeats the text: {said}");
    s.apply(id, Edit::DeleteLink { page: 0, index: 0 }).unwrap();
    saved_or_refused(&s, id).expect("the one place holding it is gone");
}

/// A widget holding the text is named with its page (review S10).
#[test]
fn a_widget_holding_redacted_text_refuses_with_its_page() {
    let widget = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (code) /V (Bluefin) /Rect [300 10 400 30] /P 3 0 R >>";
    let (_, _, saved) = redact_then_save(leaky_page("/AcroForm << /Fields [6 0 R] >>", "/Annots [6 0 R]", &[widget]), |_, _| {});
    let said = saved.expect_err("the field still holds the word").to_string();
    assert!(said.contains("form field on page 1"), "{said}");
}

/// Document properties and comments added after the apply are searched too (review S10).
#[test]
fn properties_and_comments_set_after_redacting_refuse() {
    let (mut s, id, _) = redact_then_save(leaky_page("", "", &[]), |_, _| {});
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "Bluefin plans".into() }).unwrap();
    let said = saved_or_refused(&s, id).expect_err("the title holds the word").to_string();
    assert!(said.contains("document properties"), "{said}");
    s.apply(id, Edit::SetInfo { key: "Title".into(), value: "Plans".into() }).unwrap();
    saved_or_refused(&s, id).expect("the title no longer holds it");
    let shape = Shape::Note { at: [100.0, 100.0], icon: NoteIcon::Comment };
    let note = NewAnnotation { page: 0, style: Style::default_for(&shape), shape, contents: "Bluefin again".into(), author: "Ada".into() };
    s.apply(id, Edit::AddAnnotation(note)).unwrap();
    let said = saved_or_refused(&s, id).expect_err("the comment holds the word").to_string();
    assert!(said.contains("comment on page 1"), "{said}");
}

/// Trimming the oldest undo step keeps an apply it can no longer undo held (review S10).
#[test]
fn an_apply_past_the_undo_limit_stays_held() {
    let (mut s, id, saved) = redact_then_save(leaky_page("", "/Annots [6 0 R]", &[BLUEFIN_LINK]), |_, _| {});
    assert!(saved.is_err());
    for i in 0..=crate::history::MAX_UNDO {
        s.apply(id, Edit::SetInfo { key: "Subject".into(), value: format!("draft {i}") }).unwrap();
    }
    assert!(saved_or_refused(&s, id).is_err(), "the apply can't be undone, and the link still holds the word");
}

/// Revert drops the applied redaction and what it held back (review S10).
#[test]
fn reverting_drops_what_an_apply_held_back() {
    let (mut s, id, saved) = redact_then_save(leaky_page("", "/Annots [6 0 R]", &[BLUEFIN_LINK]), |_, _| {});
    assert!(saved.is_err());
    s.revert(id).unwrap();
    saved_or_refused(&s, id).expect("the redaction is gone with the revert");
}

/// A file that would keep an earlier revision refuses while an apply is held, and not otherwise
/// (review S10).
#[test]
fn a_file_keeping_an_earlier_revision_refuses_only_after_an_apply() {
    let mut s = Session::new().with_clock(|| 1_700_000_000);
    let id = s.open("leaky.pdf", None, Arc::new(leaky_page("", "", &[])), None).unwrap();
    s.apply(id, Edit::AddBookmark { parent: vec![], index: 0, title: "Contents".into(), page: 0, left: None, top: None }).unwrap();
    let two = s.save_bytes(id).unwrap();
    assert!(pdfcraft_cos::Document::open(two.clone()).unwrap().revisions().len() > 1, "an incremental save");
    s.check_save(id, &two).expect("no apply is held");
    mark_shown_text(&mut s, id, "BLUEFIN");
    s.apply(id, Edit::ApplyRedactions { pages: None }).unwrap();
    assert_eq!(s.check_save(id, &two), Err(EditError::RedactedRevisionKept));
}

/// The removed text isn't kept in the report of the last apply, which outlives the save (review N1).
#[test]
fn the_last_redaction_report_keeps_no_removed_text() {
    let (s, _, _) = redact_then_save(leaky_page("", "", &[]), |_, _| {});
    let report = s.last_redaction_report().expect("the last edit applied redactions");
    assert_eq!(report.removed, pdfcraft_redact::Removed::default());
}
