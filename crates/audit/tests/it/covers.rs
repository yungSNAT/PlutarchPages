//! Covers other than an opaque black filled rectangle: block characters drawn over text, images,
//! opaque covers of any colour, translucent and blended covers and covers painted through a soft
//! mask, with copying allowed or withheld; Square annotations and their appearances; and the
//! audit's bounds on saved graphics states and malformed boxes. Contributor-original fixtures,
//! built in code.

use std::sync::Arc;

use pdfcraft_audit::{AuditError, audit_redactions};
use pdfcraft_cos::Document;
use pdfcraft_testkit::{Pdf, stream_obj};

use crate::{BLACK_SHADING, HELVETICA, TWO_LINES, covered, open, page};

/// Helvetica `/F1`, the block font `/B`, the form `/Fm` (which draws blocks), a black shading
/// `/Sh`, the forms of [`objects`], and graphics states at opacity 1 (`/Full`), 0.9 (`/Most`), 0.5
/// (`/Half`), 0.2 (`/Faint`) and 0 (`/Clear`), one that sets no opacity (`/Wide`) and one with a
/// soft mask.
const RESOURCES: &str = "<< /Font << /F1 5 0 R /B 6 0 R >> \
    /XObject << /Fm 9 0 R /Im 12 0 R /Fh 13 0 R /Fp 14 0 R /Fw 15 0 R /Fg 16 0 R /Fn 17 0 R >> /Shading << /Sh 10 0 R >> \
    /ExtGState << /Full << /ca 1 >> /Most << /ca 0.9 >> /Half << /ca 0.5 >> /Faint << /ca 0.2 >> /Clear << /ca 0 >> /Wide << /LW 4 >> \
    /Masked << /SMask << /Type /Mask /S /Luminosity /G 11 0 R >> >> >> >>";

/// Objects 6 to 17 for [`RESOURCES`]: a font whose `A` is U+2588 FULL BLOCK, a full em wide, its
/// `/ToUnicode` and descriptor, a form drawing six blocks over SECRET, a black shading, a soft
/// mask's group, a black pixel with a colour key mask of its own, and forms over SECRET: a
/// transparency group filling a black box (`/Fh`), a plain form doing so (`/Fp`), a plain form
/// filling a white box at opacity 1 (`/Fw`), a group doing so (`/Fg`) and a group drawing `/Fw`
/// (`/Fn`).
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
        stream_obj("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Mask [255 255]", "\0"),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency >>", "0 g 100 688 220 32 re f"),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 612 792]", "0 g 100 688 220 32 re f"),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 612 792]", "/Full gs 1 g 100 688 220 32 re f"),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency >>", "/Full gs 1 g 100 688 220 32 re f"),
        stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency >>", "/Fw Do"),
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

/// A translucent rectangle hides text when the text and what lies around it, both seen through it,
/// differ by a contrast below 3. Black at 0.5 over black text on the white page leaves it legible
/// (about 5), on grey not (about 2), and 2 such covers one on the other let too little through for
/// the second to leave it legible. Grey at 0.2 is faint (R3368); white or yellow at 0.9 hides black
/// text, and yellow at 0.5 (a highlight) leaves it legible.
/// Where the audit can't read what lies under the text (a box over part of the line, a curve on the
/// page), the cover is judged by its colour alone, as a shading, an image, a blended cover or a
/// Square annotation is. A fully transparent one paints nothing, until a state that sets `/ca`
/// again.
#[test]
fn a_translucent_cover_is_judged_against_what_lies_under_the_text() {
    for (paint, hidden) in [
        ("q /Most gs 100 688 220 32 re f Q", true),
        ("q /Most gs 100 688 220 32 re W n /Sh sh Q", true),
        ("q /Half gs 100 688 220 32 re f Q", false),
        ("q /Half gs 100 688 220 32 re W n /Sh sh Q", true),
        ("q /Faint gs 0.3 g 100 688 220 32 re f Q", false),
        ("q /Most gs 1 g 100 688 220 32 re f Q", true),
        ("q /Half gs 1 g 100 688 220 32 re f Q", false),
        ("q /Most gs 1 1 0 rg 100 688 220 32 re f Q", true),
        ("q /Half gs 1 1 0 rg 100 688 220 32 re f Q", false),
        // A transparency group drawn translucent: its fills show with an alpha the audit does not
        // work out, whatever opacity they set, at any depth, so they are judged by colour.
        ("q /Half gs /Fh Do Q", true),
        ("q /Half gs /Fg Do Q", false),
        ("q /Half gs /Fn Do Q", false),
        // A plain form's content is painted as the page's own would be.
        ("q /Half gs /Fp Do Q", false),
        ("q /Half gs /Fw Do Q", true),
        ("q /Clear gs 100 688 220 32 re f Q", false),
        ("q /Clear gs /Wide gs 100 688 220 32 re f Q", false),
        ("q /Clear gs /Wide gs /Most gs 100 688 220 32 re f Q", true),
    ] {
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&page(RESOURCES, &format!("{TWO_LINES} {paint}"), &objects())), want, "{paint}");
    }
    let half = "q /Half gs 100 688 220 32 re f Q";
    for (under, hidden) in [
        ("0.5 g 0 0 612 792 re f 0 g", true),
        ("0.9 g 0 0 612 792 re f 0 g", false),
        ("0.5 g 100 650 40 100 re f 0 g", true),
        ("0.9 g 500 50 m 550 50 l 525 90 l f 0 g", true),
        // A stroke, or an image with a mask of its own, under the text: what lies there is not read.
        ("0 G 40 w 100 704 m 320 704 l S", true),
        ("q 220 0 0 32 100 688 cm /Im Do Q", true),
        ("q /Faint gs 0 0 612 792 re f Q", false),
    ] {
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&page(RESOURCES, &format!("{under} {TWO_LINES} {half}"), &objects())), want, "{under}");
    }
    let stacked = format!("{TWO_LINES} q /Half gs 100 688 220 32 re f 100 688 220 32 re f Q");
    assert_eq!(covered(&page(RESOURCES, &stacked, &objects())), ["SECRET"]);
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
    open(pdf.build())
}

/// The CTM that places an image's unit square where the other tests paint their cover over SECRET.
const OVER_SECRET: &str = "220 0 0 32 100 688 cm";

/// An image painted over text hides it as a filled rectangle does, over the box the CTM places it
/// in, within the clip: an image XObject, an inline image or an image mask, on the page, in a form
/// or in an annotation's appearance. One painted first, beside the text, clipped away or fully
/// transparent doesn't, nor does one with a mask of its own, which lets what lies under it show
/// where the mask says. Over invisible text an image is what an OCR tool's scan is; a rectangle
/// still hides it.
#[test]
fn an_image_painted_over_text_hides_it() {
    let resources = "<< /Font << /F1 5 0 R >> /XObject << /Im 6 0 R /Soft 7 0 R /Keyed 8 0 R /Stencil 9 0 R /Fm 10 0 R /NullMask 11 0 R >> \
                     /ExtGState << /Clear << /ca 0 >> >> >>";
    let image = |extra: &str| {
        stream_obj(&format!("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 {extra}"), "0")
    };
    let objects = [
        image(""),
        image("/SMask 6 0 R"),
        image("/Mask [0 255]"),
        stream_obj("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true", "0"),
        stream_obj(
            "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /Im 6 0 R >> >>",
            format!("q {OVER_SECRET} /Im Do Q"),
        ),
        image("/SMask 12 0 R /Mask 12 0 R"),
        b"null".to_vec(),
    ];
    let ocr = "BT 3 Tr /F1 24 Tf 100 700 Td (SECRET) Tj ET";
    for (content, hidden) in [
        (format!("{TWO_LINES} q {OVER_SECRET} /Im Do Q"), true),
        (format!("{TWO_LINES} q {OVER_SECRET} BI /W 1 /H 1 /CS /G /BPC 8 ID 0 EI Q"), true),
        (format!("{TWO_LINES} q {OVER_SECRET} BI /W 1 /H 1 /IM true ID 0 EI Q"), true),
        (format!("{TWO_LINES} q {OVER_SECRET} /Stencil Do Q"), true),
        (format!("{TWO_LINES} /Fm Do"), true),
        (format!("q {OVER_SECRET} /Im Do Q {TWO_LINES}"), false),
        (format!("{TWO_LINES} q 220 0 0 32 100 400 cm /Im Do Q"), false),
        (format!("{TWO_LINES} q 0 0 10 10 re W n {OVER_SECRET} /Im Do Q"), false),
        (format!("{TWO_LINES} q /Clear gs {OVER_SECRET} /Im Do Q"), false),
        (format!("{TWO_LINES} q {OVER_SECRET} /Soft Do Q"), false),
        (format!("{TWO_LINES} q {OVER_SECRET} /Keyed Do Q"), false),
        (format!("{TWO_LINES} q {OVER_SECRET} /NullMask Do Q"), true),
        (format!("{ocr} q {OVER_SECRET} /Im Do Q"), false),
        (format!("{ocr} 0 g 100 688 220 32 re f"), true),
    ] {
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&page(resources, &content, &objects)), want, "{content}");
    }
    let stamp = annotated(
        TWO_LINES,
        "<< /Type /Annot /Subtype /Stamp /Rect [100 688 320 720] /AP << /N 7 0 R >> >>",
        &[stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Im 8 0 R >> >>", "/Im Do"), image("")],
    );
    assert_eq!(covered(&stamp), ["SECRET"]);
}

/// A cover painted fully opaque in the Normal blend mode replaces what lies under it (ISO 32000-2
/// §11.3.5), so it hides text whatever the two colours: black over red text, white, light grey or
/// yellow over black. One blended (a highlight multiplied onto the page), painted through a soft
/// mask or as a Square's interior colour at its opacity below 1 hides it only when it is dark against
/// the text; of a list of blend modes the first counts, as PDFium reads it, and an unknown one is
/// Normal. A later graphics state or a restore that sets the Normal mode or no soft mask makes covers
/// opaque again. An annotation's `/CA` and `/BM` don't change its appearance, which PDFium and pdf.js
/// paint as it is.
#[test]
fn an_opaque_cover_hides_text_whatever_its_colour() {
    let resources = "<< /Font << /F1 5 0 R >> /ExtGState << /Multiply << /BM /Multiply >> /Plain << /BM /Normal >> \
                     /Listed << /BM [/Multiply /Normal] >> /Unknown << /BM [/Unknown /Normal] >> /Mixed << /BM [/Unknown /Multiply] >> /Odd << /BM /Unknown >> /Masked << /SMask << /Type /Mask /S /Luminosity /G 6 0 R >> >> \
                     /Unmasked << /SMask /None >> >> >>";
    let group = stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency /CS /DeviceGray >>", "1 g 0 0 612 792 re f");
    let objects = [group];
    let cover = "100 688 220 32 re f";
    for (content, hidden) in [
        (format!("BT 1 0 0 rg /F1 24 Tf 100 700 Td (SECRET) Tj ET 0 g {cover}"), true),
        (format!("{TWO_LINES} 1 g {cover}"), true),
        (format!("{TWO_LINES} 0.9 g {cover}"), true),
        (format!("{TWO_LINES} 1 1 0 rg {cover}"), true),
        (format!("{TWO_LINES} q /Plain gs 1 g {cover} Q"), true),
        (format!("{TWO_LINES} q /Multiply gs 1 1 0 rg {cover} Q"), false),
        (format!("{TWO_LINES} q /Listed gs 1 1 0 rg {cover} Q"), false),
        (format!("{TWO_LINES} q /Unknown gs 1 g {cover} Q"), true),
        (format!("{TWO_LINES} q /Mixed gs 1 g {cover} Q"), true),
        (format!("{TWO_LINES} q /Multiply gs /Odd gs 1 g {cover} Q"), true),
        (format!("{TWO_LINES} q /Multiply gs 0 g {cover} Q"), true),
        (format!("{TWO_LINES} q /Multiply gs /Plain gs 1 1 0 rg {cover} Q"), true),
        (format!("{TWO_LINES} q /Multiply gs Q 1 1 0 rg {cover}"), true),
        (format!("{TWO_LINES} q /Masked gs 1 g {cover} Q"), false),
        (format!("{TWO_LINES} q /Masked gs /Unmasked gs 1 g {cover} Q"), true),
    ] {
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&page(resources, &content, &objects)), want, "{content}");
    }
    for (square, hidden) in [("/IC [1 1 1]", true), ("/IC [1 1 0]", true), ("/IC [1 1 0] /BM /Multiply", true), ("/IC [1 1 0] /CA 0.5", false)] {
        let doc = annotated(TWO_LINES, &format!("<< /Type /Annot /Subtype /Square /Rect [100 688 320 720] {square} >>"), &[]);
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&doc), want, "{square}");
    }
    for (annot, data, hidden) in [
        ("", "1 1 0 rg 0 0 220 32 re f", true),
        ("/CA 0.4", "1 1 0 rg 0 0 220 32 re f", true),
        ("/BM /Multiply", "1 1 0 rg 0 0 220 32 re f", true),
        ("", "/M gs 1 1 0 rg 0 0 220 32 re f", false),
        ("/CA 0.4", "0 g 0 0 220 32 re f", true),
    ] {
        let ap = stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 220 32] /Resources << /ExtGState << /M << /BM /Multiply >> >> >>", data);
        let doc = annotated(TWO_LINES, &format!("<< /Type /Annot /Subtype /Highlight /Rect [100 688 320 720] {annot} /AP << /N 7 0 R >> >>"), &[ap]);
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&doc), want, "{annot} {data}");
    }
}

/// A viewer paints a Square's appearance where it has one, and makes one from its interior colour
/// only where it has none (pdf.js, PDFium, Poppler and MuPDF): a black `/IC` behind a border-only
/// appearance hides nothing. Without an appearance, or with one for a state `/AS` doesn't name, the
/// interior colour is the cover.
#[test]
fn a_squares_appearance_is_painted_instead_of_its_interior_colour() {
    let doc = |ap: &str| {
        let border = stream_obj("/Type /XObject /Subtype /Form /BBox [0 0 220 32]", "0 g 2 w 1 1 218 30 re S");
        annotated(TWO_LINES, &format!("<< /Type /Annot /Subtype /Square /Rect [100 688 320 720] /IC [0] {ap} >>"), &[border])
    };
    assert_eq!(covered(&doc("/AP << /N 7 0 R >>")), Vec::<String>::new());
    assert_eq!(covered(&doc("")), ["SECRET"]);
    assert_eq!(covered(&doc("/AP << /N << /On 7 0 R >> >>")), ["SECRET"]);
}

/// A straight stroke, horizontal or vertical on the page and solid, hides what lies under the band
/// its line width paints, as a filled rectangle over that band does: a pen drawn across text, as
/// pdf.js's ink editor saves it (a curve whose control points lie on the line). Its stroking colour
/// and opacity (`G`, `RG`, `K`, `CS` with `SC`, `/CA`) decide, not the fill's; its width is in
/// user space, set with `w` or an ExtGState's `/LW`. A path filled and stroked (`B`, `B*`, `b`,
/// `b*`) is stroked over its fill, so its stroke hides the text whatever the fill's colour. A
/// dashed one, a thin one through the text, and one drawn before the text hide nothing. A stroke
/// leaves what lies under the text unread, so a translucent one is judged by its colour against
/// the text's.
#[test]
fn a_stroke_over_text_hides_it() {
    let resources = "<< /Font << /F1 5 0 R >> /ExtGState << /Clear << /ca 0 >> /PenHalf << /CA 0.5 >> /Thick << /LW 33 >> \
                     /Dash << /D [[4 4] 0] >> /Mult << /BM /Multiply >> >> >>";
    let pen = "100 704 m 320 704 l S";
    for (paint, hidden) in [
        (format!("0 G 33 w {pen}"), true),
        ("0 G 33 w 1 J 100 704 m 100 704 320 704 320 704 c S".to_owned(), true),
        ("0 G 220 w 210 690 m 210 722 l S".to_owned(), true),
        ("q 2 0 0 2 0 0 cm 0 G 16.5 w 50 352 m 160 352 l S Q".to_owned(), true),
        (format!("1 g 0 G 33 w {pen}"), true),
        (format!("0 g 1 G 33 w {pen}"), true),
        (format!("q /Clear gs 0 G 33 w {pen} Q"), true),
        (format!("/Thick gs 0 G {pen}"), true),
        (format!("q /Mult gs 0 g 1 1 0 RG 33 w {pen} Q"), false),
        (format!("q /Mult gs 1 1 0 rg 0 G 33 w {pen} Q"), true),
        (format!("q /Mult gs 0 0 0 1 K 33 w {pen} Q"), true),
        (format!("q /Mult gs /DeviceGray CS 1 SC 33 w {pen} Q"), false),
        (format!("q /Mult gs 1 G /DeviceGray CS 33 w {pen} Q"), true),
        (format!("q /PenHalf gs 0 G 33 w {pen} Q"), true),
        (format!("q /PenHalf gs 1 1 0 RG 33 w {pen} Q"), false),
        (format!("0 G 33 w [4 4] 0 d {pen}"), false),
        (format!("/Dash gs 0 G 33 w {pen}"), false),
        ("0 G 2 w 100 708 m 320 708 l S".to_owned(), false),
        ("1 g 0 G 33 w 100 704 m 320 704 l B".to_owned(), true),
        ("1 g 0 G 33 w 100 704 m 320 704 l B*".to_owned(), true),
        ("1 g 0 G 33 w 100 704 m 320 704 l b".to_owned(), true),
        ("1 g 0 G 33 w 100 704 m 320 704 l b*".to_owned(), true),
    ] {
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&page(resources, &format!("{TWO_LINES} {paint}"), &[])), want, "{paint}");
    }
    let before = page(resources, &format!("0 G 33 w {pen} {TWO_LINES}"), &[]);
    assert!(covered(&before).is_empty(), "text painted after the stroke is on it");
}

/// A stroke is read as a cover only where the clip is a box on the page, as the box the audit
/// keeps for it then holds exactly what shows: one `re` or one four-cornered `m`/`l` path made the
/// clip, or a form's or an appearance's `/BBox` whose corners land on a box. Under any other clip,
/// a triangle, a box with a hole cut in it (by `W*`, or by `W` around a box drawn the other way),
/// or a box the CTM shears however slightly, the stroke may be clipped away from the text, so the
/// audit reads no cover from it rather than report text it can't show is hidden.
#[test]
fn a_stroke_counts_only_under_a_clip_that_is_a_box() {
    let pen = "0 G 33 w 100 704 m 320 704 l S";
    // Each sheared form or appearance shears its box, then shears back, so the stroke lands where
    // `pen` paints on the page; only the clip differs. `/Tiny` shears by a millionth and stretches
    // its box to match, so the clip is as far from a box as `/Shear`'s.
    let shear = ("/BBox [0 0 100 800] /Matrix [1 0 1 1 0 0]", format!("1 0 -1 1 0 0 cm {pen}"));
    let tiny = ("/BBox [0 0 100 800000000] /Matrix [1 0 0.000001 0.000001 0 0]", format!("1 0 -1 1000000 0 0 cm {pen}"));
    let form = |(dict, content): &(&str, String)| stream_obj(&format!("/Type /XObject /Subtype /Form {dict}"), content);
    let resources = "<< /Font << /F1 5 0 R >> /XObject << /Shear 6 0 R /Tiny 7 0 R /Plain 8 0 R >> >>";
    let forms = [form(&shear), form(&tiny), form(&("/BBox [0 0 612 792]", pen.to_owned()))];
    for (paint, hidden) in [
        (format!("q 90 600 240 200 re W n {pen} Q"), true),
        (format!("q 90 600 m 330 600 l 330 800 l 90 800 l h W n {pen} Q"), true),
        (format!("q 200 600 130 200 re W n {pen} Q"), false),
        (format!("q 100 800 m 320 800 l 320 600 l h W n {pen} Q"), false),
        (format!("q 100 800 m 320 800 l 320 600 l h W* n {pen} Q"), false),
        (format!("q 90 690 120 40 re 0 0 612 792 re W* n {pen} Q"), false),
        (format!("q 210 690 -120 40 re 0 0 612 792 re W n {pen} Q"), false),
        ("/Plain Do".to_owned(), true),
        ("/Shear Do".to_owned(), false),
        ("/Tiny Do".to_owned(), false),
    ] {
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&page(resources, &format!("{TWO_LINES} {paint}"), &forms)), want, "{paint}");
    }
    for (appearance, rect, hidden) in
        [(("/BBox [0 0 612 792]", pen.to_owned()), "[0 0 612 792]", true), (shear, "[0 0 900 800]", false), (tiny, "[0 0 900 800]", false)]
    {
        let doc = annotated(TWO_LINES, &format!("<< /Type /Annot /Subtype /Ink /Rect {rect} /AP << /N 7 0 R >> >>"), &[form(&appearance)]);
        let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
        assert_eq!(covered(&doc), want, "appearance {}", appearance.0);
    }
}

/// A fill, a shading or an image is read as a cover only where the clip is a box on the page, as a
/// stroke is, and over the part of its box within the clip: under a triangle or a box with a hole
/// it may be clipped away from the text, and a box clip beside the text leaves it showing. An image
/// the CTM shears paints no box either.
#[test]
fn a_fill_shading_or_image_counts_only_under_a_clip_that_is_a_box() {
    let resources = "<< /Font << /F1 5 0 R >> /Shading << /Sh 6 0 R >> /XObject << /Im 7 0 R >> >>";
    let objects = [
        BLACK_SHADING.as_bytes().to_vec(),
        stream_obj("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "0"),
    ];
    for painter in ["0 g 90 600 240 200 re f", "/Sh sh", "240 0 0 200 90 600 cm /Im Do"] {
        for (clip, hidden) in [
            ("90 600 240 200 re W n", true),
            ("90 600 m 330 600 l 330 800 l 90 800 l h W n", true),
            ("200 600 130 200 re W n", false),
            ("100 800 m 320 800 l 320 600 l h W n", false),
            ("100 800 m 320 800 l 320 600 l h W* n", false),
            ("90 690 120 40 re 0 0 612 792 re W* n", false),
            ("210 690 -120 40 re 0 0 612 792 re W n", false),
        ] {
            let paint = format!("q {clip} {painter} Q");
            let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
            assert_eq!(covered(&page(resources, &format!("{TWO_LINES} {paint}"), &objects)), want, "{paint}");
        }
    }
    let sheared = format!("{TWO_LINES} q 240 0 200 200 0 600 cm /Im Do Q");
    assert!(covered(&page(resources, &sheared, &objects)).is_empty(), "{sheared}");
}

/// Text shown in a render mode that clips (4 to 7) adds its glyphs to the clip at `ET`, so a stroke
/// or a fill painted after it, here one far from the glyphs, is no cover; nor after an empty text
/// object in such a mode, whose clip may be empty. Invisible text (mode 3) clips nothing, and a
/// restore brings the clip back.
#[test]
fn a_cover_under_a_text_clip_is_not_read() {
    let resources = "<< /Font << /F1 5 0 R >> >>";
    for painter in ["0 G 33 w 100 704 m 320 704 l S", "0 g 90 600 240 200 re f"] {
        for (clip, hidden) in [
            ("BT 3 Tr /F1 24 Tf 300 300 Td (X) Tj ET", true),
            ("BT 4 Tr /F1 24 Tf 300 300 Td (X) Tj ET", false),
            ("BT 7 Tr /F1 24 Tf 300 300 Td (X) Tj ET", false),
            ("7 Tr BT ET", false),
            ("q BT 7 Tr /F1 24 Tf 300 300 Td (X) Tj ET Q", true),
        ] {
            let paint = format!("{TWO_LINES} {clip} {painter}");
            let want: &[&str] = if hidden { &["SECRET"] } else { &[] };
            assert_eq!(covered(&page(resources, &paint, &[])), want, "{paint}");
        }
    }
}
