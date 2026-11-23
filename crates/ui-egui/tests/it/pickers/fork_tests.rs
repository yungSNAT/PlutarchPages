// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// Two pages; page 2 draws text in a form nested one deeper than Office export reads.
fn office_form_limit_fixture() -> Vec<u8> {
    use pdfcraft_testkit::Pdf;
    let depth = pdfcraft_engine::compare::MAX_FORM_DEPTH + 1;
    let mut pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 /MediaBox [0 0 300 300] >>")
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>")
        .stream("", "BT /F1 12 Tf 20 200 Td (Retained first page) Tj ET")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << /Font << /F1 3 0 R >> /XObject << /D1 8 0 R >> >> >>")
        .stream("", "BT /F1 12 Tf 20 200 Td (Retained second page) Tj ET /D1 Do");
    for i in 1..=depth {
        let (resources, body) = if i == depth {
            ("/Font << /F1 3 0 R >>".to_string(), "BT /F1 12 Tf 20 100 Td (Beyond depth limit) Tj ET".to_string())
        } else {
            (format!("/XObject << /D{} {} 0 R >>", i + 1, 8 + i), format!("/D{} Do", i + 1))
        };
        pdf = pdf.stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 300 300] /Resources << {resources} >>"), body);
    }
    pdf.build()
}

/// Export to HTML or RTF says which pages lost text past its reading bounds, with where it wrote
/// the file (R2128); a complete export says only where.
#[test]
fn office_export_reports_text_left_out_at_the_form_limit() {
    use pdfcraft_engine::compare::OfficeFormat;
    let dir = AttachmentDir::new();
    let original = office_form_limit_fixture();
    let mut h = harness(&[]);
    h.state_mut().open_bytes("bounded.pdf", None, original.clone()).unwrap();
    let id = h.state().views[0].id;
    for (command, format) in [("export.html", OfficeFormat::Html), ("export.rtf", OfficeFormat::Rtf)] {
        let checked = h.state().session.get(id).unwrap().export_office_checked(format);
        assert_eq!(checked.truncated_pages, [1], "only page 2 loses nested text");
        let path = dir.0.join(format!("bounded.{}", format.extension()));
        h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
        h.state_mut().toast = None;
        assert!(h.state_mut().execute(command));
        let output = std::fs::read(&path).unwrap();
        assert_eq!(output, checked.bytes);
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("Retained first page") && text.contains("Retained second page"), "{command}: {text}");
        assert!(!text.contains("Beyond depth limit"), "{command}: the reading bound must remain enforced");
        let expected =
            format!("Exported to {}. Text or images were left out on pages 2 because the document exceeds the export limits.", path.display());
        assert_eq!(h.state().toast.as_ref().map(|t| t.0.as_str()), Some(expected.as_str()), "{command}");
        let doc = h.state().session.get(id).unwrap();
        assert_eq!(doc.bytes.as_ref(), &original);
        assert!(!doc.dirty);
    }
    let mut h = harness(&[1]);
    let path = dir.0.join("complete.html");
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    assert!(h.state_mut().execute("export.html"));
    assert_eq!(h.state().toast.as_ref().map(|t| t.0.clone()), Some(format!("Exported to {}", path.display())));
}
