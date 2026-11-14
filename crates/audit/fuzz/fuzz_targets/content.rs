//! Feeds arbitrary bytes to the redaction audit's content reading.
//!
//! An input that starts with `%PDF` is opened as a document. Any other input is the content of a
//! page, and also of the form XObject, the Type 3 glyph, the tiling pattern and the annotation
//! appearance that page's resources offer, so the fuzzer explores operators rather than file syntax.
//!
//! Results are discarded: the target checks that no input panics.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use pdfcraft_cos::Document;
use pdfcraft_testkit::Pdf;

/// The largest input the target reads (1 MiB); larger inputs are skipped.
const MAX_INPUT: usize = 1 << 20;

/// What the page offers its content, by name: Helvetica `/F1`, a Type 3 font `/T`, graphics states
/// that set a font, an alpha and a soft mask, a form `/Fm`, a tiling pattern `/P` and a shading
/// `/Sh`.
const RESOURCES: &str = "<< /Font << /F1 5 0 R /T 6 0 R >> \
    /ExtGState << /G1 << /Font [5 0 R 12] /ca 0.5 >> /G2 << /SMask << /S /Luminosity /G 8 0 R >> >> /G3 << /Font [6 0 R 10] >> >> \
    /XObject << /Fm 8 0 R >> /Pattern << /P 9 0 R >> \
    /Shading << /Sh << /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 1 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >> >> >>";

fn document(content: &[u8]) -> Vec<u8> {
    let type3 = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 7 0 R >> \
        /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] >>";
    Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources {RESOURCES} /Annots [11 0 R] >>"))
        .stream("", content)
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj(type3)
        .stream("", content)
        .stream("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency /CS /DeviceGray >>", content)
        .stream("/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10", content)
        .stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 200 50] /Resources {RESOURCES}"), content)
        .obj("<< /Type /Annot /Subtype /FreeText /Rect [100 600 500 700] /AP << /N 10 0 R >> >>")
        .build()
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT {
        return;
    }
    let bytes = if data.starts_with(b"%PDF") { data.to_vec() } else { document(data) };
    let Ok(doc) = Document::open(Arc::new(bytes)) else {
        return;
    };
    let _report = pdfcraft_audit::audit_document(&doc);
});
