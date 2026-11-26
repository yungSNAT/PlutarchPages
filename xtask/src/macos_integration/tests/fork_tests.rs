use super::{Options, args, parse_args, pdf_registration};

use crate::gates::PrivateDir;

fn make_pdf(page_count: usize, with_info: bool) -> Vec<u8> {
    use lopdf::{Dictionary, Object, Stream, dictionary};
    let mut doc = lopdf::Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for _ in 0..page_count {
        let content_id = doc.add_object(Stream::new(dictionary! {}, b"BT /F1 12 Tf (Hello) Tj ET".to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
            "Contents" => content_id,
        });
        kids.push(Object::Reference(page_id));
    }
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => page_count as i64 }));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    if with_info {
        let mut info = Dictionary::new();
        info.set("Title", Object::string_literal("Demo"));
        info.set("Author", Object::string_literal(vec![0xfe, 0xff, 0x00, b'A', 0x00, b'u']));
        info.set("Subject", Object::string_literal("x".repeat(1050)));
        let info_id = doc.add_object(info);
        doc.trailer.set("Info", info_id);
    }
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn probe_reports_keep_status_output_and_truncation_distinct() {
    assert_eq!(parse_args(&args(&["-h"])).unwrap(), Options { help: true, bundle: None, pdf: None });
    assert_eq!(parse_args(&args(&["--help"])).unwrap(), Options { help: true, bundle: None, pdf: None });
    assert!(parse_args(&args(&["--unknown"])).is_err());

    assert_eq!(pdf_registration(""), "com.adobe.pdf not declared");
    let partial = "<key>CFBundleDocumentTypes</key><dict><string>com.adobe.pdf</string></dict>";
    assert_eq!(pdf_registration(partial), "com.adobe.pdf (role=unknown role, rank=unknown rank)");

    let ok = super::super::Probe { status: Some(0), stdout: "out".into(), stderr: "err".into(), error: None };
    let fail = super::super::Probe { status: Some(2), stdout: String::new(), stderr: "bad".into(), error: None };
    let missing = super::super::Probe { status: None, stdout: "   ".into(), stderr: String::new(), error: Some("not found".into()) };
    let bare = super::super::Probe { status: None, stdout: String::new(), stderr: String::new(), error: None };

    assert_eq!(super::super::probe_status(&ok, "valid"), "valid");
    assert_eq!(super::super::probe_status(&fail, "valid"), "failed (exit 2)");
    assert_eq!(super::super::probe_status(&missing, "valid"), "unavailable (not found)");
    assert_eq!(super::super::probe_status(&bare, "valid"), "unavailable");
    assert_eq!(super::super::combined_output(&ok), "out\nerr");
    assert_eq!(super::super::combined_output(&fail), "bad");
    assert_eq!(super::super::useful_output(&ok).as_deref(), Some("out\nerr"));
    assert_eq!(super::super::useful_output(&missing), None);
    assert_eq!(super::super::bounded_text("short", 10), "short");
    assert_eq!(super::super::bounded_text("abcdef", 3), "abc…");
    assert_eq!(super::super::pdf_string(b"plain"), "plain");
    assert_eq!(super::super::pdf_string(&[0xfe, 0xff, 0x00, b'H', 0x00, b'i']), "Hi");
}

#[test]
fn bundle_report_distinguishes_missing_incomplete_and_populated_bundles() {
    let dir = PrivateDir::new("macos-bundle").unwrap();
    let mut r = String::new();
    super::super::write_bundle_report(&mut r, &dir.path().join("absent.app")).unwrap();
    assert!(r.contains("missing or not a directory"), "{r}");

    let empty_app = dir.path().join("Empty.app");
    std::fs::create_dir_all(&empty_app).unwrap();
    r.clear();
    super::super::write_bundle_report(&mut r, &empty_app).unwrap();
    assert!(r.contains("Info.plist: missing") && r.contains("none under Contents/PlugIns"), "{r}");

    let bin_app = dir.path().join("Bin.app");
    std::fs::create_dir_all(bin_app.join("Contents/PlugIns")).unwrap();
    std::fs::write(bin_app.join("Contents/Info.plist"), [0xff, 0xfe, 0x00]).unwrap();
    r.clear();
    super::super::write_bundle_report(&mut r, &bin_app).unwrap();
    assert!(r.contains("not UTF-8 XML") && r.contains("PlugIns directory is empty"), "{r}");

    let full_app = dir.path().join("Full.app");
    std::fs::create_dir_all(full_app.join("Contents/MacOS")).unwrap();
    std::fs::create_dir_all(full_app.join("Contents/PlugIns")).unwrap();
    std::fs::write(full_app.join("Contents/PlugIns/b.appex"), b"").unwrap();
    std::fs::write(full_app.join("Contents/PlugIns/a.appex"), b"").unwrap();
    std::fs::write(
        full_app.join("Contents/Info.plist"),
        "<key>CFBundleIdentifier</key><string>ai.test.app</string><key>CFBundleExecutable</key><string>MyBin</string>",
    )
    .unwrap();
    r.clear();
    super::super::write_bundle_report(&mut r, &full_app).unwrap();
    assert!(r.contains("executable missing") && r.contains("a.appex, b.appex"), "{r}");

    std::fs::write(full_app.join("Contents/MacOS/MyBin"), b"stub").unwrap();
    r.clear();
    super::super::write_bundle_report(&mut r, &full_app).unwrap();
    assert!(r.contains("architectures:"), "{r}");
    assert!(super::super::bounded_entries(&dir.path().join("no-such"), 4).is_err());

    r.clear();
    super::super::write_tool_report(&mut r, "git", "git", &["--version"]).unwrap();
    assert!(r.contains("git: available") && r.contains("output:"), "{r}");

    r.clear();
    super::super::write_tool_report(&mut r, "silent", "git", &["check-ref-format", "refs/heads/main"]).unwrap();
    assert!(r.contains("available (no output returned)"), "{r}");

    r.clear();
    super::super::write_tool_report(&mut r, "missing", "pdfcraft-uninstalled-report-test-tool", &[]).unwrap();
    assert!(r.contains("missing: unavailable"), "{r}");
}

#[test]
fn pdf_report_decodes_metadata_and_limits_page_inspection() {
    let dir = PrivateDir::new("macos-pdf").unwrap();
    let mut r = String::new();
    assert!(super::super::write_pdf_report(&mut r, &dir.path().join("missing.pdf")).is_err());

    let bad_pdf = dir.path().join("bad.pdf");
    std::fs::write(&bad_pdf, b"not a pdf").unwrap();
    assert!(super::super::write_pdf_report(&mut r, &bad_pdf).is_err());

    let zero_pdf = dir.path().join("zero.pdf");
    std::fs::write(&zero_pdf, make_pdf(0, true)).unwrap();
    r.clear();
    super::super::write_pdf_report(&mut r, &zero_pdf).unwrap();
    assert!(r.contains("Title: Demo") && r.contains("Author: Au") && r.contains("…") && r.contains("text: no pages"), "{r}");

    let one_pdf = dir.path().join("one.pdf");
    std::fs::write(&one_pdf, make_pdf(1, false)).unwrap();
    r.clear();
    super::super::write_pdf_report(&mut r, &one_pdf).unwrap();
    assert!(r.contains("pages: 1") && r.contains("Title: <absent>") && r.contains("text:"), "{r}");

    let many_pdf = dir.path().join("many.pdf");
    std::fs::write(&many_pdf, make_pdf(33, false)).unwrap();
    r.clear();
    super::super::write_pdf_report(&mut r, &many_pdf).unwrap();
    assert!(r.contains("pages: 33") && r.contains("first 32 pages inspected"), "{r}");

    super::super::run(&args(&["--help"])).unwrap();
    super::super::run(&[]).unwrap();
    let bundle_s = dir.path().to_string_lossy().into_owned();
    let pdf_s = one_pdf.to_string_lossy().into_owned();
    super::super::run(&args(&["--bundle", &bundle_s, "--pdf", &pdf_s])).unwrap();
}
