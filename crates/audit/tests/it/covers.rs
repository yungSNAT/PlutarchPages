//! Covers other than an opaque filled rectangle: block characters drawn over text, translucent
//! covers and covers painted through a soft mask, with copying allowed or withheld; and the audit's
//! bounds on saved graphics states and malformed boxes. Contributor-original fixtures, built in
//! code.

use std::sync::Arc;

use pdfcraft_audit::{AuditError, audit_redactions};
use pdfcraft_cos::Document;
use pdfcraft_testkit::{Pdf, stream_obj};

use crate::{BLACK_SHADING, HELVETICA, TWO_LINES, covered, page};

/// Helvetica `/F1`, the block font `/B`, the form `/Fm` (which draws blocks), a black shading
/// `/Sh`, and graphics states at opacity 0.9 (`/Most`), 0.5 (`/Half`) and 0 (`/Clear`), one that
/// sets no opacity (`/Wide`) and one with a soft mask.
const RESOURCES: &str = "<< /Font << /F1 5 0 R /B 6 0 R >> /XObject << /Fm 9 0 R >> /Shading << /Sh 10 0 R >> \
    /ExtGState << /Most << /ca 0.9 >> /Half << /ca 0.5 >> /Clear << /ca 0 >> /Wide << /LW 4 >> \
    /Masked << /SMask << /Type /Mask /S /Luminosity /G 11 0 R >> >> >> >>";

/// Objects 6 to 11 for [`RESOURCES`]: a font whose `A` is U+2588 FULL BLOCK, a full em wide, its
/// `/ToUnicode` and descriptor, a form drawing six blocks over SECRET, a black shading and a soft
/// mask's group.
fn objects() -> Vec<Vec<u8>> {
    vec![
        b"<< /Type /Font /Subtype /TrueType /BaseFont /Blocks /FirstChar 65 /LastChar 65 /Widths [1000] /FontDescriptor 8 0 R /ToUnicode 7 0 R >>"
            .to_vec(),
        stream_obj(
            "",
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Blocks def 1 begincodespacerange <00> <FF> \
             endcodespacerange 1 beginbfchar <41> <2588> endbfchar endcmap CMapName currentdict /CMap defineresource pop end end",
        ),
        b"<< /Type /FontDescriptor /FontName /Blocks /Flags 4 /FontBBox [0 -200 1000 800] /ItalicAngle 0 /Ascent 800 /Descent -200 \
          /CapHeight 700 /StemV 80 >>"
            .to_vec(),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /B 6 0 R >> >>", BLOCKS),
        BLACK_SHADING.as_bytes().to_vec(),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency /CS /DeviceGray >>", "1 g 0 0 612 792 re f"),
    ]
}

/// Six full blocks over SECRET, from its start to past its end.
const BLOCKS: &str = "BT /B 24 Tf 100 700 Td (AAAAAA) Tj ET";

/// Block characters typed over text hide it as a filled rectangle does: drawn after it, filled
/// (render mode 0 or 2) and dark against it, translucent or not. Stroked, invisible or fully
/// transparent ones, ones drawn first and white ones leave it legible. Blocks hide no text of their
/// own.
#[test]
fn block_characters_drawn_over_text_hide_it() {
    for (content, hidden) in [
        (format!("{TWO_LINES} {BLOCKS}"), true),
        (format!("{TWO_LINES} {}", BLOCKS.replace("BT ", "BT 2 Tr ")), true),
        (format!("{TWO_LINES} /Fm Do"), true),
        (format!("{TWO_LINES} q /Most gs {BLOCKS} Q"), true),
        (format!("{TWO_LINES} q /Half gs {BLOCKS} Q"), true),
        (format!("{TWO_LINES} {}", BLOCKS.replace("BT ", "BT 1 Tr ")), false),
        (format!("{TWO_LINES} {}", BLOCKS.replace("BT ", "BT 3 Tr ")), false),
        (format!("{BLOCKS} {TWO_LINES}"), false),
        (format!("{TWO_LINES} 1 g {BLOCKS}"), false),
        (format!("{TWO_LINES} q /Clear gs {BLOCKS} Q"), false),
        (format!("{BLOCKS} 0 g 100 688 220 32 re f"), false),
        (format!("{BLOCKS} {}", BLOCKS.replace("100 700", "190 700")), false),
    ] {
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&page(RESOURCES, &content, &objects())), want, "{content}");
    }
}

/// A translucent cover counts as opaque: how much it hides depends on what lies under the text,
/// which the audit doesn't read. Black at 0.5 over black text on grey leaves contrast about 2, and
/// 2 such covers one on the other let even less through. A light one never hides dark text, and a
/// fully transparent one paints nothing, until a state that sets `/ca` again. A rectangle, a
/// shading and a Square annotation alike.
#[test]
fn a_translucent_cover_counts_as_opaque() {
    for (paint, hidden) in [
        ("q /Most gs 100 688 220 32 re f Q", true),
        ("q /Most gs 100 688 220 32 re W n /Sh sh Q", true),
        ("q /Half gs 100 688 220 32 re f Q", true),
        ("q /Half gs 100 688 220 32 re W n /Sh sh Q", true),
        ("q /Most gs 1 1 0 rg 100 688 220 32 re f Q", false),
        ("q /Clear gs 100 688 220 32 re f Q", false),
        ("q /Clear gs /Wide gs 100 688 220 32 re f Q", false),
        ("q /Clear gs /Wide gs /Half gs 100 688 220 32 re f Q", true),
    ] {
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&page(RESOURCES, &format!("{TWO_LINES} {paint}"), &objects())), want, "{paint}");
    }
    let grey = format!("0.5 g 0 0 612 792 re f 0 g {TWO_LINES} q /Half gs 100 688 220 32 re f Q");
    assert_eq!(covered(&page(RESOURCES, &grey, &objects())), ["SECRET"]);
    let stacked = format!("{TWO_LINES} q /Half gs 100 688 220 32 re f 100 688 220 32 re f Q");
    assert_eq!(covered(&page(RESOURCES, &stacked, &objects())), ["SECRET", "SECRET"]);
    for (square, hidden) in [("/IC [0] /CA 0.9", true), ("/IC [0] /CA 0.5", true), ("/IC [1 1 0] /CA 0.9", false), ("/IC [0] /CA 0", false)] {
        let doc = annotated(TWO_LINES, &format!("<< /Type /Annot /Subtype /Square /Rect [100 688 320 720] {square} >>"), &[]);
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&doc), want, "{square}");
    }
}

/// Where copying is withheld, a cover is still reported, without its text, and a box over a line
/// of blocks alone hides no text, as where copying is allowed.
#[test]
fn without_copying_blocks_are_no_hidden_text() {
    let restricted = |content: &str| {
        let mut doc = page(RESOURCES, content, &objects());
        doc.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: "",
            owner_password: "owner",
            permissions: -1 ^ 0b1_0000,
            encrypt_metadata: true,
            seed: [7; 32],
        })
        .expect("encrypt");
        let bytes = Arc::new(pdfcraft_cos::write_full(&doc, &Default::default()).expect("save"));
        Document::open_with_password(bytes, None).expect("open")
    };
    assert_eq!(covered(&restricted(&format!("{TWO_LINES} {BLOCKS}"))), [""]);
    assert_eq!(covered(&restricted(&format!("{BLOCKS} 0 g 100 688 220 32 re f"))), Vec::<String>::new());
}

/// The audit doesn't read soft masks: a cover painted through one counts as it would without it.
#[test]
fn a_cover_painted_through_a_soft_mask_counts_as_unmasked() {
    let doc = page(RESOURCES, &format!("{TWO_LINES} q /Masked gs 100 688 220 32 re f Q"), &objects());
    assert_eq!(covered(&doc), ["SECRET"]);
}

/// More than 4,096 graphics states saved at once fail the audit, counted across the page and the
/// forms it draws; states a form leaves saved end with it, restored ones don't count, and an
/// annotation's appearance starts with none saved, as a viewer paints it.
#[test]
fn too_many_saved_graphics_states_fail_the_audit() {
    let audit = |content: String, form: &str| {
        let res = "<< /Font << /F1 5 0 R >> /XObject << /Fm 6 0 R >> >>";
        audit_redactions(&page(res, &content, &[stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 612 792]", form)]))
    };
    for (content, form, fails) in [
        (format!("{TWO_LINES} {}", "q ".repeat(4097)), String::new(), true),
        (format!("{} /Fm Do", "q ".repeat(2000)), "q ".repeat(2097), true),
        (format!("{TWO_LINES} {}", "q ".repeat(4096)), String::new(), false),
        (format!("{TWO_LINES} {}", "q Q ".repeat(100_000)), String::new(), false),
        ("/Fm Do /Fm Do".to_string(), "q ".repeat(3000), false),
    ] {
        let result = audit(content, &form);
        assert_eq!(matches!(result, Err(AuditError::TooManyStates)), fails, "{result:?}");
        assert!(fails || result.is_ok(), "{result:?}");
    }
    let appearance = stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", "q 1 g 0 0 10 10 re f Q");
    let doc = annotated(
        &format!("{TWO_LINES} {}", "q ".repeat(4096)),
        "<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /AP << /N 7 0 R >> >>",
        &[appearance],
    );
    let result = audit_redactions(&doc);
    assert!(result.is_ok(), "{result:?}");
}

/// A `/BBox` that is not 4 numbers is no box. A form with one clips nothing, so a shading it paints
/// fills the page; an appearance with one can't be placed, and fails the audit.
#[test]
fn a_box_of_five_numbers_is_no_box() {
    let form = stream_obj("/Type /XObject /Subtype /Form /BBox [100 688 320 720 0] /Resources << /Shading << /Sh 7 0 R >> >>", "/Sh sh");
    let doc =
        page("<< /Font << /F1 5 0 R >> /XObject << /Fm 6 0 R >> >>", &format!("{TWO_LINES} /Fm Do"), &[form, BLACK_SHADING.as_bytes().to_vec()]);
    let all = covered(&doc).concat();
    assert!(all.contains("SECRET") && all.contains("PUBLIC"), "{all}");
    let appearance = stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 10 10 0]", "0 g 0 0 10 10 re f");
    let doc = annotated(TWO_LINES, "<< /Type /Annot /Subtype /Square /Rect [100 688 320 720] /AP << /N 7 0 R >> >>", &[appearance]);
    let result = audit_redactions(&doc);
    assert!(matches!(result, Err(AuditError::BadAppearance(0))), "{result:?}");
}

/// One page of `content` (in Helvetica `/F1`) with the annotation `annot` (object 6), then
/// `objects` numbered from 7.
fn annotated(content: &str, annot: &str, objects: &[Vec<u8>]) -> Document {
    let pdf = Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>")
        .stream("", content)
        .obj(HELVETICA)
        .obj(annot)
        .objs(objects);
    Document::open(Arc::new(pdf.build())).expect("test pdf must open")
}
