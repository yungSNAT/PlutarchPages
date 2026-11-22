// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// Find and redaction see script variants as the letters they are, report the document's own text
/// and box each glyph alone. Glyphs are found one at a time, so right-to-left reordering is moot.
#[test]
fn text_find_and_redact_mark_match_script_variants() {
    use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full};
    use std::sync::Arc;

    let dir = workdir("script-variants");
    let mut doc = Document::open(Arc::new(fixture(1))).unwrap();
    // Synthetic glyph codes: A Persian kaf, B Persian yeh, C Persian 1, X lam-alef, Y Kangxi "see".
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapType 2 def \
                1 begincodespacerange <00> <FF> endcodespacerange 5 beginbfchar \
                <41> <06A9> <42> <06CC> <43> <06F1> <58> <FEFB> <59> <2F92> \
                endbfchar endcmap CMapName currentdict /CMap defineresource pop end end";
    let unicode = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));
    doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"ToUnicode".to_vec(), Object::Ref(unicode))).unwrap();
    let content = "BT /F1 24 Tf 20 150 Td (A) Tj 40 0 Td (B) Tj 40 0 Td (C) Tj 40 0 Td (X) Tj 40 0 Td (Y) Tj ET";
    doc.set(ObjRef::new(5, 0), Object::Stream(Stream::flate(Dict::new(), content.as_bytes())));
    std::fs::write(dir.join("variants.pdf"), write_full(&doc, &SaveOptions::default()).unwrap()).unwrap();

    let mut a = auto(&dir);
    let id = ok(&mut a, "doc_open", json!({"path":"variants.pdf"}))["doc"].as_u64().unwrap();
    for (typed, drawn) in [
        ("\u{643}", "\u{6A9}"),
        ("\u{64A}", "\u{6CC}"),
        ("1", "\u{6F1}"),
        ("\u{644}\u{627}", "\u{FEFB}"),
        ("\u{627}", "\u{FEFB}"),
        ("\u{898B}", "\u{2F92}"),
    ] {
        let found = ok(&mut a, "text_find", json!({"doc":id,"query":typed}));
        assert_eq!(found["count"], 1, "{typed}: {found}");
        assert_eq!(found["matches"][0]["text"], drawn, "{typed}");
        let rects = found["matches"][0]["rects"].as_array().unwrap();
        assert_eq!(rects.len(), 1, "{typed}");
        assert_eq!(found["matches"], ok(&mut a, "text_find", json!({"doc":id,"query":drawn}))["matches"], "{typed}");
    }
    let marked = ok(&mut a, "redact_mark", json!({"doc":id,"words":["\u{643}","1"]}));
    assert_eq!(marked["marks_pending"].as_u64(), Some(2), "{marked}");
}

/// A saved file with each `/ID` array blanked: a save draws the identifier at random, as hex digits
/// that can spell the ones a test searches for.
fn without_file_id(mut bytes: Vec<u8>) -> Vec<u8> {
    let mut from = 0;
    while let Some(key) = bytes[from..].windows(3).position(|w| w == b"/ID") {
        from += key + 3;
        let open = from + bytes[from..].iter().take_while(|b| b.is_ascii_whitespace()).count();
        if bytes.get(open) == Some(&b'[')
            && let Some(len) = bytes[open..].iter().position(|&b| b == b']')
        {
            bytes[open..open + len].fill(b' ');
        }
    }
    bytes
}

/// An applied redaction at the end of a line leaves the words before it, then a gap the text
/// matrix jumps over and the redaction box: nothing is under the box any more, so the audit must
/// not report the words beside it as covered. In the middle of a line it never did.
#[test]
fn an_applied_redaction_exposes_width_without_covered_text() {
    let dir = workdir("audit-applied-end-of-line");
    let body = "BT /F1 11 Tf 72 692 Td (Mail the team: someone@example.com) Tj ET\nBT /F1 11 Tf 72 662 Td (Account 0042-7788 is in the middle of this line.) Tj ET";
    std::fs::write(dir.join("mail.pdf"), helvetica_page(body)).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "mail.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "redact_mark", json!({ "doc": doc, "pattern": "email" }));
    ok(&mut a, "redact_mark", json!({ "doc": doc, "find": "0042-7788" }));
    ok(&mut a, "redact_apply", json!({ "doc": doc }));
    let text = ok(&mut a, "text_extract", json!({ "doc": doc }))["pages"][0]["text"].as_str().unwrap().to_string();
    assert!(text.contains("Mail the team:") && !text.contains("someone") && !text.contains("0042"), "{text}");
    let r = ok(&mut a, "doc_audit_redactions", json!({ "doc": doc }));
    let findings = r["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 2, "{r}");
    assert!(findings.iter().all(|f| f["kind"] == "retained-advance" && f["covered_text"] == ""), "{r}");
    assert_eq!(r["document_findings"], json!([]), "{r}");
}

/// #568: a document whose security forbids copying (extract bit clear, empty user password)
/// must refuse text extraction in automation; the owner password lifts the restriction.
#[test]
fn copy_restricted_documents_refuse_text_extraction() {
    use std::sync::Arc;
    let dir = workdir("copy-restricted");
    // Print-only permissions (bit 3), copy/extract bit 5 clear, no user password to open.
    let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: 0b0100,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
    std::fs::write(dir.join("locked.pdf"), &bytes).unwrap();
    let mut a = auto(&dir);

    // Opens without a password (empty user password), but text extraction is refused.
    let id = ok(&mut a, "doc_open", json!({ "path": "locked.pdf" }))["doc"].as_u64().unwrap();
    let refused = |e: &ToolError| matches!(e, ToolError::Failed(m) if m.contains("don't allow copying"));
    assert!(refused(&a.call("text_extract", &json!({ "doc": id })).unwrap_err()), "text_extract must be blocked");
    assert!(refused(&a.call("text_find", &json!({ "doc": id, "query": "Page" })).unwrap_err()), "text_find must be blocked");
    assert!(refused(&a.call("doc_export_text", &json!({ "doc": id, "path": "out.txt" })).unwrap_err()), "doc_export_text must be blocked");
    assert!(!dir.join("out.txt").exists(), "no text file was written for a blocked export");

    // The owner password lifts the restriction.
    let oid = ok(&mut a, "doc_open", json!({ "path": "locked.pdf", "password": "owner" }))["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, oid), vec!["Page 1"], "the owner may extract text");
    let _ = std::fs::remove_dir_all(&dir);
}

/// doc_create lays out CSV tables, Markdown and email messages (PDF-XChange's CSV, Markdown and
/// EML converters); email attachments are listed unless embedding is asked for, and characters
/// the built-in fonts can't show are reported, never dropped silently.
#[test]
fn doc_create_lays_out_csv_markdown_and_email() {
    let dir = workdir("create-structured");
    std::fs::write(dir.join("ledger.csv"), "Name,Amount\nAda,3.50\n\"Babbage, C\",4\n").unwrap();
    std::fs::write(dir.join("notes.md"), "# Notes\n\nSome **bold** text and a [link](https://example.org).\n\n- one\n- two\n").unwrap();
    std::fs::write(
        dir.join("mail.eml"),
        "From: Ada <ada@example.org>\r\nTo: cb@example.org\r\nSubject: Engines\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nThe analytical engine.\r\n--b\r\nContent-Type: text/csv; name=\"t.csv\"\r\nContent-Disposition: attachment; filename=\"t.csv\"\r\n\r\na,b\r\n--b--\r\n",
    )
    .unwrap();
    let mut a = auto(&dir);

    let csv = ok(&mut a, "doc_create", json!({ "from": "csv", "path": "ledger.csv" }));
    let text = page_text(&mut a, csv["doc"].as_u64().unwrap()).join("\n");
    assert!(text.contains("Babbage, C") && text.contains("Amount") && text.contains("3.50"), "{text}");
    assert!(csv.get("notes").is_none(), "nothing was lost: {csv}");

    let md = ok(&mut a, "doc_create", json!({ "from": "markdown", "path": "notes.md" }));
    // Runs in different faces sit side by side; compare words, not exact spacing.
    let text = page_text(&mut a, md["doc"].as_u64().unwrap()).join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(text.contains("Notes") && text.contains("Some bold text and a link.") && text.contains("two"), "{text}");

    let mail = ok(&mut a, "doc_create", json!({ "from": "email", "path": "mail.eml" }));
    let id = mail["doc"].as_u64().unwrap();
    let text = page_text(&mut a, id).join("\n");
    assert!(text.contains("Engines") && text.contains("The analytical engine.") && text.contains("t.csv"), "{text}");
    assert!(mail["notes"].to_string().contains("listed but not included"), "{mail}");
    assert_eq!(ok(&mut a, "doc_info", json!({ "doc": id }))["attachments"].as_array().map(Vec::len), Some(0));
    let embedded = ok(&mut a, "doc_create", json!({ "from": "email", "path": "mail.eml", "embed_attachments": true }));
    let info = ok(&mut a, "doc_info", json!({ "doc": embedded["doc"] }));
    assert_eq!(info["attachments"][0]["name"], "t.csv", "{info}");

    let greek = ok(&mut a, "doc_create", json!({ "from": "markdown", "text": "Ω and ✓" }));
    assert!(greek["notes"].to_string().contains("written as ?"), "{greek}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
fn executable(path: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// External converters (PDF-XChange's Office2PDF and browser-to-PDF): off by default; when turned
/// on, the configured program converts and the result opens. A fake browser stands in for Chrome
/// and "prints" a prepared PDF to its --print-to-pdf target.
#[cfg(unix)]
#[test]
fn doc_create_through_external_converters() {
    let dir = workdir("external");
    std::fs::write(dir.join("page.html"), "<h1>Hi</h1>").unwrap();
    std::fs::write(dir.join("report.docx"), b"PK\x03\x04").unwrap();
    let mut off = auto(&dir).with_external_converters(pdfcraft_engine::ExternalConverters::default());
    let err = off.call("doc_create", &json!({ "from": "html", "path": "page.html" })).unwrap_err();
    assert!(err.to_string().contains("External converters are off"), "{err}");

    let prepared = dir.join("prepared.pdf");
    std::fs::write(&prepared, fixture(2)).unwrap();
    let fake = dir.join("fake-chrome");
    executable(
        &fake,
        &format!(
            "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in --print-to-pdf=*) cp '{}' \"${{a#--print-to-pdf=}}\";; esac; done\n",
            prepared.display()
        ),
    );
    let on = pdfcraft_engine::ExternalConverters { enabled: true, browser: Some(fake), office: None };
    let mut a = auto(&dir).with_external_converters(on);
    let made = ok(&mut a, "doc_create", json!({ "from": "html", "path": "page.html" }));
    assert_eq!((made["pages"].as_u64(), made["name"].as_str()), (Some(2), Some("page.pdf")), "{made}");
    // Private addresses are refused before any program runs; public pages print.
    let err = a.call("doc_create", &json!({ "from": "web_page", "url": "http://192.168.1.1/" })).unwrap_err();
    assert!(err.to_string().contains("private network"), "{err}");
    let web = ok(&mut a, "doc_create", json!({ "from": "web_page", "url": "https://example.org/" }));
    assert_eq!(web["name"], "example.org.pdf");
    // A file that doesn't match the converter asked for is refused.
    assert!(matches!(a.call("doc_create", &json!({ "from": "html", "path": "report.docx" })), Err(ToolError::InvalidArgs(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Read Out Loud sends the page text to the speech program's standard input and never returns it.
#[cfg(unix)]
#[test]
fn read_out_loud_sends_text_only_to_the_speech_program() {
    let dir = workdir("speech");
    let heard = dir.join("heard.txt");
    let fake = dir.join("espeak-ng");
    executable(&fake, &format!("#!/bin/sh\ncat > '{}'\n", heard.display()));
    let mut a = auto(&dir).with_speech_backend(pdfcraft_engine::SpeechBackend::EspeakNg(fake));
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "speech_start", json!({ "doc": doc, "pages": [2] }));
    assert_eq!((&r["speaking"], &r["truncated"], &r["pages"]), (&json!(true), &json!(false), &json!([2])), "{r}");
    assert!(!r.to_string().contains("Page 2"), "the text is not returned: {r}");
    for _ in 0..100 {
        if ok(&mut a, "speech_status", json!({}))["speaking"] == false {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(std::fs::read_to_string(&heard).unwrap().trim(), "Page 2");
    assert_eq!(ok(&mut a, "speech_stop", json!({}))["speaking"], false);
    // At most 50 pages in one go, even when asked for explicitly.
    assert!(a.call("speech_start", &json!({ "doc": doc, "pages": vec![1; 51] })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Bookmark style (PDF-XChange Bookmarks plug-in, Acrobat bookmark Properties): colour, bold and
/// italic are written as /C and /F, read back after saving, undoable, and cleared by the default.
#[test]
fn bookmark_style_sets_colour_bold_and_italic() {
    let dir = workdir("bookmark-style");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "bookmark_add", json!({ "doc": doc, "title": "Intro", "page": 1 }));
    let first = |a: &mut Automation, doc: u64| ok(a, "doc_info", json!({ "doc": doc }))["outline"][0].clone();
    ok(&mut a, "bookmark_replace_style", json!({ "doc": doc, "path": [1], "color": "#CC1A1A", "bold": true }));
    let o = first(&mut a, doc);
    assert_eq!((o["color"].as_str(), o["bold"].as_bool(), o.get("italic")), (Some("#CC1A1A"), Some(true), None), "{o}");
    ok(&mut a, "doc_save", json!({ "doc": doc, "path": "styled.pdf" }));
    let mut b = auto(&dir);
    let re = ok(&mut b, "doc_open", json!({ "path": "styled.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(first(&mut b, re)["color"], "#CC1A1A", "the style survives saving");
    ok(&mut a, "bookmark_replace_style", json!({ "doc": doc, "path": [1], "italic": true }));
    let o = first(&mut a, doc);
    assert!(o.get("color").is_none() && o.get("bold").is_none() && o["italic"] == true, "{o}");
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(first(&mut a, doc)["color"], "#CC1A1A");
    assert!(matches!(a.call("bookmark_replace_style", &json!({ "doc": doc, "path": [1], "color": "red" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(a.call("bookmark_replace_style", &json!({ "doc": doc, "path": [7] })), Err(ToolError::Failed(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

/// PDF Optimizer ▸ Discard Objects additions (PDF-XChange Optimizer parity): form submit, import
/// and reset actions, external cross-references and PDF/A identification, each only when asked.
#[test]
fn optimize_discards_form_actions_external_refs_and_standards_id() {
    use pdfcraft_cos::{Dict, Object, PdfString};
    use std::sync::Arc;
    let dir = workdir("optimize-actions");
    let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(1))).unwrap();
    let action = |kind: &str| {
        let mut a = Dict::new();
        a.set(b"S".to_vec(), Object::name(kind));
        a
    };
    let annot = |doc: &mut pdfcraft_cos::Document, subtype: &str, a: Dict| {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name(subtype));
        d.set(b"Rect".to_vec(), Object::Array(vec![10.into(), 10.into(), 50.into(), 30.into()]));
        d.set(b"A".to_vec(), Object::Dict(a));
        doc.add(d)
    };
    let mut gotor = action("GoToR");
    gotor.set(b"F".to_vec(), PdfString::literal(b"other.pdf".to_vec()));
    let mut uri = action("URI");
    uri.set(b"URI".to_vec(), PdfString::literal(b"https://example.org/".to_vec()));
    let refs = [annot(&mut doc, "Widget", action("SubmitForm")), annot(&mut doc, "Link", gotor), annot(&mut doc, "Link", uri)];
    let page = pdfcraft_model::pages(&doc).swap_remove(0).obj;
    doc.update_dict(page, |p| p.set(b"Annots".to_vec(), Object::Array(refs.iter().map(|r| Object::Ref(*r)).collect()))).unwrap();
    let mut sd = Dict::new();
    sd.set(b"Type".to_vec(), Object::name("Metadata"));
    sd.set(b"Subtype".to_vec(), Object::name("XML"));
    let xmp = br#"<x:xmpmeta><rdf:RDF><rdf:Description rdf:about="" pdfaid:part="2" pdfaid:conformance="B"/></rdf:RDF></x:xmpmeta>"#;
    let meta = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(sd, xmp.to_vec())));
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| c.set(b"Metadata".to_vec(), Object::Ref(meta))).unwrap();
    std::fs::write(dir.join("forms.pdf"), pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()).unwrap();

    let mut a = auto(&dir);
    let id = ok(&mut a, "doc_open", json!({ "path": "forms.pdf" }))["doc"].as_u64().unwrap();
    let kept = ok(&mut a, "doc_optimize", json!({ "doc": id, "path": "kept.pdf" }));
    assert_eq!(
        (kept["form_actions"].as_u64(), kept["external_refs"].as_u64(), kept["standards_ids"].as_u64()),
        (Some(0), Some(0), Some(0)),
        "off by default"
    );
    let r = ok(
        &mut a,
        "doc_optimize",
        json!({ "doc": id, "path": "clean.pdf", "discard_form_actions": true, "discard_external_refs": true, "remove_standards_id": true }),
    );
    // The link's remote go-to goes with upstream's link pass (external_links); the fork's sweep of
    // every other action then finds none (external_refs).
    assert_eq!(
        (r["form_actions"].as_u64(), r["external_links"].as_u64(), r["external_refs"].as_u64(), r["standards_ids"].as_u64()),
        (Some(1), Some(1), Some(0), Some(2)),
        "{r}"
    );
    let out = pdfcraft_cos::Document::open(Arc::new(std::fs::read(dir.join("clean.pdf")).unwrap())).unwrap();
    let kinds: Vec<String> = out
        .object_numbers()
        .into_iter()
        .filter_map(|n| out.get(pdfcraft_cos::ObjRef::new(n, out.generation(n))).as_dict().cloned())
        .filter_map(|d| {
            d.get(b"A").map(|a| out.resolve(a)).and_then(|a| a.as_dict().and_then(|a| a.name(b"S")).map(|s| String::from_utf8_lossy(s).into_owned()))
        })
        .collect();
    assert_eq!(kinds, vec!["URI".to_string()], "only the web link keeps its action");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Convert colours to grayscale (PDF-XChange Color Converter): RGB fills and strokes become gray,
/// the change is undoable, and a page with nothing to convert says so.
#[test]
fn doc_convert_gray_rewrites_colours() {
    use std::sync::Arc;
    let dir = workdir("gray");
    let mut doc = pdfcraft_cos::Document::open(Arc::new(fixture(2))).unwrap();
    let page = pdfcraft_model::pages(&doc).swap_remove(0).obj;
    let content = doc.add(pdfcraft_cos::Object::Stream(pdfcraft_cos::Stream::flate(
        pdfcraft_cos::Dict::new(),
        b"1 0 0 rg 0 0 50 50 re f 0 0 1 RG 0 0 m 50 50 l S BT /F1 12 Tf 0 1 0 rg 10 10 Td (Hi) Tj ET",
    )));
    doc.update_dict(page, |p| p.set(b"Contents".to_vec(), pdfcraft_cos::Object::Ref(content))).unwrap();
    std::fs::write(dir.join("colour.pdf"), pdfcraft_cos::write_full(&doc, &Default::default()).unwrap()).unwrap();
    let mut a = auto(&dir);
    let id = ok(&mut a, "doc_open", json!({ "path": "colour.pdf" }))["doc"].as_u64().unwrap();
    let r = ok(&mut a, "doc_convert_gray", json!({ "doc": id }));
    assert_eq!((r["colours"].as_u64(), r["left"].as_u64()), (Some(3), Some(0)), "{r}");
    ok(&mut a, "doc_save", json!({ "doc": id, "path": "gray.pdf" }));
    let out = pdfcraft_cos::Document::open(Arc::new(std::fs::read(dir.join("gray.pdf")).unwrap())).unwrap();
    let p = pdfcraft_model::pages(&out).swap_remove(0);
    let data = match &*out.resolve(p.dict.get(b"Contents").unwrap()) {
        pdfcraft_cos::Object::Stream(s) => String::from_utf8_lossy(&s.decoded().unwrap()).into_owned(),
        other => panic!("{other:?}"),
    };
    assert!(!data.contains("rg") && !data.contains("RG") && data.contains("0.3 g") && data.contains("0.11 G"), "{data}");
    ok(&mut a, "edit_undo", json!({ "doc": id }));
    assert!(matches!(a.call("doc_convert_gray", &json!({ "doc": id, "pages": [2] })), Err(ToolError::Failed(m)) if m.contains("no colours")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `doc_print` with `type3_as_image` sends the pages that draw with Type 3 fonts as pictures and
/// names them, while the other pages stay text (#2684: on by default for Windows printers only, so
/// a saved print-ready PDF keeps every page as it is unless asked).
#[test]
fn doc_print_sends_type3_pages_as_images() {
    let dir = workdir("print-type3");
    let pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 300] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /XObject << /Fm 7 0 R >> >> >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 11 0 R >> >> >>")
        .stream("", "q /Fm Do Q")
        .stream("", "BT /F1 24 Tf 20 150 Td (Page 2) Tj ET")
        .stream("/Type /XObject /Subtype /Form /BBox [0 0 200 300] /Resources << /Font << /F9 8 0 R >> >>", "BT /F9 40 Tf 30 200 Td <010F01> Tj ET")
        .obj(
            "<< /Type /Font /Subtype /Type3 /FontMatrix [0.001 0 0 0.001 0 0] /FontBBox [0 0 600 700] /FirstChar 0 /LastChar 15 \
             /Widths [0 600 0 0 0 0 0 0 0 0 0 0 0 0 0 250] /Encoding << /Differences [1 /g3B 15 /g3] >> /CharProcs << /g3B 9 0 R /g3 10 0 R >> >>",
        )
        .stream("", "600 0 0 0 600 700 d1\n0 0 600 700 re f")
        .stream("", "250 0 0 0 0 0 d1")
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .build();
    std::fs::write(dir.join("chrome.pdf"), pdf).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "chrome.pdf" }))["doc"].as_u64().unwrap();
    let pictures = |name: &str| {
        let out = pdfcraft_cos::Document::open(std::sync::Arc::new(std::fs::read(dir.join(name)).unwrap())).unwrap();
        out.scan_objects().filter(|(_, o)| matches!(&**o, pdfcraft_cos::Object::Stream(st) if st.dict.name(b"Subtype") == Some(b"Image"))).count()
    };
    let r = ok(&mut a, "doc_print", json!({ "doc": doc, "type3_as_image": true, "dpi": 100, "path": "mixed.pdf" }));
    assert_eq!(r["type3_as_image"], json!([1]), "{r}");
    assert_eq!(pictures("mixed.pdf"), 1);
    let mixed = ok(&mut a, "doc_open", json!({ "path": "mixed.pdf" }))["doc"].as_u64().unwrap();
    assert_eq!(page_text(&mut a, mixed), ["", "Page 2"]);
    let r = ok(&mut a, "doc_print", json!({ "doc": doc, "path": "plain.pdf" }));
    assert!(r.get("type3_as_image").is_none(), "{r}");
    assert_eq!(pictures("plain.pdf"), 0);
    match a.call("doc_print", &json!({ "doc": doc, "type3_as_image": true, "dpi": 20, "path": "x.pdf" })) {
        Err(ToolError::InvalidArgs(m)) => assert!(m.contains("dpi"), "{m}"),
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// `text_edit` with `keep_font` refuses, and leaves the page as it was, where the text's own font
/// can't show the new text and another would stand in; text the font can show still goes in, and
/// without `keep_font` the other font stands in as before. `F3` is a subset with glyphs for `a`
/// and `b` only, of a face no computer has.
#[test]
fn text_edit_keep_font_refuses_a_stand_in_font() {
    let dir = workdir("keep-font");
    let content = "BT /F3 12 Tf 72 700 Td (ab ba) Tj ET";
    std::fs::write(
        dir.join("subset.pdf"),
        pdf_of(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 4 0 R /Resources << /Font << /F3 5 0 R >> >> >>",
            &format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+NoSuchFace /FirstChar 97 /LastChar 99 /Widths [500 520 0] /Encoding /WinAnsiEncoding >>",
        ]),
    )
    .unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "subset.pdf" }))["doc"].as_u64().unwrap();
    for target in [json!({ "line": 1 }), json!({ "paragraph": 1 })] {
        let args = |text: &str, keep: bool| {
            let mut args = json!({ "doc": doc, "page": 1, "text": text, "keep_font": keep });
            args.as_object_mut().unwrap().extend(target.as_object().unwrap().clone());
            args
        };
        let refused = err(&mut a, "text_edit", args("cab", true));
        assert!(matches!(&refused, ToolError::Failed(m) if m.contains("keep_font")), "{target}: {refused}");
        assert_eq!(page_text(&mut a, doc), ["ab ba"], "{target}");
        ok(&mut a, "text_edit", args("ba ab", true));
        assert_eq!(page_text(&mut a, doc), ["ba ab"], "{target}");
        ok(&mut a, "text_edit", args("cab", false));
        assert_eq!(page_text(&mut a, doc), ["cab"], "{target}");
        ok(&mut a, "edit_undo", json!({ "doc": doc }));
        ok(&mut a, "edit_undo", json!({ "doc": doc }));
        assert_eq!(page_text(&mut a, doc), ["ab ba"], "{target}");
    }
    // A paragraph's formatting alone keeps its words and font. A font asked for is no stand-in,
    // but the fallback outlines drawn where the font asked for can't show the text are.
    let paragraph = |extra: Value| {
        let mut args = json!({ "doc": doc, "page": 1, "paragraph": 1, "keep_font": true });
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        args
    };
    ok(&mut a, "text_edit", paragraph(json!({ "color": "#336699" })));
    assert_eq!(page_text(&mut a, doc), ["ab ba"]);
    ok(&mut a, "text_edit", paragraph(json!({ "text": "cab", "font": "helvetica" })));
    assert_eq!(page_text(&mut a, doc), ["cab"]);
    let refused = err(&mut a, "text_edit", paragraph(json!({ "text": "αβ", "font": "helvetica" })));
    assert!(matches!(&refused, ToolError::Failed(m) if m.contains("keep_font") && m.contains("Type3")), "{refused}");
    assert_eq!(page_text(&mut a, doc), ["cab"]);
    std::fs::remove_dir_all(dir).unwrap();
}
