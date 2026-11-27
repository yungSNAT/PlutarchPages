//! PDF/A-1b (ISO 19005-1:2005, level B): what [`crate::verify`] checks for [`Level::A1b`], each
//! problem under its ISO 19005-1 clause.
//!
//! The checks share their predicates with parts 2 and 3 but not their clause numbers or their
//! choice of rules: each call here names its own clause, so a change to the part 2 checks cannot
//! move a part 1 citation. Where ISO 19005-1 differs, this file follows it: actions and annotations
//! are allow-lists (PDF 1.4's types less the forbidden ones), transparency is not allowed at all,
//! only widgets need appearances, and page additional actions are not a rule.
//!
//! Checked: 6.1.2 the header; 6.1.3 a file identifier and no encryption; 6.1.7 no external
//! streams; 6.1.10 no LZW streams; 6.1.11 no embedded files; 6.1.12 the integer, real, string, name
//! and CID limits of PDF 1.4; 6.1.13 no optional content; 6.2.2 the output intent profiles;
//! 6.2.3.3 device colour against the output intent; 6.2.3.4 Separation and DeviceN; 6.2.4 images;
//! 6.2.5 to 6.2.7 PostScript, reference and PostScript XObjects; 6.2.8 and 6.2.9 graphics states and
//! rendering intents; 6.3.4 fonts embedded; 6.4 transparency; 6.5.2 and 6.5.3 annotations; 6.6.1
//! and 6.6.2 actions; 6.7.2 metadata; 6.7.3 the title; 6.7.8 and 6.7.9 XMP extension schemas and
//! value types (with the part 2 schema lists); 6.7.11 identification; 6.9 forms.
//!
//! Not checked yet: the file's lexical structure (6.1.3 data after the end, 6.1.4, 6.1.6, 6.1.7
//! stream lengths, 6.1.8), content stream operators and nesting (6.1.10 inline images, 6.1.12 q
//! depth, 6.2.10), ICCBased colour spaces (6.2.3.2), font dictionaries and programs (6.3.2, 6.3.3,
//! 6.3.5 to 6.3.7), the other information dictionary entries (6.7.3) and the XMP packet header
//! (6.7.5).

use std::collections::HashMap;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use super::{Level, gstate, header, limits, spot, trailer};

/// The action types ISO 19005-1 6.6.1 leaves allowed (PDF 1.4's, less the forbidden ones).
const ALLOWED_ACTIONS: &[&[u8]] = &[b"GoTo", b"GoToR", b"Thread", b"URI", b"Named", b"SubmitForm"];

/// The annotation types ISO 19005-1 6.5.2 leaves allowed, as veraPDF reads them.
const ALLOWED_ANNOTS: &[&[u8]] = &[
    b"Text",
    b"Link",
    b"FreeText",
    b"Line",
    b"Square",
    b"Circle",
    b"Highlight",
    b"Underline",
    b"Squiggly",
    b"StrikeOut",
    b"Stamp",
    b"Ink",
    b"Popup",
    b"Widget",
    b"PrinterMark",
    b"TrapNet",
];

/// How many `/Parent` steps are followed looking for a field's type.
const MAX_FIELD_DEPTH: usize = 32;

fn forbidden_action(s: &[u8]) -> bool {
    !ALLOWED_ACTIONS.contains(&s)
}

/// Checks `doc` against the ISO 19005-1 level B clauses listed above. Nothing is fixable:
/// [`crate::convert`] doesn't convert to PDF/A-1b.
pub(super) fn check(
    doc: &Document,
    issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool),
    located_issue: &impl Fn(&'static str, &'static str, Vec<(&'static str, String)>, ObjRef, bool),
) {
    let mut part1_issue =
        |clause: &'static str, pattern: &'static str, value: Option<String>, page: Option<usize>| issue(clause, pattern, value, page, false);
    check_metadata(doc, &mut part1_issue);
    check_structure(doc, &mut part1_issue);
    check_catalog(doc, &mut part1_issue);
    let objs = super::objects(doc);
    check_colour(doc, &objs, &mut part1_issue);
    check_limits(doc, &objs, &mut part1_issue);
    super::check_xmp_packets(&objs, ("6.7.9", "6.7.8"), located_issue);
    let rgb_intent = super::pdfa_intent_profile(doc) == Some(Some(3));
    let drawing = super::drawing_fonts(doc, &objs);
    let pages = pdfcraft_model::pages(doc);
    // Each annotation's page: the first page that lists it.
    let mut annot_pages: HashMap<ObjRef, usize> = HashMap::new();
    for (i, p) in pages.iter().enumerate() {
        if let Some(a) = p.dict.get(b"Annots").map(|a| doc.resolve(a))
            && let Some(a) = a.as_array()
        {
            for r in a.iter().filter_map(Object::as_ref) {
                annot_pages.entry(r).or_insert(i);
            }
        }
    }
    let mut object = |d: &Dict, o: &Object, draws: bool, page: Option<usize>| {
        check_object(doc, d, o, Seen { draws, page, rgb_intent }, &mut part1_issue);
    };
    for (r, o) in &objs {
        if let Some(d) = super::dict_of(o) {
            object(d, o, drawing.contains(r), annot_pages.get(r).copied());
        }
    }
    // Annotations written straight into a page's list are no object of their own.
    for (i, p) in pages.iter().enumerate() {
        if let Some(a) = p.dict.get(b"Annots").map(|a| doc.resolve(a))
            && let Some(a) = a.as_array()
        {
            for entry in a {
                if let Object::Dict(d) = entry {
                    object(d, entry, false, Some(i));
                }
            }
        }
    }
}

/// 6.7.2, 6.7.3 and 6.7.11: the metadata stream and what it identifies.
fn check_metadata(doc: &Document, part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>)) {
    let id = super::identification(doc, Level::A1b);
    if id.missing {
        part1_issue("6.7.2", "The document has no XMP metadata", None, None);
    }
    if id.unidentified {
        part1_issue("6.7.11", "The metadata doesn't identify the file as {}", Some(Level::A1b.label().to_owned()), None);
    }
    if id.title_differs {
        part1_issue("6.7.3", "The title in the metadata and the document information differ", None, None);
    }
    let metadata = super::catalog(doc).get(b"Metadata").map(|m| doc.resolve(m));
    if let Some(m) = metadata
        && let Object::Stream(s) = &*m
        && s.dict.get(b"Filter").is_some()
    {
        part1_issue("6.7.2", "The metadata stream is compressed (Filter)", None, None);
    }
}

/// 6.1.2 and 6.1.3: the header, the file identifier and encryption.
fn check_structure(doc: &Document, part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>)) {
    let h = header::check(doc);
    if h.version {
        part1_issue("6.1.2", "The file doesn't start with a %PDF-1.0 to %PDF-1.7 header line", None, None);
    }
    if h.comment {
        part1_issue("6.1.2", "The file header isn't followed by a comment of four bytes above 127", None, None);
    }
    // Part 1 asks only that the identifier is there, not what it holds.
    if trailer::check(doc) == Some(trailer::IdProblem::Missing) {
        part1_issue("6.1.3", "The trailer has no file identifier (ID)", None, None);
    }
    if doc.trailer().get(b"Encrypt").is_some() {
        part1_issue("6.1.3", "The document is encrypted", None, None);
    }
}

/// 6.1.11, 6.1.13, 6.6.1, 6.6.2 and 6.9: what the catalog holds.
fn check_catalog(doc: &Document, part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>)) {
    let cat = super::catalog(doc);
    if cat.get(b"OCProperties").is_some() {
        part1_issue("6.1.13", "The document has optional content (OCProperties)", None, None);
    }
    if cat.get(b"AA").is_some() {
        part1_issue("6.6.2", "The document has additional actions (/AA)", None, None);
    }
    if let Some(names) = cat.get(b"Names").map(|n| doc.resolve(n))
        && let Some(names) = names.as_dict()
    {
        if names.get(b"JavaScript").is_some() {
            part1_issue("6.6.1", "The document has document-level JavaScript", None, None);
        }
        if names.get(b"EmbeddedFiles").is_some() {
            part1_issue("6.1.11", "Embedded files aren't allowed", None, None);
        }
    }
    if let Some(af) = cat.get(b"AcroForm").map(|a| doc.resolve(a))
        && af.as_dict().is_some_and(|a| matches!(a.get(b"NeedAppearances"), Some(Object::Bool(true))))
    {
        part1_issue("6.9", "The form asks viewers to make appearances (NeedAppearances)", None, None);
    }
}

/// 6.2.2, 6.2.3.3, 6.2.3.4, 6.2.8 and 6.2.9: the output intents, device colour, spot colour and
/// graphics states.
fn check_colour(
    doc: &Document,
    objs: &[(ObjRef, std::sync::Arc<Object>)],
    part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>),
) {
    let intents = super::catalog(doc).get(b"OutputIntents").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
    // 6.2.2: every intent that names a profile names the same object.
    let profiles: Vec<Object> =
        intents.iter().filter_map(|oi| doc.resolve(oi).as_dict().and_then(|d| d.get(b"DestOutputProfile").cloned())).collect();
    if profiles.len() > 1 && (profiles.iter().any(|p| p.as_ref().is_none()) || profiles.windows(2).any(|w| w[0] != w[1])) {
        part1_issue("6.2.2", "The output intents name different profiles", None, None);
    }
    let pdfa = intents.iter().find_map(|oi| doc.resolve(oi).as_dict().filter(|d| d.name(b"S") == Some(b"GTS_PDFA1")).cloned());
    if let Some(intent) = pdfa
        && let Some(profile) = intent.get(b"DestOutputProfile").map(|p| doc.resolve(p))
        && !icc_output_profile(&profile)
    {
        part1_issue("6.2.2", "The output intent's profile isn't an ICC version 2 printer or display profile in grey, RGB or CMYK", None, None);
    }
    let used = super::uses_device_colour(doc, objs);
    if used.incomplete {
        part1_issue("6.2.3.3", "Some content could not be read for device colour", None, None);
    }
    match super::pdfa_intent_profile(doc) {
        None if used.any() => part1_issue("6.2.3.3", "Device colour is used, but there is no PDF/A output intent", None, None),
        Some(n) => {
            if used.cmyk && !used.default_cmyk && n != Some(4) {
                part1_issue("6.2.3.3", "DeviceCMYK is used, but the PDF/A output intent isn't a CMYK profile", None, None);
            }
            if used.rgb && !used.default_rgb && n != Some(3) {
                part1_issue("6.2.3.3", "DeviceRGB is used, but the PDF/A output intent isn't an RGB profile", None, None);
            }
        }
        None => {}
    }
    let spots = spot::check(doc, objs);
    for name in spots.differing {
        part1_issue("6.2.3.4", "Separation colour spaces named {} differ in their alternate space or tint transform", Some(name), None);
    }
    for name in spots.undefined {
        part1_issue("6.2.3.4", "The spot colour {} of a DeviceN colour space isn't in its Colorants dictionary", Some(name), None);
    }
    let gs = gstate::check(doc);
    if gs.transfer {
        part1_issue("6.2.8", "A graphics state has a transfer function (TR)", None, None);
    }
    if gs.transfer2 {
        part1_issue("6.2.8", "A graphics state has a transfer function (TR2) other than Default", None, None);
    }
    for intent in gs.intents {
        part1_issue("6.2.9", "A graphics state has the rendering intent {}, which isn't a standard one", Some(intent), None);
    }
    if gs.intent_not_name {
        part1_issue("6.2.9", "A graphics state has a rendering intent that isn't a name", None, None);
    }
}

/// Whether `profile` is a stream holding an ICC profile of a version below 3, for a printer or a
/// display, in grey, RGB or CMYK (the ICC header: the major version at byte 8, the device class at
/// bytes 12 to 15 and the data colour space at 16 to 19).
fn icc_output_profile(profile: &Object) -> bool {
    let Object::Stream(s) = profile else { return false };
    let Ok(data) = s.decoded_within(super::MAX_ICC_PROFILE) else { return false };
    let (Some(version), Some(class), Some(space)) = (data.get(8), data.get(12..16), data.get(16..20)) else { return false };
    *version < 3 && matches!(class, b"prtr" | b"mntr") && matches!(space, b"GRAY" | b"RGB " | b"CMYK")
}

/// 6.1.12: the implementation limits of PDF 1.4.
fn check_limits(
    doc: &Document,
    objs: &[(ObjRef, std::sync::Arc<Object>)],
    part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>),
) {
    let over = limits::exceeded(doc, objs, &super::referenced_content(doc, objs), limits::PDF_1_4);
    if over.int {
        part1_issue("6.1.12", "An integer is outside the 32-bit range", None, None);
    }
    if over.real {
        part1_issue("6.1.12", "A real number is outside ±32767", None, None);
    }
    if over.string {
        part1_issue("6.1.12", "A string is longer than 65535 bytes", None, None);
    }
    if over.name {
        part1_issue("6.1.12", "A name is longer than 127 bytes", None, None);
    }
    if limits::cid_over_limit(doc, objs) {
        part1_issue("6.1.12", "A CMap maps a character code to a CID above 65535", None, None);
    }
}

/// What the walk knows about an object besides the object itself.
#[derive(Clone, Copy)]
struct Seen {
    /// A page draws with it as a font.
    draws: bool,
    /// The page that lists it as an annotation.
    page: Option<usize>,
    /// The PDF/A output intent's profile is an RGB one.
    rgb_intent: bool,
}

fn check_object(
    doc: &Document,
    d: &Dict,
    o: &Object,
    seen: Seen,
    part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>),
) {
    let ty = d.name(b"Type");
    let subtype = d.name(b"Subtype");
    // 6.6.1: the object's own action and the ones written into it.
    let own = (d.get(b"S").is_some() && super::is_action(d)).then_some(d);
    let direct = [b"A".as_slice(), b"OpenAction"].map(|key| d.get(key).and_then(Object::as_dict));
    for action in [own].into_iter().chain(direct).flatten() {
        if let Some(name) = super::chain_forbidden_by(doc, action, forbidden_action) {
            part1_issue("6.6.1", "A {} action isn't allowed", Some(name), None);
        }
    }
    let is_annot = matches!(o, Object::Dict(_)) && super::is_annotation(doc, d, ty, subtype, seen.page.is_some());
    if is_annot {
        check_annotation(doc, d, subtype, seen, part1_issue);
    } else if d.get(b"FT").is_some() && d.get(b"AA").is_some() {
        part1_issue("6.6.2", "A form field has additional actions", None, None);
    }
    // 6.3.4: a font a page draws with is embedded (Type 3 fonts are their own programs).
    if seen.draws
        && subtype != Some(b"Type3")
        && (ty == Some(b"Font") || d.get(b"BaseFont").is_some() && subtype.is_some())
        && !super::font_embedded(doc, d)
    {
        match d.name(b"BaseFont") {
            Some(n) => part1_issue("6.3.4", "The font {} isn't embedded", Some(String::from_utf8_lossy(n).into_owned()), None),
            None => part1_issue("6.3.4", "A font isn't embedded", None, None),
        }
    }
    if let Object::Stream(s) = o {
        check_stream(doc, s, subtype, part1_issue);
    }
    // 6.1.11: a file specification with the file in it.
    if d.get(b"EF").is_some() {
        part1_issue("6.1.11", "Embedded files aren't allowed", None, None);
    }
    // 6.4: graphics states, wherever a resource dictionary or the object itself holds one.
    if ty == Some(b"ExtGState") {
        check_graphics_state(doc, d, part1_issue);
    }
    if let Some(states) = d.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().and_then(|r| r.get(b"ExtGState").cloned())) {
        let states = doc.resolve(&states);
        for (_, gs) in states.as_dict().map(Dict::iter).into_iter().flatten() {
            if let Some(gs) = doc.resolve(gs).as_dict() {
                check_graphics_state(doc, gs, part1_issue);
            }
        }
    }
    if let Some(group) = d.get(b"Group").map(|g| doc.resolve(g))
        && group.as_dict().is_some_and(|g| g.name(b"S") == Some(b"Transparency"))
    {
        part1_issue("6.4", "A transparency group is used", None, None);
    }
}

/// 6.1.7, 6.1.10, 6.2.4 to 6.2.7 and 6.4: one stream's filters and keys.
fn check_stream(
    doc: &Document,
    s: &Stream,
    subtype: Option<&[u8]>,
    part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>),
) {
    if super::uses_lzw(doc, s) {
        part1_issue("6.1.10", "A stream uses LZW compression", None, None);
    }
    if [b"F".as_slice(), b"FFilter", b"FDecodeParms"].iter().any(|k| s.dict.get(k).is_some()) {
        part1_issue("6.1.7", "A stream refers to an external file", None, None);
    }
    let bits = s.dict.get(b"BitsPerComponent").map(|b| doc.resolve(b)).and_then(|b| b.as_int());
    match subtype {
        Some(b"Image") => {
            if matches!(s.dict.get(b"Interpolate"), Some(Object::Bool(true))) {
                part1_issue("6.2.4", "An image asks to be interpolated", None, None);
            }
            if s.dict.get(b"Alternates").is_some() || s.dict.get(b"OPI").is_some() {
                part1_issue("6.2.4", "An image has alternates or OPI information", None, None);
            }
            let mask = matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)));
            match bits {
                Some(b) if mask && b != 1 => part1_issue("6.2.4", "An image mask has BitsPerComponent {}, not 1", Some(b.to_string()), None),
                Some(b) if !mask && ![1, 2, 4, 8].contains(&b) => {
                    part1_issue("6.2.4", "An image has BitsPerComponent {}, not 1, 2, 4 or 8", Some(b.to_string()), None)
                }
                _ => {}
            }
            if s.dict.get(b"SMask").is_some() {
                part1_issue("6.4", "An XObject has a soft mask (SMask)", None, None);
            }
        }
        Some(b"Form") => {
            if s.dict.name(b"Subtype2") == Some(b"PS") || s.dict.get(b"PS").is_some() {
                part1_issue("6.2.5", "A form XObject holds PostScript (PS or Subtype2 PS)", None, None);
            }
            if s.dict.get(b"Ref").is_some() {
                part1_issue("6.2.6", "Reference XObjects aren't allowed", None, None);
            }
            if s.dict.get(b"SMask").is_some() {
                part1_issue("6.4", "An XObject has a soft mask (SMask)", None, None);
            }
        }
        Some(b"PS") => part1_issue("6.2.7", "PostScript XObjects aren't allowed", None, None),
        _ => {}
    }
}

/// 6.4: a graphics state paints opaquely, with normal blending and no soft mask.
fn check_graphics_state(doc: &Document, gs: &Dict, part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>)) {
    if let Some(mask) = gs.get(b"SMask").map(|m| doc.resolve(m))
        && mask.as_name() != Some(b"None")
    {
        part1_issue("6.4", "A graphics state has a soft mask (SMask)", None, None);
    }
    if let Some(bm) = gs.get(b"BM").map(|b| doc.resolve(b)) {
        let modes: Vec<Object> = match &*bm {
            Object::Array(list) => list.iter().map(|m| (*doc.resolve(m)).clone()).collect(),
            other => vec![other.clone()],
        };
        for mode in modes {
            let name = mode.as_name().unwrap_or_default();
            if !matches!(name, b"Normal" | b"Compatible") {
                part1_issue(
                    "6.4",
                    "A graphics state has the blend mode {}: only Normal and Compatible are allowed",
                    Some(String::from_utf8_lossy(name).into_owned()),
                    None,
                );
            }
        }
    }
    let opaque = |key: &[u8]| gs.get(key).map(|a| doc.resolve(a)).is_none_or(|a| a.as_f64() == Some(1.0));
    if !opaque(b"CA") || !opaque(b"ca") {
        part1_issue("6.4", "A graphics state has a constant alpha (CA or ca) other than 1", None, None);
    }
}

/// 6.5.2, 6.5.3, 6.6.1, 6.6.2 and 6.9: one annotation.
fn check_annotation(
    doc: &Document,
    d: &Dict,
    subtype: Option<&[u8]>,
    seen: Seen,
    part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>),
) {
    let page = seen.page;
    let name = subtype.map(|bytes| String::from_utf8_lossy(bytes).into_owned()).unwrap_or_default();
    if !subtype.is_some_and(|s| ALLOWED_ANNOTS.contains(&s)) {
        part1_issue("6.5.2", "{} annotations aren't allowed", Some(name), page);
        return;
    }
    if d.get(b"CA").map(|a| doc.resolve(a)).is_some_and(|a| a.as_f64() != Some(1.0)) {
        part1_issue("6.5.3", "A {} annotation has a constant alpha (CA) other than 1", Some(name.clone()), page);
    }
    let flags = d.get(b"F").map(|f| doc.resolve(f)).and_then(|f| f.as_int()).unwrap_or(0);
    if flags & 4 == 0 || flags & (1 | 2 | 32) != 0 {
        part1_issue("6.5.3", "A {} annotation isn't set to print, or is hidden", Some(name.clone()), page);
    }
    if (d.get(b"C").is_some() || d.get(b"IC").is_some()) && !seen.rgb_intent {
        part1_issue("6.5.3", "A {} annotation has a colour (C or IC), but the output intent isn't RGB", Some(name.clone()), page);
    }
    let widget = subtype == Some(b"Widget");
    let appearance = d.get(b"AP").map(|a| doc.resolve(a));
    if let Some(ap) = appearance.as_deref().and_then(Object::as_dict) {
        if ap.iter().any(|(k, _)| k.as_slice() != b"N") {
            part1_issue("6.5.3", "A {} annotation's appearance dictionary holds entries other than N", Some(name.clone()), page);
        }
        if let Some(normal) = ap.get(b"N") {
            let button = widget && field_type(doc, d).as_deref() == Some(b"Btn".as_slice());
            let right = match &*doc.resolve(normal) {
                Object::Dict(states) => button && !states.is_empty(),
                Object::Stream(_) => !button,
                _ => false,
            };
            if !right {
                part1_issue("6.5.3", "A {} annotation's normal appearance is the wrong kind of object", Some(name.clone()), page);
            }
        }
    }
    if widget {
        if d.get(b"A").is_some() {
            part1_issue("6.6.1", "A form field performs an action (A)", None, page);
        }
        if d.get(b"AA").is_some() {
            part1_issue("6.6.2", "A {} annotation has additional actions", Some(name.clone()), page);
        }
        if appearance.is_none() {
            part1_issue("6.9", "A {} annotation has no appearance stream", Some(name), page);
        }
    }
}

/// A field's type (`/FT`), its own or the nearest one its `/Parent` fields give it.
fn field_type(doc: &Document, d: &Dict) -> Option<Vec<u8>> {
    let mut current = d.clone();
    for _ in 0..MAX_FIELD_DEPTH {
        if let Some(ft) = current.get(b"FT").map(|f| doc.resolve(f)) {
            return ft.as_name().map(<[u8]>::to_vec);
        }
        current = doc.resolve(current.get(b"Parent")?).as_dict()?.clone();
    }
    None
}
