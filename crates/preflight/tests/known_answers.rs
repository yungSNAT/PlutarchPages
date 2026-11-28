//! What `verify` answers for each clause it implements, pinned as a known answer.
//!
//! Each test builds a small file by hand, from the shape the clause describes rather than from
//! anything this repository saves, so a bug shared between the verifier and the writer cannot make a
//! case pass. Two things in the file are not hand-written: the output intent profiles come from
//! `icc::srgb()`, the crate's own sRGB generator, and the encrypted document in the 6.1.3 case is
//! encrypted by the object layer, because writing an encryption dictionary by hand would restate the
//! rules under test.
//!
//! The clause strings are the verifier's own citations, pinned here as `lib.rs` writes them. They
//! have not been compared with the text of the profile yet, so a number that does not name the rule
//! it is attached to is a finding about the verifier and not about this file.
//!
//! The positive case expects no issue under the clause and the negative case expects one; both are
//! asserted by clause string, so a later change that alters an answer fails here first. Cases where
//! the verifier is known to disagree with the standard are left out on purpose and recorded as gaps
//! where the change that adds them is reviewed. Nothing here is a conformance claim.

use pdfcraft_cos::{Algorithm, CosError, Document, NewEncryption};
use pdfcraft_preflight::{ConvertError, Level, convert, declared, icc, verify};

/// A document from hand-written objects and extra trailer entries (copied from the crate's unit
/// tests).
fn build(objs: &[&str], trailer_extra: &str) -> Document {
    // ISO 19005-2 6.1.2: the header line is followed by a comment of four bytes above 127.
    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    // ISO 19005-2 6.1.3: the trailer names a file identifier; a test about another clause gets one
    // unless it gives its own.
    let id = if trailer_extra.contains("/ID") { "" } else { "/ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]" };
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {id} {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(std::sync::Arc::new(out)).unwrap()
}

/// The byte-order mark an XMP packet's `xpacket` instruction names.
const BOM: char = '\u{feff}';

/// The XMP packet identifying a file as `part`/`conformance`, with `title` as its `dc:title`.
fn xmp(part: u8, conformance: &str, title: Option<&str>) -> String {
    let title = match title {
        Some(t) => format!("   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{t}</rdf:li></rdf:Alt></dc:title>\n"),
        None => String::new(),
    };
    format!(
        r#"<?xpacket begin="{BOM}" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:pdfaid="http://www.aiim.org/pdfa/ns/id/" xmlns:dc="http://purl.org/dc/elements/1.1/">
   <pdfaid:part>{part}</pdfaid:part>
   <pdfaid:conformance>{conformance}</pdfaid:conformance>
{title}  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
    )
}

/// The XMP packet of a PDF/A-2b file, with `title` as its title when one is given.
fn a2b_packet(title: Option<&str>) -> String {
    xmp(2, "B", title)
}

/// The catalog's `/Metadata` stream object holding `packet`.
fn xmp_object(packet: &str) -> String {
    format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{packet}\nendstream", packet.len())
}

/// A content stream object holding `content`.
fn content(content: &str) -> String {
    format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
}

/// An ASCII-hex stream object holding `bytes`, with `dict` as its other entries.
fn hex_stream(dict: &str, bytes: &[u8]) -> String {
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    let data = format!("{hex}>");
    format!("<< /Filter /ASCIIHexDecode {dict} /Length {} >>\nstream\n{data}\nendstream", data.len())
}

/// A 128-byte ICC header for a four-component profile, with no `/N` so the header decides.
fn cmyk_profile_header() -> Vec<u8> {
    let mut header = vec![0u8; 128];
    header[0..4].copy_from_slice(&128u32.to_be_bytes());
    header[16..20].copy_from_slice(b"CMYK");
    header[20..24].copy_from_slice(b"XYZ ");
    header[36..40].copy_from_slice(b"acsp");
    header
}

/// A minimal valid ICC v2.1 printer CMYK profile with a 3-to-4 `B2A0` `mft2` LUT (288 bytes).
fn valid_v2_cmyk_profile() -> Vec<u8> {
    let mut out = vec![0u8; 288];
    out[0..4].copy_from_slice(&288u32.to_be_bytes());
    out[8] = 2;
    out[9] = 0x10;
    out[12..16].copy_from_slice(b"prtr");
    out[16..20].copy_from_slice(b"CMYK");
    out[20..24].copy_from_slice(b"Lab ");
    out[36..40].copy_from_slice(b"acsp");
    out[128..132].copy_from_slice(&1u32.to_be_bytes());
    out[132..136].copy_from_slice(b"B2A0");
    out[136..140].copy_from_slice(&144u32.to_be_bytes());
    out[140..144].copy_from_slice(&144u32.to_be_bytes());
    let tag = &mut out[144..288];
    tag[0..4].copy_from_slice(b"mft2");
    tag[8..11].copy_from_slice(&[3, 4, 2]);
    for off in [12, 28, 44] {
        tag[off..off + 4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    }
    tag[48..50].copy_from_slice(&2u16.to_be_bytes());
    tag[50..52].copy_from_slice(&2u16.to_be_bytes());
    for i in 0..3 {
        tag[52 + i * 4 + 2..52 + i * 4 + 4].copy_from_slice(&u16::MAX.to_be_bytes());
    }
    for i in 0..4 {
        tag[128 + i * 4 + 2..128 + i * 4 + 4].copy_from_slice(&u16::MAX.to_be_bytes());
    }
    out
}

/// A minimal valid ICC v2.1 printer grey profile with a `kTRC` gamma 2.2 curve (160 bytes).
fn valid_v2_gray_profile() -> Vec<u8> {
    let mut out = vec![0u8; 160];
    out[0..4].copy_from_slice(&160u32.to_be_bytes());
    out[8] = 2;
    out[9] = 0x10;
    out[12..16].copy_from_slice(b"prtr");
    out[16..20].copy_from_slice(b"GRAY");
    out[20..24].copy_from_slice(b"XYZ ");
    out[36..40].copy_from_slice(b"acsp");
    out[128..132].copy_from_slice(&1u32.to_be_bytes());
    out[132..136].copy_from_slice(b"kTRC");
    out[136..140].copy_from_slice(&144u32.to_be_bytes());
    out[140..144].copy_from_slice(&14u32.to_be_bytes());
    let tag = &mut out[144..158];
    tag[0..4].copy_from_slice(b"curv");
    tag[8..12].copy_from_slice(&1u32.to_be_bytes());
    tag[12..14].copy_from_slice(&0x0233u16.to_be_bytes());
    out
}

/// The catalog's `/OutputIntents`: a `GTS_PDFA1` intent naming the profile in object `n`.
fn output_intent(n: usize) -> String {
    let condition = "/OutputConditionIdentifier (sRGB IEC61966-2.1)";
    let intent = format!("<< /Type /OutputIntent /S /GTS_PDFA1 {condition} /DestOutputProfile {n} 0 R >>");
    format!("/OutputIntents [{intent}]")
}

/// An embedded font program stream object, compressed with `filter` when one is given. Its bytes
/// are not read by any rule: a font program is data, not content.
fn font_program(filter: Option<&str>) -> String {
    let filter = match filter {
        Some(filter) => format!("/Filter {filter} "),
        None => String::new(),
    };
    format!("<< /Length1 4 {filter}/Length 4 >>\nstream\nabcd\nendstream")
}

/// An image XObject that asks to be interpolated, or does not.
fn image(interpolate: bool) -> String {
    format!(
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Interpolate {interpolate} /Length 1 >>\nstream\n\u{0}\nendstream"
    )
}

/// A document whose catalog (object 1) carries `catalog_extra` and, when `packet` is given, the
/// metadata object 3; whose page tree (object 2) holds the single page `page` (object 4); and whose
/// objects 5 onwards are `more`, which `page` and `catalog_extra` refer to by number. An absent
/// `packet` leaves object 3 empty. The page must resolve, so a case that loses it fails instead of
/// passing because no rule ever reached it.
fn page_document(page: &str, packet: Option<&str>, catalog_extra: &str, more: &[&str], trailer_extra: &str) -> Document {
    let metadata = match packet {
        Some(packet) => xmp_object(packet),
        None => "null".to_owned(),
    };
    let catalog = match packet {
        Some(_) => format!("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R {catalog_extra} >>"),
        None => format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>"),
    };
    let pages = "<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 200 200] >>";
    let mut objs = vec![catalog.as_str(), pages, metadata.as_str(), page];
    objs.extend_from_slice(more);
    let doc = build(&objs, trailer_extra);
    assert_eq!(pdfcraft_model::pages(&doc).len(), 1, "the page of this case does not resolve");
    doc
}

/// Asserts that `verify` reports `clause` for `doc`.
fn assert_reported(doc: &Document, level: Level, clause: &str) {
    let issues = verify(doc, level);
    assert!(issues.iter().any(|i| i.clause == clause), "no {clause} issue in {issues:#?}");
}

/// Asserts that `verify` reports nothing under `clause` for `doc`.
fn assert_not_reported(doc: &Document, level: Level, clause: &str) {
    let found: Vec<_> = verify(doc, level).into_iter().filter(|i| i.clause == clause).collect();
    assert!(found.is_empty(), "{clause} was reported: {found:#?}");
}

#[test]
fn xmp_metadata_is_required_clause_6_6_2_1() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let identified = page_document(page, Some(packet.as_str()), "", &[], "");
    assert_not_reported(&identified, Level::A2b, "6.6.2.1");
    assert_eq!(declared(&identified).pdfa, Some((2, "B".into())));
    let bare = page_document(page, None, "", &[], "");
    assert_reported(&bare, Level::A2b, "6.6.2.1");
}

#[test]
fn the_declared_part_and_conformance_must_be_the_level_clause_6_6_4() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let matching = a2b_packet(None);
    let doc = page_document(page, Some(matching.as_str()), "", &[], "");
    assert_not_reported(&doc, Level::A2b, "6.6.4");
    for (part, conformance) in [(3, "B"), (2, "A")] {
        let packet = xmp(part, conformance, None);
        let doc = page_document(page, Some(packet.as_str()), "", &[], "");
        assert_reported(&doc, Level::A2b, "6.6.4");
        assert_eq!(declared(&doc).pdfa, Some((part, conformance.to_owned())));
    }
}

#[test]
fn the_xmp_title_must_match_the_information_dictionary_clause_6_6_3() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let info = "/Info << /Title (Report) >>";
    let matching = a2b_packet(Some("Report"));
    let doc = page_document(page, Some(matching.as_str()), "", &[], info);
    assert_not_reported(&doc, Level::A2b, "6.6.3");
    let differing = a2b_packet(Some("Draft"));
    let doc = page_document(page, Some(differing.as_str()), "", &[], info);
    assert_reported(&doc, Level::A2b, "6.6.3");
}

#[test]
fn the_trailer_must_not_name_encryption_clause_6_1_3() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let plain = page_document(page, Some(packet.as_str()), "", &[], "");
    assert_not_reported(&plain, Level::A2b, "6.1.3");
    // An encrypted file's keys are not written out by hand here, so the /Encrypt dictionary comes
    // from the object layer; the rule under test only asks whether the trailer names one.
    let mut encrypted = page_document(page, Some(packet.as_str()), "", &[], "");
    let params = NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -4,
        encrypt_metadata: true,
        seed: [7; 32],
    };
    encrypted.set_encryption(&params).unwrap();
    assert_reported(&encrypted, Level::A2b, "6.1.3");
    let report = convert(&mut encrypted, Level::A2b).unwrap();
    assert!(!report.remaining.iter().any(|i| i.clause == "6.1.3"), "{:#?}", report.remaining);
}

#[test]
fn device_colour_needs_an_output_intent_clause_6_2_3() {
    let page = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>";
    let packet = a2b_packet(None);
    let rgb = content("1 0 0 rg 10 10 20 20 re f");
    let doc = page_document(page, Some(packet.as_str()), "", &[rgb.as_str()], "");
    assert_reported(&doc, Level::A2b, "6.2.3");
    let none = content("q Q");
    let doc = page_document(page, Some(packet.as_str()), "", &[none.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.2.3");
}

#[test]
fn device_rgb_needs_an_rgb_output_intent_clause_6_2_4_3() {
    let page = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << >> >>";
    let packet = a2b_packet(None);
    let rgb = content("1 0 0 rg 10 10 20 20 re f");
    let srgb = hex_stream("/N 3", &icc::srgb());
    let doc = page_document(page, Some(packet.as_str()), &output_intent(6), &[rgb.as_str(), srgb.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.2.4.3");
    // The intent is there, so the neighbour rule about device colour without one stays quiet.
    assert_not_reported(&doc, Level::A2b, "6.2.3");
    let cmyk = hex_stream("", &cmyk_profile_header());
    let doc = page_document(page, Some(packet.as_str()), &output_intent(6), &[rgb.as_str(), cmyk.as_str()], "");
    assert_reported(&doc, Level::A2b, "6.2.4.3");
}

#[test]
fn the_catalog_must_not_have_additional_actions_clause_6_5_2() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let plain = page_document(page, Some(packet.as_str()), "", &[], "");
    assert_not_reported(&plain, Level::A2b, "6.5.2");
    let acting = page_document(page, Some(packet.as_str()), "/AA << /WC << /S /GoTo >> >>", &[], "");
    assert_reported(&acting, Level::A2b, "6.5.2");
}

#[test]
fn the_catalog_must_not_have_document_javascript_clause_6_5_1() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let plain = page_document(page, Some(packet.as_str()), "", &[], "");
    assert_not_reported(&plain, Level::A2b, "6.5.1");
    // An empty name tree is enough: an action object here would report the same clause, and this
    // case is about the catalog's own entry.
    let tree = "/Names << /JavaScript << /Names [] >> >>";
    let scripted = page_document(page, Some(packet.as_str()), tree, &[], "");
    assert_reported(&scripted, Level::A2b, "6.5.1");
}

#[test]
fn the_form_must_not_ask_viewers_for_appearances_clause_6_4_1() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let plain = page_document(page, Some(packet.as_str()), "/AcroForm << /Fields [] >>", &[], "");
    assert_not_reported(&plain, Level::A2b, "6.4.1");
    let asking = "/AcroForm << /Fields [] /NeedAppearances true >>";
    let doc = page_document(page, Some(packet.as_str()), asking, &[], "");
    assert_reported(&doc, Level::A2b, "6.4.1");
}

#[test]
fn forbidden_annotation_types_are_reported_clause_6_3_1() {
    let page = "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>";
    let packet = a2b_packet(None);
    let link = "<< /Type /Annot /Subtype /Link /Rect [10 10 30 30] /P 4 0 R /F 4 >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[link], "");
    assert_not_reported(&doc, Level::A2b, "6.3.1");
    let sound = "<< /Type /Annot /Subtype /Sound /Rect [10 10 30 30] /P 4 0 R /F 4 >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[sound], "");
    assert_reported(&doc, Level::A2b, "6.3.1");
}

#[test]
fn annotations_must_print_and_not_hide_clause_6_3_2() {
    let page = "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>";
    let packet = a2b_packet(None);
    let appearance = "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream";
    let printing = "<< /Type /Annot /Subtype /Square /Rect [10 10 30 30] /P 4 0 R /F 4 /AP << /N 6 0 R >> >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[printing, appearance], "");
    assert_not_reported(&doc, Level::A2b, "6.3.2");
    let unprinted = "<< /Type /Annot /Subtype /Square /Rect [10 10 30 30] /P 4 0 R /F 0 /AP << /N 6 0 R >> >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[unprinted, appearance], "");
    assert_reported(&doc, Level::A2b, "6.3.2");
    let hidden = "<< /Type /Annot /Subtype /Square /Rect [10 10 30 30] /P 4 0 R /F 6 /AP << /N 6 0 R >> >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[hidden, appearance], "");
    assert_reported(&doc, Level::A2b, "6.3.2");
}

#[test]
fn annotations_need_an_appearance_stream_clause_6_3_3() {
    let page = "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>";
    let packet = a2b_packet(None);
    let appearance = "<< /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Length 0 >>\nstream\n\nendstream";
    let with = "<< /Type /Annot /Subtype /Square /Rect [10 10 30 30] /P 4 0 R /F 4 /AP << /N 6 0 R >> >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[with, appearance], "");
    assert_not_reported(&doc, Level::A2b, "6.3.3");
    let without = "<< /Type /Annot /Subtype /Square /Rect [10 10 30 30] /P 4 0 R /F 4 >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[without], "");
    assert_reported(&doc, Level::A2b, "6.3.3");
}

#[test]
fn lzw_compressed_streams_are_reported_clause_6_1_7_2() {
    let page = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /Font << /F1 6 0 R >> >> >>";
    let packet = a2b_packet(None);
    let text = content("BT /F1 12 Tf 10 10 Td (Hi) Tj ET");
    let font = "<< /Type /Font /Subtype /TrueType /BaseFont /Sans /FirstChar 72 /LastChar 105 /Widths [500 500] /FontDescriptor 7 0 R >>";
    let descriptor = "<< /Type /FontDescriptor /FontName /Sans /Flags 32 /FontFile2 8 0 R >>";
    let flate = font_program(Some("/FlateDecode"));
    let doc = page_document(page, Some(packet.as_str()), "", &[text.as_str(), font, descriptor, flate.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.1.7.2");
    // The neighbour rule holds: the font still reads as embedded.
    assert_not_reported(&doc, Level::A2b, "6.2.11.4");
    let lzw = font_program(Some("/LZWDecode"));
    let doc = page_document(page, Some(packet.as_str()), "", &[text.as_str(), font, descriptor, lzw.as_str()], "");
    assert_reported(&doc, Level::A2b, "6.1.7.2");
}

#[test]
fn streams_must_not_read_external_files_clause_6_1_7_1() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(None);
    let held = "<< /Length 4 >>\nstream\nabcd\nendstream";
    let doc = page_document(page, Some(packet.as_str()), "", &[held], "");
    assert_not_reported(&doc, Level::A2b, "6.1.7.1");
    // A stream object nothing refers to: the rule looks at every stream in the file.
    let external = "<< /Length 4 /F (data.bin) >>\nstream\nabcd\nendstream";
    let doc = page_document(page, Some(packet.as_str()), "", &[external], "");
    assert_reported(&doc, Level::A2b, "6.1.7.1");
}

#[test]
fn images_must_not_be_interpolated_clause_6_2_8() {
    let page = "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>";
    let packet = a2b_packet(None);
    let drawing = content("q 10 0 0 10 0 0 cm /Im0 Do Q");
    let smooth = image(true);
    let doc = page_document(page, Some(packet.as_str()), "", &[smooth.as_str(), drawing.as_str()], "");
    assert_reported(&doc, Level::A2b, "6.2.8");
    let crisp = image(false);
    let doc = page_document(page, Some(packet.as_str()), "", &[crisp.as_str(), drawing.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.2.8");
}

#[test]
fn postscript_xobjects_are_reported_clause_6_2_9() {
    let plain = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>";
    let with = "<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Resources << /XObject << /PS0 6 0 R >> >> >>";
    let packet = a2b_packet(None);
    let drawing = content("q Q");
    let doc = page_document(plain, Some(packet.as_str()), "", &[drawing.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.2.9");
    let postscript = "<< /Type /XObject /Subtype /PS /Length 0 >>\nstream\n\nendstream";
    let doc = page_document(with, Some(packet.as_str()), "", &[drawing.as_str(), postscript], "");
    assert_reported(&doc, Level::A2b, "6.2.9");
}

#[test]
fn fonts_must_be_embedded_clause_6_2_11_4() {
    let page = "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> >>";
    let packet = a2b_packet(None);
    let embedded = "<< /Type /Font /Subtype /TrueType /BaseFont /Sans /FontDescriptor 6 0 R >>";
    let descriptor = "<< /Type /FontDescriptor /FontName /Sans /Flags 32 /FontFile2 7 0 R >>";
    let program = font_program(None);
    let doc = page_document(page, Some(packet.as_str()), "", &[embedded, descriptor, program.as_str()], "");
    assert_not_reported(&doc, Level::A2b, "6.2.11.4");
    let missing = "<< /Type /Font /Subtype /TrueType /BaseFont /Sans >>";
    let doc = page_document(page, Some(packet.as_str()), "", &[missing], "");
    assert_reported(&doc, Level::A2b, "6.2.11.4");
}

/// A file built by hand, apart from its output profile, with everything the verifier asks for: an
/// XMP packet naming PDF/A-2b with a title that matches the document information, an sRGB output
/// intent and a blank page. It checks that the rules agree with each other, which is not by itself a
/// conformance claim.
#[test]
fn minimal_document_has_no_verifier_issues() {
    let page = "<< /Type /Page /Parent 2 0 R >>";
    let packet = a2b_packet(Some("Report"));
    let profile = hex_stream("/N 3", &icc::srgb());
    let info = "/Info << /Title (Report) >>";
    let mut doc = page_document(page, Some(packet.as_str()), &output_intent(5), &[profile.as_str()], info);
    let issues = verify(&doc, Level::A2b);
    assert!(issues.is_empty(), "{issues:#?}");
    assert_eq!(declared(&doc).pdfa, Some((2, "B".into())));
    assert_eq!(declared(&doc).output_intents, ["sRGB IEC61966-2.1"]);
    let report = convert(&mut doc, Level::A2b).unwrap();
    assert!(report.remaining.is_empty(), "{:#?}", report.remaining);
    assert_eq!(declared(&doc).pdfa, Some((2, "B".into())));
}

// PDF/A-1b (ISO 19005-1 level B). Every case builds a file that is otherwise clean for PDF/A-1b
// (part 1 identification, an sRGB output intent in object 5, objects 6 onwards for the case), so
// the passing file is checked to have no issue at all, and the failing one to have its clause.

/// A page with nothing on it.
const PAGE: &str = "<< /Type /Page /Parent 2 0 R >>";

/// A PDF/A-1b file: the page `page`, `catalog_extra` in the catalog besides the output intent, and
/// `more` as objects 6 onwards.
fn a1b(page: &str, catalog_extra: &str, more: &[&str]) -> Document {
    a1b_with(&hex_stream("/N 3", &icc::srgb()), page, catalog_extra, more)
}

/// [`a1b`] with `profile` as the output intent's profile.
fn a1b_with(profile: &str, page: &str, catalog_extra: &str, more: &[&str]) -> Document {
    let catalog = format!("{} {catalog_extra}", output_intent(5));
    let mut objs = vec![profile];
    objs.extend_from_slice(more);
    page_document(page, Some(&xmp(1, "B", None)), &catalog, &objs, "")
}

/// The page, catalog entries and objects 6 onwards of one [`a1b`] file.
type Parts<'a> = (&'a str, &'a str, &'a [&'a str]);

/// One part 1 case: `good` has no issue at all and `bad` has one under `clause`.
fn a1b_case(clause: &str, good: Parts, bad: Parts) {
    let ok = a1b(good.0, good.1, good.2);
    let issues = verify(&ok, Level::A1b);
    assert!(issues.is_empty(), "the passing file of {clause} has issues: {issues:#?}");
    assert_reported(&a1b(bad.0, bad.1, bad.2), Level::A1b, clause);
}

/// A page drawing the content stream in object 6, with `resources`.
fn drawing(resources: &str) -> String {
    format!("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << {resources} >> >>")
}

/// A page listing the annotation in object 6.
const ANNOTATED: &str = "<< /Type /Page /Parent 2 0 R /Annots [6 0 R] >>";

/// An empty form XObject, with `dict` as its other entries.
fn form(dict: &str) -> String {
    format!("<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] {dict} /Length 0 >>\nstream\n\nendstream")
}

#[test]
fn level_1b_names_part_1() {
    assert_eq!(Level::from_id("PDF/A-1b"), Some(Level::A1b));
    assert_eq!(Level::from_id("1b"), Some(Level::A1b));
    assert_eq!((Level::A1b.part(), Level::A1b.label(), Level::A1b.standard()), (1, "PDF/A-1b", "ISO 19005-1:2005"));
    assert_eq!(Level::A2b.standard(), "ISO 19005-2:2011");
}

#[test]
fn part_1_identification_metadata_and_structure() {
    let ok = a1b(PAGE, "", &[]);
    assert!(verify(&ok, Level::A1b).is_empty(), "{:#?}", verify(&ok, Level::A1b));
    // The same file declared as part 2 is no PDF/A-1b file (6.7.11).
    let profile = hex_stream("/N 3", &icc::srgb());
    let part2 = page_document(PAGE, Some(&a2b_packet(None)), &output_intent(5), &[&profile], "");
    assert_reported(&part2, Level::A1b, "6.7.11");
    assert_not_reported(&part2, Level::A2b, "6.6.4");
    // 6.7.2: metadata, not compressed.
    let bare = page_document(PAGE, None, &output_intent(5), &[&profile], "");
    assert_reported(&bare, Level::A1b, "6.7.2");
    let packet = xmp(1, "B", None);
    let compressed = hex_stream("/Type /Metadata /Subtype /XML", packet.as_bytes());
    let plain = xmp_object(&packet);
    let catalog = format!("{} /Metadata 6 0 R", output_intent(5));
    let issues = verify(&page_document(PAGE, None, &catalog, &[&profile, &plain], ""), Level::A1b);
    assert!(issues.is_empty(), "{issues:#?}");
    assert_reported(&page_document(PAGE, None, &catalog, &[&profile, &compressed], ""), Level::A1b, "6.7.2");
    // 6.1.3: an identifier must be there, but part 1 doesn't ask what it holds.
    let short_id = page_document(PAGE, Some(&packet), &output_intent(5), &[&profile], "/ID [<00> <>]");
    assert_not_reported(&short_id, Level::A1b, "6.1.3");
    assert_reported(&short_id, Level::A2b, "6.1.3");
}

#[test]
fn part_1_catalog_entries() {
    a1b_case("6.1.13", (PAGE, "", &[]), (PAGE, "/OCProperties << /OCGs [] /D << >> >>", &[]));
    a1b_case("6.1.11", (PAGE, "", &[]), (PAGE, "/Names << /EmbeddedFiles << /Names [] >> >>", &[]));
    a1b_case("6.1.11", (PAGE, "", &["<< /Type /Filespec /F (a.txt) >>"]), (PAGE, "", &["<< /Type /Filespec /F (a.txt) /EF << >> >>"]));
    a1b_case("6.6.2", (PAGE, "", &[]), (PAGE, "/AA << >>", &[]));
    a1b_case("6.9", (PAGE, "/AcroForm << /Fields [] >>", &[]), (PAGE, "/AcroForm << /Fields [] /NeedAppearances true >>", &[]));
}

#[test]
fn part_1_actions_are_an_allow_list() {
    let go_to = "/OpenAction << /S /GoTo /D [4 0 R /Fit] >>";
    let hide = "/OpenAction << /S /Hide /T (x) >>";
    a1b_case("6.6.1", (PAGE, go_to, &[]), (PAGE, hide, &[]));
    a1b_case("6.6.1", (PAGE, "/OpenAction << /S /Named /N /NextPage >>", &[]), (PAGE, "/OpenAction << /S /Named /N /Print >>", &[]));
    // Part 2 allows embedded go-to actions (PDF 1.6); part 1 does not.
    let embedded = "/OpenAction << /S /GoToE /D [0 /Fit] >>";
    a1b_case("6.6.1", (PAGE, go_to, &[]), (PAGE, embedded, &[]));
    let profile = hex_stream("/N 3", &icc::srgb());
    let catalog = format!("{} {embedded}", output_intent(5));
    assert_not_reported(&page_document(PAGE, Some(&a2b_packet(None)), &catalog, &[&profile], ""), Level::A2b, "6.5.1");
    // Page additional actions are a part 2 rule only.
    let acting = a1b("<< /Type /Page /Parent 2 0 R /AA << >> >>", "", &[]);
    assert!(verify(&acting, Level::A1b).is_empty(), "{:#?}", verify(&acting, Level::A1b));
}

#[test]
fn part_1_annotations() {
    let square = |extra: &str| format!("<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] {extra} >>");
    // 6.5.2: Polygon is allowed in part 2 but is not on veraPDF's part 1 list.
    let polygon = "<< /Type /Annot /Subtype /Polygon /Rect [0 0 10 10] /F 4 >>";
    a1b_case("6.5.2", (ANNOTATED, "", &[&square("/F 4")]), (ANNOTATED, "", &[polygon]));
    // 6.5.3: printed and not hidden (no exception for any type), opaque, an appearance of N only.
    a1b_case("6.5.3", (ANNOTATED, "", &[&square("/F 4")]), (ANNOTATED, "", &[&square("")]));
    a1b_case("6.5.3", (ANNOTATED, "", &[&square("/F 4 /CA 1")]), (ANNOTATED, "", &[&square("/F 4 /CA 0.5")]));
    let ap = form("");
    let only_n = square("/F 4 /AP << /N 7 0 R >>");
    let with_d = square("/F 4 /AP << /N 7 0 R /D 7 0 R >>");
    a1b_case("6.5.3", (ANNOTATED, "", &[&only_n, &ap]), (ANNOTATED, "", &[&with_d, &ap]));
    // A colour needs an RGB output intent, which this one is.
    let coloured = a1b(ANNOTATED, "", &[&square("/F 4 /C [1 0 0]")]);
    assert!(verify(&coloured, Level::A1b).is_empty(), "{:#?}", verify(&coloured, Level::A1b));
    // Only widgets need an appearance.
    let widget = |extra: &str| format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (f) /Rect [0 0 10 10] /F 4 {extra} >>");
    let shown = widget("/AP << /N 7 0 R >>");
    a1b_case("6.9", (ANNOTATED, "", &[&shown, &ap]), (ANNOTATED, "", &[&widget(""), &ap]));
    let acting = widget("/AP << /N 7 0 R >> /A << /S /GoTo /D [4 0 R /Fit] >>");
    a1b_case("6.6.1", (ANNOTATED, "", &[&shown, &ap]), (ANNOTATED, "", &[&acting, &ap]));
    let additional = widget("/AP << /N 7 0 R >> /AA << >>");
    a1b_case("6.6.2", (ANNOTATED, "", &[&shown, &ap]), (ANNOTATED, "", &[&additional, &ap]));
    // A button's normal appearance is a dictionary of states, anything else's a stream.
    let button = "<< /Type /Annot /Subtype /Widget /FT /Btn /T (b) /Rect [0 0 10 10] /F 4 /AP << /N 7 0 R >> >>";
    a1b_case("6.5.3", (ANNOTATED, "", &[&shown, &ap]), (ANNOTATED, "", &[button, &ap]));
}

#[test]
fn part_1_transparency_is_not_allowed() {
    let gs = |state: &str| drawing(&format!("/ExtGState << /G << {state} >> >>"));
    let empty = content("");
    let opaque = gs("/CA 1 /ca 1.0 /BM /Normal /SMask /None");
    a1b_case("6.4", (&opaque, "", &[&empty]), (&gs("/CA 0.5"), "", &[&empty]));
    a1b_case("6.4", (&gs("/BM /Compatible"), "", &[&empty]), (&gs("/BM /Multiply"), "", &[&empty]));
    let masked = gs("/SMask << /S /Luminosity /G 7 0 R >>");
    a1b_case("6.4", (&gs("/SMask /None"), "", &[&empty]), (&masked, "", &[&empty, &form("")]));
    let grouped = "<< /Type /Page /Parent 2 0 R /Group << /S /Transparency >> >>";
    a1b_case("6.4", (PAGE, "", &[]), (grouped, "", &[]));
    a1b_case("6.4", (PAGE, "", &[&form("")]), (PAGE, "", &[&form("/SMask 7 0 R")]));
}

#[test]
fn part_1_streams_and_xobjects() {
    let image = |bits: u8, extra: &str| {
        format!(
            "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent {bits} {extra} /Length 2 >>\nstream\n\0\0\nendstream"
        )
    };
    a1b_case("6.2.4", (PAGE, "", &[&image(8, "")]), (PAGE, "", &[&image(16, "")]));
    a1b_case("6.2.4", (PAGE, "", &[&image(8, "")]), (PAGE, "", &[&image(8, "/Interpolate true")]));
    let mask = |bits: u8| {
        format!("<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true /BitsPerComponent {bits} /Length 1 >>\nstream\n\0\nendstream")
    };
    a1b_case("6.2.4", (PAGE, "", &[&mask(1)]), (PAGE, "", &[&mask(8)]));
    a1b_case("6.2.5", (PAGE, "", &[&form("")]), (PAGE, "", &[&form("/Subtype2 /PS")]));
    a1b_case("6.2.6", (PAGE, "", &[&form("")]), (PAGE, "", &[&form("/Ref << /F (x.pdf) /Page 0 >>")]));
    let postscript = form("").replace("/Subtype /Form", "/Subtype /PS");
    a1b_case("6.2.7", (PAGE, "", &[&form("")]), (PAGE, "", &[&postscript]));
    let lzw = content("").replace("<<", "<< /Filter /LZWDecode");
    a1b_case("6.1.10", (PAGE, "", &[&content("")]), (PAGE, "", &[&lzw]));
    let external = content("").replace("<<", "<< /FDecodeParms << >>");
    a1b_case("6.1.7", (PAGE, "", &[&content("")]), (PAGE, "", &[&external]));
}

#[test]
fn part_1_limits_are_those_of_pdf_1_4() {
    // 6.1.12: reals within ±32767, where part 2 allows ±3.403 × 10³⁸.
    let page = drawing("");
    let line = |x: &str| content(&format!("{x} 0 m 0 0 l S"));
    a1b_case("6.1.12", (&page, "", &[&line("32767")]), (&page, "", &[&line("40000.5")]));
    let profile = hex_stream("/N 3", &icc::srgb());
    let part2 = page_document(&page, Some(&a2b_packet(None)), &output_intent(5), &[&profile, &line("40000.5")], "");
    assert_not_reported(&part2, Level::A2b, "6.1.13");
    // Strings up to 65535 bytes, where part 2 stops at 32767.
    let string = |n: usize| format!("<< /S ({}) >>", "a".repeat(n));
    a1b_case("6.1.12", (PAGE, "", &[&string(40000)]), (PAGE, "", &[&string(70000)]));
}

#[test]
fn part_1_output_intent_and_colour() {
    // 6.2.2: an ICC profile below version 3.
    let mut v4 = icc::srgb();
    v4[8] = 4;
    let ok = a1b(PAGE, "", &[]);
    assert!(verify(&ok, Level::A1b).is_empty());
    assert_reported(&a1b_with(&hex_stream("/N 3", &v4), PAGE, "", &[]), Level::A1b, "6.2.2");
    // Several intents naming one profile are fine; naming two is not.
    let packet = xmp(1, "B", None);
    let profile = hex_stream("/N 3", &icc::srgb());
    let intent = |n: usize, s: &str| format!("<< /Type /OutputIntent /S /{s} /OutputConditionIdentifier (sRGB) /DestOutputProfile {n} 0 R >>");
    let same = format!("/OutputIntents [{} {}]", intent(5, "GTS_PDFA1"), intent(5, "GTS_PDFX"));
    let different = format!("/OutputIntents [{} {}]", intent(5, "GTS_PDFA1"), intent(6, "GTS_PDFX"));
    let one = page_document(PAGE, Some(&packet), &same, &[&profile, &profile], "");
    assert!(verify(&one, Level::A1b).is_empty(), "{:#?}", verify(&one, Level::A1b));
    assert_reported(&page_document(PAGE, Some(&packet), &different, &[&profile, &profile], ""), Level::A1b, "6.2.2");
    // 6.2.3.3: DeviceRGB and DeviceGray with an RGB intent, but not DeviceCMYK.
    let page = drawing("");
    let rgb = content("1 0 0 rg 0.5 g 0 0 1 1 re f");
    let cmyk = content("0 0 0 1 k 0 0 1 1 re f");
    a1b_case("6.2.3.3", (&page, "", &[&rgb]), (&page, "", &[&cmyk]));
}

#[test]
fn part_1_fonts_are_embedded() {
    let page = drawing("/Font << /F1 7 0 R >>");
    let text = content("BT /F1 12 Tf (a) Tj ET");
    let embedded = "<< /Type /Font /Subtype /Type1 /BaseFont /X /FirstChar 97 /LastChar 97 /Widths [500] /FontDescriptor 8 0 R >>";
    let descriptor = "<< /Type /FontDescriptor /FontName /X /Flags 32 /FontBBox [0 0 1 1] /ItalicAngle 0 /Ascent 1 /Descent 0 /CapHeight 1 /StemV 1 /FontFile 9 0 R >>";
    let program = font_program(None);
    let bare = "<< /Type /Font /Subtype /Type1 /BaseFont /X >>";
    a1b_case("6.3.4", (&page, "", &[&text, embedded, descriptor, &program]), (&page, "", &[&text, bare]));
}

#[test]
fn converting_to_part_1_refuses_and_changes_nothing() {
    let mut doc = a1b("<< /Type /Page /Parent 2 0 R /Group << /S /Transparency >> >>", "", &[]);
    let before = doc.bytes().to_vec();
    let err = convert(&mut doc, Level::A1b).unwrap_err();
    assert!(matches!(err, ConvertError::UnsupportedLevel(Level::A1b)));
    assert!(format!("{err:?}").contains("UnsupportedLevel"));
    assert!(err.to_string().contains("PDF/A-1b can be verified only"), "{err}");
    assert_eq!(doc.bytes().as_slice(), before.as_slice());
    let cos_err = ConvertError::from(CosError::NotPdf);
    assert!(format!("{cos_err:?}").contains("Cos"));
    assert!(cos_err.to_string().contains("not a PDF"), "{cos_err}");
}

/// `doc`'s bytes with `from` (at the start of the file) written as `to`, of the same length so
/// every offset still holds.
fn rewritten(doc: &Document, from: &[u8], to: &[u8]) -> Document {
    assert_eq!(from.len(), to.len());
    let mut bytes = doc.bytes().to_vec();
    let at = bytes.windows(from.len()).position(|w| w == from).expect("the bytes to rewrite");
    bytes[at..at + to.len()].copy_from_slice(to);
    Document::open(std::sync::Arc::new(bytes)).unwrap()
}

/// The issues `verify` reports for `doc` at 1b under `clause`, as their messages.
fn messages(doc: &Document, clause: &str) -> Vec<String> {
    verify(doc, Level::A1b).into_iter().filter(|i| i.clause == clause).map(|i| i.message).collect()
}

#[test]
fn part_1_header_and_trailer() {
    // 6.1.2: any %PDF-d.d version at the start, and a binary comment after it.
    let ok = a1b(PAGE, "", &[]);
    let version_2 = rewritten(&ok, b"%PDF-1.7", b"%PDF-2.0");
    assert!(verify(&version_2, Level::A1b).is_empty(), "{:#?}", verify(&version_2, Level::A1b));
    assert_reported(&version_2, Level::A2b, "6.1.2");
    assert_reported(&rewritten(&ok, b"%PDF-1.7", b"%PDF-1.x"), Level::A1b, "6.1.2");
    assert_reported(&rewritten(&ok, b"%\xE2\xE3\xCF\xD3", b"%abcd"), Level::A1b, "6.1.2");
    // 6.1.3: an identifier (the build helper adds none when the trailer extra names /ID), and no
    // encryption.
    let profile = hex_stream("/N 3", &icc::srgb());
    let packet = xmp(1, "B", None);
    let no_id = page_document(PAGE, Some(&packet), &output_intent(5), &[&profile], "/IDX 1");
    assert_reported(&no_id, Level::A1b, "6.1.3");
    let mut encrypted = a1b(PAGE, "", &[]);
    let params = NewEncryption {
        algorithm: Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -4,
        encrypt_metadata: true,
        seed: [7; 32],
    };
    encrypted.set_encryption(&params).unwrap();
    assert_reported(&encrypted, Level::A1b, "6.1.3");
}

#[test]
fn part_1_metadata_stream_and_identification() {
    let profile = hex_stream("/N 3", &icc::srgb());
    let packet = xmp(1, "B", None);
    // 6.7.2: the stream is marked as XML metadata.
    let untyped = format!("<< /Length {} >>\nstream\n{packet}\nendstream", packet.len());
    let catalog = format!("{} /Metadata 6 0 R", output_intent(5));
    assert_reported(&page_document(PAGE, None, &catalog, &[&profile, &untyped], ""), Level::A1b, "6.7.2");
    // 6.7.11: conformance A or B, written exactly.
    let level_a = page_document(PAGE, Some(&xmp(1, "A", None)), &output_intent(5), &[&profile], "");
    assert!(verify(&level_a, Level::A1b).is_empty(), "{:#?}", verify(&level_a, Level::A1b));
    assert_reported(&page_document(PAGE, Some(&xmp(1, "b", None)), &output_intent(5), &[&profile], ""), Level::A1b, "6.7.11");
    // 6.7.3: the title matches the information dictionary's.
    let titled = |info: &str| {
        let info = format!("<< /Title ({info}) >>");
        page_document(PAGE, Some(&xmp(1, "B", Some("A"))), &output_intent(5), &[&profile, &info], "/Info 6 0 R")
    };
    assert!(verify(&titled("A"), Level::A1b).is_empty(), "{:#?}", verify(&titled("A"), Level::A1b));
    assert_reported(&titled("B"), Level::A1b, "6.7.3");
    // 6.7.9: a predefined property with the wrong value type, under the part 1 clause.
    let ns = "xmlns:dc=\"http://purl.org/dc/elements/1.1/\"";
    let dated = packet.replacen(ns, &format!("{ns} xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\""), 1).replacen(
        "<pdfaid:part>",
        "<xmp:CreateDate>not a date</xmp:CreateDate>\n   <pdfaid:part>",
        1,
    );
    let doc = page_document(PAGE, Some(&dated), &output_intent(5), &[&profile], "");
    assert_reported(&doc, Level::A1b, "6.7.9");
    assert_not_reported(&doc, Level::A1b, "6.6.2.3.1");
}

#[test]
fn part_1_actions_in_additional_actions() {
    // 6.6.1: an action written into a page's or an annotation's /AA is checked like any other.
    let page_aa = |action: &str| format!("<< /Type /Page /Parent 2 0 R /AA << /O << {action} >> >> >>");
    a1b_case("6.6.1", (&page_aa("/S /GoTo /D [4 0 R /Fit]"), "", &[]), (&page_aa("/S /JavaScript /JS (app.alert(1))"), "", &[]));
    let link = |action: &str| format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /F 4 /AA << /E << {action} >> >> >>");
    a1b_case("6.6.1", (ANNOTATED, "", &[&link("/S /URI /URI (https://example.org)")]), (ANNOTATED, "", &[&link("/S /Launch /F (calc.exe)")]));
    // The JavaScript name tree, and a field that is no widget with additional actions (6.6.2).
    a1b_case("6.6.1", (PAGE, "", &[]), (PAGE, "/Names << /JavaScript << /Names [] >> >>", &[]));
    let field = |extra: &str| format!("<< /FT /Tx /T (f) {extra} >>");
    a1b_case("6.6.2", (PAGE, "/AcroForm << /Fields [6 0 R] >>", &[&field("")]), (PAGE, "/AcroForm << /Fields [6 0 R] >>", &[&field("/AA << >>")]));
}

#[test]
fn part_1_colour_rules() {
    // 6.2.3.3: a Default colour space covers its device space when there is no output intent.
    let packet = xmp(1, "B", None);
    let page = |spaces: &str| format!("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /ColorSpace << {spaces} >> >> >>");
    let red = content("1 0 0 rg 0 0 1 1 re f");
    let covered = page("/DefaultRGB [/CalRGB << /WhitePoint [0.9505 1 1.089] >>]");
    assert_not_reported(&page_document(&covered, Some(&packet), "", &["<< >>", &red], ""), Level::A1b, "6.2.3.3");
    let gray = content("1 0 0 rg 0.5 g 0 0 1 1 re f");
    assert_reported(&page_document(&covered, Some(&packet), "", &["<< >>", &gray], ""), Level::A1b, "6.2.3.3");
    assert_reported(&page_document(&page(""), Some(&packet), "", &["<< >>", &red], ""), Level::A1b, "6.2.3.3");
    // 6.2.3.4 is a part 2 rule only: 2 Separation spaces of one name may differ.
    let tint = |c1: &str| format!("<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [{c1}] /N 1 >>");
    let spots = page("/CS1 [/Separation /Spot /DeviceRGB 7 0 R] /CS2 [/Separation /Spot /DeviceRGB 8 0 R]");
    let doc = a1b(&spots, "", &[&content(""), &tint("1 0 0"), &tint("0 1 0")]);
    assert!(verify(&doc, Level::A1b).is_empty(), "{:#?}", verify(&doc, Level::A1b));
    // 6.2.2: the profile is for a printer or a display, in grey, RGB or CMYK.
    let mut scanner = icc::srgb();
    scanner[12..16].copy_from_slice(b"scnr");
    assert_reported(&a1b_with(&hex_stream("/N 3", &scanner), PAGE, "", &[]), Level::A1b, "6.2.2");
    let mut lab = icc::srgb();
    lab[16..20].copy_from_slice(b"Lab ");
    assert_reported(&a1b_with(&hex_stream("/N 3", &lab), PAGE, "", &[]), Level::A1b, "6.2.2");
}

#[test]
fn part_1_graphics_states_and_xobjects() {
    let gs = |state: &str| drawing(&format!("/ExtGState << /G << {state} >> >>"));
    let empty = content("");
    // 6.4: an alpha of 1 within a real's precision is 1; a blend mode must be a name.
    a1b_case("6.4", (&gs("/CA 1.0000001 /ca 0.9999999"), "", &[&empty]), (&gs("/CA 0.99"), "", &[&empty]));
    let odd = a1b(&gs("/BM 3"), "", &[&empty]);
    assert_eq!(messages(&odd, "6.4"), ["A graphics state has a blend mode that isn't a name"]);
    let array_bm = a1b(&gs("/BM [/Multiply /Multiply /Screen]"), "", &[&empty]);
    assert_eq!(messages(&array_bm, "6.4"), ["A graphics state has a blend mode that isn't a name"]);
    let repeated = a1b(&drawing("/ExtGState << /G1 << /BM /Multiply >> /G2 << /BM /Multiply >> /G3 << /BM /Screen >> >>"), "", &[&empty]);
    assert_eq!(messages(&repeated, "6.4").len(), 2, "{:#?}", messages(&repeated, "6.4"));
    a1b_case("6.4", (PAGE, "", &[&form("")]), (PAGE, "", &[&form("/Group << /S /Transparency >>")]));
    // 6.2.8 and 6.2.9: no transfer functions, standard rendering intents.
    a1b_case("6.2.8", (&gs("/TR2 /Default"), "", &[&empty]), (&gs("/TR /Identity"), "", &[&empty]));
    a1b_case("6.2.8", (&gs("/TR2 /Default"), "", &[&empty]), (&gs("/TR2 /Identity"), "", &[&empty]));
    a1b_case("6.2.9", (&gs("/RI /Perceptual"), "", &[&empty]), (&gs("/RI /Vivid"), "", &[&empty]));
    // 6.2.4: no Alternates on an image, no OPI on an image or a form.
    let image =
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\0\nendstream";
    a1b_case("6.2.4", (PAGE, "", &[image]), (PAGE, "", &[&image.replace("/Length", "/Alternates [] /Length")]));
    a1b_case("6.2.4", (PAGE, "", &[&form("")]), (PAGE, "", &[&form("/OPI << >>")]));
    // 6.1.12: integers within 32 bits, names up to 127 bytes.
    a1b_case("6.1.12", (PAGE, "", &["<< /K 2147483647 >>"]), (PAGE, "", &["<< /K 3000000000 >>"]));
    let name = |n: usize| format!("<< /K /{} >>", "a".repeat(n));
    a1b_case("6.1.12", (PAGE, "", &[&name(127)]), (PAGE, "", &[&name(128)]));
}

#[test]
fn part_1_annotation_colour_and_appearances() {
    // 6.5.3: a colour needs an RGB output intent, which a CMYK one is not.
    let square = "<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 4 /C [1 0 0] >>";
    let cmyk = a1b_with(&hex_stream("/N 4", &cmyk_profile_header()), ANNOTATED, "", &[square]);
    assert_reported(&cmyk, Level::A1b, "6.5.3");
    // A field other than a button has a stream as its normal appearance.
    let ap = form("");
    let text = |n: &str| format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (f) /Rect [0 0 10 10] /F 4 /AP << /N {n} >> >>");
    a1b_case("6.5.3", (ANNOTATED, "", &[&text("7 0 R"), &ap]), (ANNOTATED, "", &[&text("<< /On 7 0 R >>"), &ap]));
    // An annotation with no type is reported as such.
    let untyped = a1b(ANNOTATED, "", &["<< /Type /Annot /Rect [0 0 10 10] /F 4 >>"]);
    assert_eq!(messages(&untyped, "6.5.2"), ["An annotation has no type (Subtype)"]);
}

#[test]
fn part_1_a_default_space_covers_only_pages_that_define_it() {
    // 2 pages drawing DeviceRGB with no output intent: covered when both define DefaultRGB, not
    // when only the first does.
    let packet = xmp(1, "B", None);
    let metadata = xmp_object(&packet);
    let page = |spaces: &str| format!("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /ColorSpace << {spaces} >> >> >>");
    let default = "/DefaultRGB [/CalRGB << /WhitePoint [0.9505 1 1.089] >>]";
    let red = content("1 0 0 rg 0 0 1 1 re f");
    let two = |second: &str| {
        let catalog = "<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>";
        let pages = "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /MediaBox [0 0 200 200] >>";
        build(&[catalog, pages, &metadata, &page(default), &page(second), &red], "")
    };
    assert_not_reported(&two(default), Level::A1b, "6.2.3.3");
    assert_reported(&two(""), Level::A1b, "6.2.3.3");
}

/// A part 1 file with no output intent: the page `page` and `more` as objects 6 onwards (object 5
/// is empty).
fn a1b_no_intent(page: &str, more: &[&str]) -> Document {
    let mut objs = vec!["<< >>"];
    objs.extend_from_slice(more);
    page_document(page, Some(&xmp(1, "B", None)), "", &objs, "")
}

/// A form XObject drawing `ops`, with `dict` as its other entries.
fn drawing_form(dict: &str, ops: &str) -> String {
    format!("<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] {dict} /Length {} >>\nstream\n{ops}\nendstream", ops.len())
}

/// A colour space that is no device space.
const CAL_RGB: &str = "[/CalRGB << /WhitePoint [0.9505 1 1.089] >>]";

/// Red, drawn.
const RED: &str = "1 0 0 rg 0 0 1 1 re f";

/// A page drawing object 6 with `spaces` in its `/ColorSpace` and `more` in its resources.
fn coloured_page(spaces: &str, more: &str) -> String {
    format!("<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /ColorSpace << {spaces} >> {more} >> >>")
}

#[test]
fn part_1_a_default_space_covers_only_the_content_drawn_under_it() {
    // 6.2.3.3 with no output intent: a form draws under its own resources, not the page's.
    let default = format!("/DefaultRGB {CAL_RGB}");
    let page = coloured_page(&default, "/XObject << /Fm 7 0 R >>");
    let draw = content("/Fm Do");
    let own = drawing_form(&format!("/Resources << /ColorSpace << {default} >> >>"), RED);
    assert_not_reported(&a1b_no_intent(&page, &[&draw, &own]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&page, &[&draw, &drawing_form("/Resources << >>", RED)]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&page, &[&draw, &drawing_form("", RED)]), Level::A1b, "6.2.3.3");
    // So does an annotation's appearance stream.
    let annotated = format!("<< /Type /Page /Parent 2 0 R /Annots [6 0 R] /Resources << /ColorSpace << {default} >> >> >>");
    let square = "<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 4 /AP << /N 7 0 R >> >>";
    assert_not_reported(&a1b_no_intent(&annotated, &[square, &own]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&annotated, &[square, &drawing_form("/Resources << >>", RED)]), Level::A1b, "6.2.3.3");
    // And a Type 3 font's glyphs, under the font's resources.
    let text = content("BT /F1 1 Tf (a) Tj ET");
    let type3 = |resources: &str| {
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 8 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] {resources} >>"
        )
    };
    let glyph = content(RED);
    let font_page = coloured_page("", "/Font << /F1 7 0 R >>");
    let covered = type3(&format!("/Resources << /ColorSpace << {default} >> >>"));
    assert_not_reported(&a1b_no_intent(&font_page, &[&text, &covered, &glyph]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&font_page, &[&text, &type3(""), &glyph]), Level::A1b, "6.2.3.3");
}

#[test]
fn part_1_a_default_space_must_be_a_colour_space() {
    // A Default entry covers only as a device-independent colour space: not as no colour space, a
    // device space, or a space that draws in a device space of its own.
    let red = content(RED);
    let space = |value: &str| coloured_page(&format!("/DefaultRGB {value}"), "");
    assert_not_reported(&a1b_no_intent(&space(CAL_RGB), &[&red]), Level::A1b, "6.2.3.3");
    assert_not_reported(&a1b_no_intent(&space("7 0 R"), &[&red, CAL_RGB]), Level::A1b, "6.2.3.3");
    let profile = hex_stream("/N 3", &icc::srgb());
    assert_not_reported(&a1b_no_intent(&space("[/ICCBased 7 0 R]"), &[&red, &profile]), Level::A1b, "6.2.3.3");
    let spaces = [
        "null",
        "99 0 R",
        "/DeviceRGB",
        "5",
        "[]",
        "[/CalRGB]",
        "[/DeviceRGB]",
        "[/Indexed /DeviceRGB 0 <000000>]",
        "[/Separation /X /DeviceRGB 7 0 R]",
        "[/DeviceN [/X /Y /Z] /DeviceRGB 7 0 R]",
        "[/Pattern]",
        "[/Foo]",
    ];
    for value in spaces {
        assert_reported(&a1b_no_intent(&space(value), &[&red]), Level::A1b, "6.2.3.3");
    }
    // A colour space a resource dictionary names is used under that dictionary.
    let indexed = format!("/DefaultRGB {CAL_RGB} /CS0 [/Indexed /DeviceRGB 0 <000000>]");
    assert_not_reported(&a1b_no_intent(&coloured_page(&indexed, ""), &[&content("")]), Level::A1b, "6.2.3.3");
    let bare = "/CS0 [/Indexed /DeviceRGB 0 <000000>]";
    assert_reported(&a1b_no_intent(&coloured_page(bare, ""), &[&content("")]), Level::A1b, "6.2.3.3");
    // So are the colour spaces of shadings written into its /Shading entries or into its shading
    // patterns, whether the pattern is an object of its own or not.
    let tint = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >>";
    let shading = "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function 7 0 R >>";
    let shaded = |spaces: &str| coloured_page(spaces, &format!("/Shading << /Sh {shading} >>"));
    let sh = content("/Sh sh");
    assert_not_reported(&a1b_no_intent(&shaded(&format!("/DefaultRGB {CAL_RGB}")), &[&sh, tint]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&shaded(""), &[&sh, tint]), Level::A1b, "6.2.3.3");
    let pattern = format!("<< /PatternType 2 /Shading {shading} >>");
    let fill = content("/Pattern cs /P0 scn 0 0 1 1 re f");
    assert_reported(&a1b_no_intent(&coloured_page("", &format!("/Pattern << /P0 {pattern} >>")), &[&fill, tint]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&coloured_page("", "/Pattern << /P0 8 0 R >>"), &[&fill, tint, &pattern]), Level::A1b, "6.2.3.3");
    // A shading of its own object is judged where it is named, and a tiling pattern has none.
    let named = coloured_page(&format!("/DefaultRGB {CAL_RGB}"), "/Shading << /Sh 8 0 R >> /Pattern << /T 9 0 R >>");
    let tiling = "<< /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 1 1] /XStep 1 /YStep 1 /Resources << >> /Length 0 >>\nstream\n\nendstream";
    assert_not_reported(&a1b_no_intent(&named, &[&sh, tint, shading, tiling]), Level::A1b, "6.2.3.3");
    // An image's colour space is used under each resource dictionary that names it.
    let image =
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 3 >>\nstream\n\0\0\0\nendstream";
    let default = format!("/DefaultRGB {CAL_RGB}");
    let page = coloured_page(&default, "/XObject << /Im 7 0 R >>");
    let draw = content("/Im Do");
    assert_not_reported(&a1b_no_intent(&page, &[&draw, image]), Level::A1b, "6.2.3.3");
    // A form with empty resources elsewhere (an appearance that draws nothing) changes nothing.
    let annotated = format!(
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Annots [8 0 R] /Resources << /ColorSpace << {default} >> /XObject << /Im 7 0 R >> >> >>"
    );
    let link = "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /F 4 /AP << /N 9 0 R >> >>";
    let blank = drawing_form("/Resources << >>", "");
    assert_not_reported(&a1b_no_intent(&annotated, &[&draw, image, link, &blank]), Level::A1b, "6.2.3.3");
    // But a form with no XObjects of its own that draws one may draw the page's, under its own
    // resources.
    let both = coloured_page(&default, "/XObject << /Im 7 0 R /Fm 8 0 R >>");
    let fallback = drawing_form("/Resources << >>", "/Im Do");
    assert_reported(&a1b_no_intent(&both, &[&content("/Fm Do"), image, &fallback]), Level::A1b, "6.2.3.3");
    // An image only a form names is drawn under the form's resources.
    let outer = coloured_page("", "/XObject << /Fm 8 0 R >>");
    let naming = |spaces: &str| drawing_form(&format!("/Resources << /ColorSpace << {spaces} >> /XObject << /Im 7 0 R >> >>"), "/Im Do");
    assert_not_reported(&a1b_no_intent(&outer, &[&content("/Fm Do"), image, &naming(&default)]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&outer, &[&content("/Fm Do"), image, &naming("")]), Level::A1b, "6.2.3.3");
    // One that nothing names is judged under every resource dictionary content draws under.
    assert_not_reported(&a1b_no_intent(&coloured_page(&default, ""), &[&content(""), image]), Level::A1b, "6.2.3.3");
    // With an RGB intent, DeviceCMYK in a form needs the form's own valid four-component DefaultCMYK.
    let cmyk = "0 0 0 1 k 0 0 1 1 re f";
    let cmyk_profile = hex_stream("/N 4", &valid_v2_cmyk_profile());
    let cmyk_default = "/DefaultCMYK [/ICCBased 8 0 R]";
    let page = coloured_page(cmyk_default, "/XObject << /Fm 7 0 R >>");
    let draw = content("/Fm Do");
    let own = drawing_form(&format!("/Resources << /ColorSpace << {cmyk_default} >> >>"), cmyk);
    a1b_case("6.2.3.3", (&page, "", &[&draw, &own, &cmyk_profile]), (&page, "", &[&draw, &drawing_form("/Resources << >>", cmyk), &cmyk_profile]));
    // Content that cannot be read (here, with a filter no reader has) may draw in any space.
    let broken = content(RED).replace("<<", "<< /Filter /NoSuchDecode");
    a1b_case("6.2.3.3", (&drawing(""), "", &[&content("")]), (&drawing(""), "", &[&broken]));
    assert!(messages(&a1b(&drawing(""), "", &[&broken]), "6.2.3.3").contains(&"Some content could not be read for device colour".to_owned()));
    // DeviceRGB with a CMYK intent.
    let doc = a1b_with(&hex_stream("/N 4", &cmyk_profile_header()), &drawing(""), "", &[&content(RED)]);
    assert!(messages(&doc, "6.2.3.3").contains(&"DeviceRGB is used, but the PDF/A output intent isn't an RGB profile".to_owned()));
}

#[test]
fn part_1_referenced_actions_are_checked_whatever_they_hold() {
    // 6.6.1: an action an action entry points at is checked, with /P and /K or without.
    let js = "<< /S /JavaScript /JS (app.alert(1)) /P 4 0 R /K 0 >>";
    let go_to = "<< /S /GoTo /D [4 0 R /Fit] /P 4 0 R /K 0 >>";
    a1b_case("6.6.1", (PAGE, "/OpenAction 6 0 R", &[go_to]), (PAGE, "/OpenAction 6 0 R", &[js]));
    let page_aa = "<< /Type /Page /Parent 2 0 R /AA << /O 6 0 R >> >>";
    a1b_case("6.6.1", (page_aa, "", &[go_to]), (page_aa, "", &[js]));
    let link = "<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /F 4 /A 7 0 R >>";
    let uri = "<< /S /URI /URI (https://example.org) /P 4 0 R /K 0 >>";
    let launch = "<< /S /Launch /F (calc.exe) /P 4 0 R /K 0 >>";
    a1b_case("6.6.1", (ANNOTATED, "", &[link, uri]), (ANNOTATED, "", &[link, launch]));
    // A dictionary with /S that no action entry points at is no action: a page label, a border
    // style, a structure element.
    let label = a1b(PAGE, "/PageLabels << /Nums [0 6 0 R] >>", &["<< /S /D >>"]);
    let bordered = a1b(ANNOTATED, "", &["<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 4 /BS 7 0 R >>", "<< /W 1 /S /S >>"]);
    let element = a1b(PAGE, "", &["<< /S /P /P 7 0 R >>", "<< /Type /StructTreeRoot >>"]);
    for doc in [label, bordered, element] {
        assert!(verify(&doc, Level::A1b).is_empty(), "{:#?}", verify(&doc, Level::A1b));
    }
}

#[test]
fn part_1_a_shared_object_is_read_in_each_of_its_roles() {
    // Object 7 is one page's resource dictionary and the other page's ExtGState dictionary, whose
    // /G the second page draws with; whichever page comes first, /G is checked (6.4).
    let packet = xmp(1, "B", None);
    let metadata = xmp_object(&packet);
    let profile = hex_stream("/N 3", &icc::srgb());
    let catalog = format!("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R {} >>", output_intent(6));
    let pages = "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /MediaBox [0 0 200 200] >>";
    let as_resources = "<< /Type /Page /Parent 2 0 R /Resources 7 0 R >>";
    let as_states = "<< /Type /Page /Parent 2 0 R /Contents 8 0 R /Resources << /ExtGState 7 0 R >> >>";
    let draw = content("/G gs");
    for (fourth, fifth) in [(as_resources, as_states), (as_states, as_resources)] {
        let doc = build(&[&catalog, pages, &metadata, fourth, fifth, &profile, "<< /G << /CA 0.5 >> >>", &draw], "");
        assert_eq!(pdfcraft_model::pages(&doc).len(), 2);
        assert_reported(&doc, Level::A1b, "6.4");
    }
}

#[test]
fn part_1_reaches_every_case_of_its_rules() {
    // 6.5.2: an annotation written straight into a page's list.
    let direct = |annot: &str| format!("<< /Type /Page /Parent 2 0 R /Annots [<< /Type /Annot /Subtype /{annot} /Rect [0 0 10 10] /F 4 >>] >>");
    a1b_case("6.5.2", (&direct("Square"), "", &[]), (&direct("Polygon"), "", &[]));
    // 6.2.9: a rendering intent that is no name.
    let gs = |state: &str| drawing(&format!("/ExtGState << /G {state} >>"));
    let empty = content("");
    // A state named twice is read once, and an entry that is no dictionary is no state.
    let twice = drawing("/ExtGState << /G 7 0 R /H 7 0 R /I 5 >>");
    a1b_case("6.4", (&twice, "", &[&empty, "<< /CA 1 >>"]), (&twice, "", &[&empty, "<< /CA 0.5 >>"]));
    // 6.2.3.3: a page's contents given as a reference to an array of streams.
    let listed = "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Resources << >> >>";
    a1b_case("6.2.3.3", (listed, "", &[&content(RED), "[6 0 R]"]), (listed, "", &[&content("0 0 0 1 k"), "[6 0 R]"]));
    // 6.5.3: an appearance dictionary with no normal appearance.
    let only_d = a1b(ANNOTATED, "", &["<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 4 /AP << /D 7 0 R >> >>", &form("")]);
    assert_eq!(
        messages(&only_d, "6.5.3"),
        [
            "A Square annotation's appearance dictionary holds entries other than N",
            "A Square annotation's appearance dictionary has no normal appearance (N)",
        ]
    );
    // Part 2 words a drawn font with no name of its own.
    let page = drawing("/Font << /F1 7 0 R >>");
    let profile = hex_stream("/N 3", &icc::srgb());
    let part2 = page_document(
        &page,
        Some(&a2b_packet(None)),
        &output_intent(5),
        &[&profile, &content("BT /F1 12 Tf (a) Tj ET"), "<< /Type /Font /Subtype /Type1 >>"],
        "",
    );
    let said: Vec<String> = verify(&part2, Level::A2b).into_iter().filter(|i| i.clause == "6.2.11.4").map(|i| i.message).collect();
    assert_eq!(said, ["A font isn't embedded"]);
    assert_eq!(messages(&a1b(&gs("<< /RI 5 >>"), "", &[&empty]), "6.2.9"), ["A graphics state has a rendering intent that isn't a name"]);
    // 6.4: a graphics state of its own object, and an image with a soft mask.
    a1b_case("6.4", (&gs("7 0 R"), "", &[&empty, "<< /Type /ExtGState /CA 1 >>"]), (&gs("7 0 R"), "", &[&empty, "<< /Type /ExtGState /CA 0.5 >>"]));
    let image =
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\0\nendstream";
    let masked = image.replace("/Length", "/SMask 7 0 R /Length");
    a1b_case("6.4", (PAGE, "", &[image, image]), (PAGE, "", &[&masked, image]));
    // 6.1.12: a CMap mapping to a CID above 65535.
    let font = "<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding 7 0 R /DescendantFonts [] >>";
    let cmap = |cid: u32| content(&format!("1 begincidrange <00> <00> {cid} endcidrange"));
    a1b_case("6.1.12", (PAGE, "", &[font, &cmap(65535)]), (PAGE, "", &[font, &cmap(65536)]));
    // 6.3.4: a drawn font with no name.
    let page = drawing("/Font << /F1 7 0 R >>");
    let text = content("BT /F1 12 Tf (a) Tj ET");
    assert_eq!(messages(&a1b(&page, "", &[&text, "<< /Type /Font /Subtype /Type1 >>"]), "6.3.4"), ["A font isn't embedded"]);
    // 6.2.5: a form with a /PS entry.
    a1b_case("6.2.5", (PAGE, "", &[&form(""), &empty]), (PAGE, "", &[&form("/PS 7 0 R"), &empty]));
    // 6.5.3: a normal appearance that is neither states nor a stream.
    let square = |n: &str| format!("<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 4 /AP << /N {n} >> >>");
    let ap = form("");
    a1b_case("6.5.3", (ANNOTATED, "", &[&square("7 0 R"), &ap]), (ANNOTATED, "", &[&square("5"), &ap]));
    // A widget's field type comes from its nearest parent that has one, and a loop of parents gives
    // none.
    let widget = "<< /Type /Annot /Subtype /Widget /Parent 8 0 R /Rect [0 0 10 10] /F 4 /AP << /N 7 0 R >> >>";
    let text_field = "<< /Parent 9 0 R >>";
    a1b_case(
        "6.5.3",
        (ANNOTATED, "", &[widget, &ap, text_field, "<< /FT /Tx /T (f) >>"]),
        (ANNOTATED, "", &[widget, &ap, text_field, "<< /FT /Btn /T (b) >>"]),
    );
    let looped = a1b(ANNOTATED, "", &[widget, &ap, "<< /Parent 8 0 R >>"]);
    assert!(verify(&looped, Level::A1b).is_empty(), "{:#?}", verify(&looped, Level::A1b));
    // 6.7.8: an extension schema description without its namespace, in any metadata stream.
    let xmp = concat!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#,
        r#"<rdf:Description rdf:about="" xmlns:pdfaExtension="http://www.aiim.org/pdfa/ns/extension/" "#,
        r#"xmlns:pdfaSchema="http://www.aiim.org/pdfa/ns/schema#"><pdfaExtension:schemas><rdf:Bag><rdf:li rdf:parseType="Resource">"#,
        r#"<pdfaSchema:schema>S</pdfaSchema:schema><pdfaSchema:prefix>e</pdfaSchema:prefix></rdf:li></rdf:Bag></pdfaExtension:schemas>"#,
        r#"</rdf:Description></rdf:RDF></x:xmpmeta>"#
    );
    assert_reported(&a1b(PAGE, "", &[&xmp_object(xmp)]), Level::A1b, "6.7.8");
}

#[test]
fn part_1_checks_shading_pattern_graphics_states() {
    // 6.4: a shading pattern's graphics state, direct or an indirect pattern with an untyped state.
    let tint = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >>";
    let shading = "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function 7 0 R >>";
    let fill = content("/Pattern cs /P0 scn 0 0 1 1 re f");
    let patterned = |state: &str| drawing(&format!("/Pattern << /P0 << /PatternType 2 /Shading {shading} /ExtGState {state} >> >>"));
    a1b_case("6.4", (&patterned("<< /CA 1 >>"), "", &[&fill, tint]), (&patterned("<< /CA 0.5 >>"), "", &[&fill, tint]));
    let indirect_pattern = format!("<< /PatternType 2 /Shading {shading} /ExtGState 9 0 R >>");
    let indirect_page = drawing("/Pattern << /P0 8 0 R >>");
    a1b_case(
        "6.4",
        (&indirect_page, "", &[&fill, tint, &indirect_pattern, "<< /BM /Normal >>", &form("")]),
        (&indirect_page, "", &[&fill, tint, &indirect_pattern, "<< /BM /Multiply /SMask << /S /Luminosity /G 10 0 R >> >>", &form("")]),
    );
}

#[test]
fn part_1_reads_direct_type3_font_resources() {
    // 6.4 and 6.2.3.3: a Type 3 font written straight into a page's resources has resources of its own.
    let text = content("BT /T3 1 Tf (a) Tj ET");
    let font_page = |resources: &str| {
        let font = format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 7 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << {resources} >> >>"
        );
        drawing(&format!("/Font << /T3 {font} >>"))
    };
    let glyph = content("1000 0 d0 /G gs 0 0 1 1 re f");
    a1b_case(
        "6.4",
        (&font_page("/ExtGState << /G << /CA 1 >> >>"), "", &[&text, &glyph]),
        (&font_page("/ExtGState << /G << /CA 0.5 >> >>"), "", &[&text, &glyph]),
    );
    let named = content("1000 0 d0 /CS0 cs 1 0 0 sc 0 0 1 1 re f");
    let spaces = |more: &str| font_page(&format!("/ColorSpace << /CS0 /DeviceRGB {more} >>"));
    assert_not_reported(&a1b_no_intent(&spaces(&format!("/DefaultRGB {CAL_RGB}")), &[&text, &named]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&spaces(""), &[&text, &named]), Level::A1b, "6.2.3.3");
}

#[test]
fn part_1_follows_aliased_page_contents() {
    // 6.2.3.3: page content reached through an alias object (single stream or array).
    let aliased = |spaces: &str| coloured_page(spaces, "");
    assert_not_reported(&a1b_no_intent(&aliased(&format!("/DefaultRGB {CAL_RGB}")), &["7 0 R", &content(RED)]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&aliased(""), &["7 0 R", &content(RED)]), Level::A1b, "6.2.3.3");
    assert_not_reported(&a1b_no_intent(&aliased(&format!("/DefaultRGB {CAL_RGB}")), &["7 0 R", "[8 0 R]", &content(RED)]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&aliased(""), &["7 0 R", "[8 0 R]", &content(RED)]), Level::A1b, "6.2.3.3");
}

#[test]
fn part_1_checks_inline_images() {
    // 6.2.4: inline images, not interpolated, with 1, 2, 4 or 8 bits (1 for a mask).
    let inline = |dict: &str| content(&format!("BI /W 1 /H 1 {dict} ID \0 EI"));
    a1b_case("6.2.4", (&drawing(""), "", &[&inline("/CS /G /BPC 8 /I false")]), (&drawing(""), "", &[&inline("/CS /G /BPC 8 /I true")]));
    a1b_case("6.2.4", (&drawing(""), "", &[&inline("/CS /G /BPC 4")]), (&drawing(""), "", &[&inline("/CS /G /BPC 16")]));
    a1b_case("6.2.4", (&drawing(""), "", &[&inline("/IM true /BPC 1")]), (&drawing(""), "", &[&inline("/IM true /BPC 2")]));
}

#[test]
fn part_1_reads_appearance_streams_without_bbox() {
    // 6.2.3.3: an appearance stream without /BBox or /Subtype is still read under its own /Resources.
    let ap_page = "<< /Type /Page /Parent 2 0 R /Annots [6 0 R] >>";
    let square = "<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 4 /AP << /N 7 0 R >> >>";
    assert_reported(&a1b_no_intent(ap_page, &[square, &content(RED)]), Level::A1b, "6.2.3.3");
    let bare_covered = format!("<< /Resources << /ColorSpace << /DefaultRGB {CAL_RGB} >> >> /Length {} >>\nstream\n{RED}\nendstream", RED.len());
    assert_not_reported(&a1b_no_intent(ap_page, &[square, &bare_covered]), Level::A1b, "6.2.3.3");
    let button = "<< /Type /Annot /Subtype /Widget /FT /Btn /Rect [0 0 10 10] /F 4 /AP << /N << /On 7 0 R >> >> >>";
    assert_reported(&a1b_no_intent(ap_page, &[button, &content(RED)]), Level::A1b, "6.2.3.3");
}

#[test]
fn part_1_judges_direct_page_shadings_under_fallback() {
    // 6.2.3.3: a direct shading in a page's /Shading is judged under fallback content too.
    let tint = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >>";
    let shading = "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function 7 0 R >>";
    let default_rgb = format!("/DefaultRGB {CAL_RGB}");
    let fallback_page = coloured_page(&default_rgb, &format!("/Shading << /Sh0 {shading} >> /XObject << /Fm 8 0 R >>"));
    let draw_fm = content("/Fm Do");
    assert_reported(&a1b_no_intent(&fallback_page, &[&draw_fm, tint, &drawing_form("/Resources << >>", "/Sh0 sh")]), Level::A1b, "6.2.3.3");
    let covered_form = drawing_form(&format!("/Resources << /ColorSpace << {default_rgb} >> >>"), "/Sh0 sh");
    assert_not_reported(&a1b_no_intent(&fallback_page, &[&draw_fm, tint, &covered_form]), Level::A1b, "6.2.3.3");
}

#[test]
fn part_1_shares_indirect_resource_maps_across_pages() {
    // Shared indirect /Resources and shared indirect /XObject, /Shading, /Pattern, /ColorSpace and
    // /Font maps are walked once, whilst keeping every page's Defaults intersection exact.
    let packet = xmp(1, "B", None);
    let metadata = xmp_object(&packet);
    let catalog = "<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>";
    let pages = "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /MediaBox [0 0 200 200] >>";
    let default_rgb = format!("/DefaultRGB {CAL_RGB}");
    let image =
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 3 >>\nstream\n\0\0\0\nendstream";
    let tint = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >>";
    let shading = "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function 9 0 R >>";
    let pattern = format!("<< /PatternType 2 /Shading {shading} >>");
    // Two pages and a form sharing one indirect /Resources dictionary.
    let shared_res = format!("<< /ColorSpace << {default_rgb} >> /XObject << /Im 7 0 R /Fm 9 0 R >> /Font << /F1 8 0 R >> >>");
    let page_shared = "<< /Type /Page /Parent 2 0 R /Resources 6 0 R >>";
    let form_shared = drawing_form("/Resources 6 0 R", RED);
    let doc_shared =
        build(&[catalog, pages, &metadata, page_shared, page_shared, &shared_res, image, "<< /Type /Font /Subtype /Type1 >>", &form_shared], "");
    assert_not_reported(&doc_shared, Level::A1b, "6.2.3.3");
    // Two pages with direct /Resources sharing indirect sub-maps: one page has DefaultRGB, the
    // other does not, so the shared /XObject, /Shading, /Pattern and /ColorSpace maps must still
    // report uncovered DeviceRGB.
    let with_default = |sub: &str| format!("<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << {default_rgb} >> {sub} >> >>");
    let without_default = |sub: &str| format!("<< /Type /Page /Parent 2 0 R /Resources << {sub} >> >>");
    let xobj_doc =
        build(&[catalog, pages, &metadata, &with_default("/XObject 6 0 R"), &without_default("/XObject 6 0 R"), "<< /Im 7 0 R >>", image], "");
    assert_reported(&xobj_doc, Level::A1b, "6.2.3.3");
    let xobj_ok = build(&[catalog, pages, &metadata, &with_default("/XObject 6 0 R"), &with_default("/XObject 6 0 R"), "<< /Im 7 0 R >>", image], "");
    assert_not_reported(&xobj_ok, Level::A1b, "6.2.3.3");
    let sub_maps = "/Shading 6 0 R /Pattern 7 0 R /Font 8 0 R";
    let sh_dict = format!("<< /Sh0 {shading} >>");
    let pat_dict = format!("<< /P0 {pattern} >>");
    let font_dict = "<< /F1 << /Type /Font /Subtype /Type1 >> >>";
    let map_fail = build(&[catalog, pages, &metadata, &with_default(sub_maps), &without_default(sub_maps), &sh_dict, &pat_dict, font_dict, tint], "");
    assert_reported(&map_fail, Level::A1b, "6.2.3.3");
    let map_ok = build(&[catalog, pages, &metadata, &with_default(sub_maps), &with_default(sub_maps), &sh_dict, &pat_dict, font_dict, tint], "");
    assert_not_reported(&map_ok, Level::A1b, "6.2.3.3");
    let cs_page = "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace 6 0 R >> >>";
    let cs_fail = build(&[catalog, pages, &metadata, cs_page, cs_page, "<< /CS0 /DeviceRGB >>"], "");
    assert_reported(&cs_fail, Level::A1b, "6.2.3.3");
}

#[test]
fn part_1_default_colour_spaces_require_compatible_cie_spaces() {
    let cmyk = content("0 0 0 1 k 0 0 1 1 re f");
    let gray = content("0.5 g 0 0 1 1 re f");
    let red = content(RED);
    let cmyk_prof = hex_stream("/N 4", &valid_v2_cmyk_profile());
    let rgb_prof = hex_stream("/N 3", &icc::srgb());
    let gray_prof = hex_stream("/N 1", &valid_v2_gray_profile());
    // DefaultCMYK accepts only 4-component ICCBased, never CalRGB or 3-component ICCBased.
    assert_reported(&a1b_no_intent(&coloured_page(&format!("/DefaultCMYK {CAL_RGB}"), ""), &[&cmyk]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&coloured_page("/DefaultCMYK [/ICCBased 7 0 R]", ""), &[&cmyk, &rgb_prof]), Level::A1b, "6.2.3.3");
    assert_not_reported(&a1b_no_intent(&coloured_page("/DefaultCMYK [/ICCBased 7 0 R]", ""), &[&cmyk, &cmyk_prof]), Level::A1b, "6.2.3.3");
    // DefaultRGB rejects Lab, 4-component ICCBased and non-integer /N.
    let lab = "[/Lab << /WhitePoint [0.9505 1 1.089] >>]";
    let real_n_prof = hex_stream("/N 3.0", &icc::srgb());
    assert_reported(&a1b_no_intent(&coloured_page(&format!("/DefaultRGB {lab}"), ""), &[&red]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&coloured_page("/DefaultRGB [/ICCBased 7 0 R]", ""), &[&red, &cmyk_prof]), Level::A1b, "6.2.3.3");
    assert_reported(&a1b_no_intent(&coloured_page("/DefaultRGB [/ICCBased 7 0 R]", ""), &[&red, &real_n_prof]), Level::A1b, "6.2.3.3");
    // DefaultGray accepts CalGray or 1-component ICCBased, not CalRGB.
    let cal_gray = "[/CalGray << /WhitePoint [0.9505 1 1.089] >>]";
    assert_reported(&a1b_no_intent(&coloured_page(&format!("/DefaultGray {CAL_RGB}"), ""), &[&gray]), Level::A1b, "6.2.3.3");
    assert_not_reported(&a1b_no_intent(&coloured_page(&format!("/DefaultGray {cal_gray}"), ""), &[&gray]), Level::A1b, "6.2.3.3");
    assert_not_reported(&a1b_no_intent(&coloured_page("/DefaultGray [/ICCBased 7 0 R]", ""), &[&gray, &gray_prof]), Level::A1b, "6.2.3.3");
}

#[test]
fn part_1_direct_type3_glyphs_follow_the_fonts_own_resources() {
    let text = content("BT /T3 1 Tf (a) Tj ET");
    let font_page = |resources: &str| {
        let font = format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 7 0 R >> /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [1000] /Resources << {resources} >> >>"
        );
        drawing(&format!("/Font << /T3 {font} >>"))
    };
    let glyph_red = content("1000 0 d0 1 0 0 rg 0 0 1 1 re f");
    assert_reported(&a1b_no_intent(&font_page(""), &[&text, &glyph_red]), Level::A1b, "6.2.3.3");
    assert_not_reported(&a1b_no_intent(&font_page(&format!("/ColorSpace << /DefaultRGB {CAL_RGB} >>")), &[&text, &glyph_red]), Level::A1b, "6.2.3.3");
    let glyph_inline = content("1000 0 d0 BI /W 1 /H 1 /CS /G /BPC 8 /I true ID \0 EI");
    assert_reported(&a1b(&font_page(""), "", &[&text, &glyph_inline]), Level::A1b, "6.2.4");
}

#[test]
fn part_1_validates_annotation_ap_structure_and_button_states() {
    let ap = form("");
    let widget = |ap_entry: &str| format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (f) /Rect [0 0 10 10] /F 4 /AP {ap_entry} >>");
    for bad in ["<< >>", "<< /N null >>", "5", "7 0 R"] {
        let doc = a1b(ANNOTATED, "", &[&widget(bad), &ap]);
        assert_reported(&doc, Level::A1b, "6.5.3");
        assert_reported(&doc, Level::A1b, "6.9");
    }
    let btn = |states: &str| format!("<< /Type /Annot /Subtype /Widget /FT /Btn /T (b) /Rect [0 0 10 10] /F 4 /AP << /N << {states} >> >> >>");
    a1b_case("6.5.3", (ANNOTATED, "", &[&btn("/On 7 0 R /Off 8 0 R"), &ap, &ap]), (ANNOTATED, "", &[&btn("/On 7 0 R /Off 5"), &ap, &ap]));
}

#[test]
fn part_1_forbids_blend_mode_arrays_and_ps_xobject_smasks() {
    let gs = |bm: &str| drawing(&format!("/ExtGState << /G << /BM {bm} >> >>"));
    let empty = content("");
    a1b_case("6.4", (&gs("/Normal"), "", &[&empty]), (&gs("[/Normal]"), "", &[&empty]));
    let ps_smask = "<< /Type /XObject /Subtype /PS /SMask 7 0 R /Length 0 >>\nstream\n\nendstream";
    let doc = a1b(PAGE, "", &[ps_smask, &form("")]);
    assert_reported(&doc, Level::A1b, "6.2.7");
    assert_reported(&doc, Level::A1b, "6.4");
}

#[test]
fn part_1_rejects_invalid_image_parameter_types() {
    let img = |extra: &str| {
        format!("<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray {extra} /Length 1 >>\nstream\n\0\nendstream")
    };
    a1b_case("6.2.4", (PAGE, "", &[&img("/BitsPerComponent 8 /Interpolate false")]), (PAGE, "", &[&img("/BitsPerComponent 8 /Interpolate 0")]));
    for real_bpc in ["/BitsPerComponent 8.0", "/BitsPerComponent 16.0"] {
        let doc = a1b(PAGE, "", &[&img(real_bpc)]);
        assert_eq!(messages(&doc, "6.2.4"), ["An image's BitsPerComponent isn't an integer"]);
    }
    let mask_real_bpc =
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true /BitsPerComponent 1.0 /Length 1 >>\nstream\n\0\nendstream";
    assert_eq!(messages(&a1b(PAGE, "", &[mask_real_bpc]), "6.2.4"), ["An image's BitsPerComponent isn't an integer"]);
    let indirect_mask =
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask 7 0 R /BitsPerComponent 8 /Length 1 >>\nstream\n\0\nendstream";
    a1b_case("6.2.4", (PAGE, "", &[&img("/BitsPerComponent 8")]), (PAGE, "", &[indirect_mask, "true"]));
    let mask_no_bpc = "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true /Length 1 >>\nstream\n\0\nendstream";
    assert!(verify(&a1b(PAGE, "", &[mask_no_bpc]), Level::A1b).is_empty());
    let inline = |dict: &str| content(&format!("BI /W 1 /H 1 {dict} ID \0 EI"));
    a1b_case("6.2.4", (&drawing(""), "", &[&inline("/CS /G /BPC 8 /I false")]), (&drawing(""), "", &[&inline("/CS /G /BPC 8 /I 0")]));
    a1b_case("6.2.4", (&drawing(""), "", &[&inline("/CS /G /BPC 8")]), (&drawing(""), "", &[&inline("/CS /G /BPC 8.0")]));
    a1b_case("6.2.4", (&drawing(""), "", &[&inline("/CS /G /BPC 8")]), (&drawing(""), "", &[&inline("/CS /G /BPC 16.0")]));
    a1b_case("6.2.4", (&drawing(""), "", &[&inline("/IM true /BPC 1")]), (&drawing(""), "", &[&inline("/IM true /BPC 1.0")]));
}

#[test]
fn part_1_validates_output_intent_icc_profile_structure() {
    // A 20-byte header-only stream and a 128-byte header without tags/BToA must both fail 6.2.2.
    let short = icc::srgb()[..20].to_vec();
    assert_reported(&a1b_with(&hex_stream("/N 3", &short), PAGE, "", &[]), Level::A1b, "6.2.2");
    let mut header_only = cmyk_profile_header();
    header_only[12..16].copy_from_slice(b"prtr");
    assert_reported(&a1b_with(&hex_stream("/N 4", &header_only), PAGE, "", &[]), Level::A1b, "6.2.2");
    // Mismatched, non-integer or missing /N fails 6.2.2.
    assert_reported(&a1b_with(&hex_stream("/N 4", &icc::srgb()), PAGE, "", &[]), Level::A1b, "6.2.2");
    assert_reported(&a1b_with(&hex_stream("/N 3.0", &icc::srgb()), PAGE, "", &[]), Level::A1b, "6.2.2");
    assert_reported(&a1b_with(&hex_stream("", &icc::srgb()), PAGE, "", &[]), Level::A1b, "6.2.2");
    // A printer RGB profile without BToA fails 6.2.2.
    let mut prtr_rgb = icc::srgb();
    prtr_rgb[12..16].copy_from_slice(b"prtr");
    assert_reported(&a1b_with(&hex_stream("/N 3", &prtr_rgb), PAGE, "", &[]), Level::A1b, "6.2.2");
    // Valid v2 printer CMYK (with B2A0) and printer grey (with kTRC) pass.
    assert!(verify(&a1b_with(&hex_stream("/N 4", &valid_v2_cmyk_profile()), PAGE, "", &[]), Level::A1b).is_empty());
    assert!(verify(&a1b_with(&hex_stream("/N 1", &valid_v2_gray_profile()), PAGE, "", &[]), Level::A1b).is_empty());
}

#[test]
fn part_1_resolves_indirect_need_appearances() {
    a1b_case(
        "6.9",
        (PAGE, "/AcroForm << /Fields [] /NeedAppearances 6 0 R >>", &["false"]),
        (PAGE, "/AcroForm << /Fields [] /NeedAppearances 6 0 R >>", &["true"]),
    );
}

#[test]
fn part_1_requires_xmp_x_default_title_when_info_has_title() {
    let profile = hex_stream("/N 3", &icc::srgb());
    let info = "<< /Title (Report) >>";
    let with_xmp = |packet: &str| page_document(PAGE, Some(packet), &output_intent(5), &[&profile, info], "/Info 6 0 R");
    // Info /Title without dc:title fails 6.7.3 in 1b, whilst 2b behaviour stays unchanged.
    assert_reported(&with_xmp(&xmp(1, "B", None)), Level::A1b, "6.7.3");
    assert_not_reported(&with_xmp(&a2b_packet(None)), Level::A2b, "6.6.3");
    // Malformed XMP fails 6.7.3 when Info /Title is set.
    assert_reported(&with_xmp("<x:xmpmeta><bad"), Level::A1b, "6.7.3");
    // dc:title without xml:lang="x-default" fails 6.7.3; with both fr and x-default matching, passes.
    let no_default = xmp(1, "B", Some("Report")).replace("xml:lang=\"x-default\"", "xml:lang=\"en\"");
    assert_reported(&with_xmp(&no_default), Level::A1b, "6.7.3");
    let multi_lang = xmp(1, "B", Some("Report")).replace(
        "<rdf:li xml:lang=\"x-default\">Report</rdf:li>",
        "<rdf:li xml:lang=\"fr\">Rapport</rdf:li><rdf:li xml:lang=\"x-default\">Report</rdf:li>",
    );
    assert!(verify(&with_xmp(&multi_lang), Level::A1b).is_empty());
}
