// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

#[test]
fn redaction_search_reads_a_layer_off_as_the_file_opens() {
    // Layer 6 is off as the file opens and never shown.
    let content = "BT /F1 20 Tf 1 0 0 1 50 300 Tm (Shown Ada) Tj ET /OC /Off BDC BT /F1 20 Tf 1 0 0 1 50 200 Tm (Hidden Ada) Tj ET EMC";
    let bytes = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R] /D << /OFF [6 0 R] >> >> >>")
        .obj("<< /Type /Pages /Kids [4 0 R] /Count 1 >>")
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> /Properties << /Off 6 0 R >> >> >>")
        .stream("", content)
        .obj("<< /Type /OCG /Name (Hidden) >>")
        .build();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("layers.pdf", None, bytes.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h.state_mut().execute("redact.search");
    h.state_mut().redact_search.text = "Ada".into();
    h.run_steps(2);
    h.get_by_label("Mark all").click();
    h.run_steps(3);
    assert_eq!((marks(&h), h.state().redact_search.found), (2, Some(2)), "each match once, the hidden one included");
}
