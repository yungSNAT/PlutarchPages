//! PDF/A-1b (ISO 19005-1:2005, level B): what [`crate::verify`] checks for [`Level::A1b`], each
//! problem under its ISO 19005-1 clause.
//!
//! The checks share their predicates with parts 2 and 3 but not their clause numbers or their
//! choice of rules: each call here names its own clause, so a change to the part 2 checks cannot
//! move a part 1 citation. Where ISO 19005-1 differs, this file follows it: annotations are an
//! allow-list, named action types are filtered to PDF 1.4's non-forbidden ones (6.6.1 is a
//! deny-list, so an action without a name `/S` is not flagged), transparency is not allowed at
//! all, only widgets need appearances, and page additional actions are not a rule.
//!
//! Checked: 6.1.2 the header; 6.1.3 a file identifier and no encryption; 6.1.7 no external
//! streams; 6.1.10 no LZW streams; 6.1.11 no embedded files; 6.1.12 the integer, real, string, name
//! and CID limits of PDF 1.4; 6.1.13 no optional content; 6.2.2 the output intent profiles;
//! 6.2.3.3 device colour against the output intent; 6.2.4 images (inline included) and form OPI;
//! 6.2.5 to 6.2.7 PostScript, reference and PostScript XObjects; 6.2.8 and 6.2.9 graphics states and
//! rendering intents; 6.3.4 fonts embedded; 6.4 transparency; 6.5.2 and 6.5.3 annotations; 6.6.1
//! and 6.6.2 actions; 6.7.2 metadata; 6.7.3 the title; 6.7.8 and 6.7.9 XMP extension schemas and
//! value types (with the part 2 schema lists); 6.7.11 identification; 6.9 forms.
//!
//! Not checked yet: the file's lexical structure (6.1.3 data after the end, 6.1.4, 6.1.6, 6.1.7
//! stream lengths, 6.1.8), content stream operators and nesting (6.1.10 inline images, 6.1.12 q
//! depth, 6.2.10), ICCBased colour spaces (6.2.3.2), font dictionaries and programs (6.3.2, 6.3.3,
//! 6.3.5 to 6.3.7), the other information dictionary entries (6.7.3) and the XMP packet header
//! (6.7.5), fonts written straight into a resource dictionary rather than as objects of their own
//! (6.3.4), and XMP packets in UTF-16 (6.7.11 reads them as UTF-8). Like parts 2 and 3, the object
//! walk reads every object in the file, including ones no page reaches.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use pdfcraft_model::Page;

use super::{Level, gstate, header, limits, trailer};

mod colour;

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

/// Records an issue: clause, message pattern, its value, page, fixable.
pub(super) type IssueSink<'a> = dyn FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool) + 'a;
/// Records an issue located at an object: clause, message pattern, values, object, fixable.
pub(super) type LocatedSink<'a> = dyn Fn(&'static str, &'static str, Vec<(&'static str, String)>, ObjRef, bool) + 'a;

fn forbidden_action(s: &[u8]) -> bool {
    !ALLOWED_ACTIONS.contains(&s)
}

/// Checks `doc` against the ISO 19005-1 level B clauses listed above. Nothing is fixable:
/// [`crate::convert`] doesn't convert to PDF/A-1b.
pub(super) fn check(doc: &Document, issue: &mut IssueSink<'_>, located_issue: &LocatedSink<'_>) {
    let mut part1_issue =
        |clause: &'static str, pattern: &'static str, value: Option<String>, page: Option<usize>| issue(clause, pattern, value, page, false);
    check_metadata(doc, &mut part1_issue);
    check_structure(doc, &mut part1_issue);
    check_catalog(doc, &mut part1_issue);
    let objs = super::objects(doc);
    let pages = pdfcraft_model::pages(doc);
    check_colour(doc, &objs, &pages, &mut part1_issue);
    check_limits(doc, &objs, &mut part1_issue);
    super::check_xmp_packets(&objs, ("6.7.9", "6.7.8"), &located_issue);
    let rgb_intent = super::pdfa_intent_profile(doc) == Some(Some(3));
    let drawing = super::drawing_fonts(doc, &objs);
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
    let mut checked = HashSet::new();
    let mut object = |d: &Dict, o: &Object, draws: bool, page: Option<usize>| {
        check_object(doc, (d, o), Seen { draws, page, rgb_intent }, &mut checked, &mut part1_issue);
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
    {
        if s.dict.name(b"Type") != Some(b"Metadata") || s.dict.name(b"Subtype") != Some(b"XML") {
            part1_issue("6.7.2", "The metadata stream isn't marked as XML metadata (Type Metadata, Subtype XML)", None, None);
        }
        if s.dict.get(b"Filter").is_some() {
            part1_issue("6.7.2", "The metadata stream is compressed (Filter)", None, None);
        }
    }
}

/// 6.1.2 and 6.1.3: the header, the file identifier and encryption.
fn check_structure(doc: &Document, part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>)) {
    let h = header::check_with(doc, header::any_version);
    if h.version {
        part1_issue("6.1.2", "The file doesn't start with a %PDF-n.n header line", None, None);
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
        && af.as_dict().is_some_and(|a| a.get(b"NeedAppearances").map(|v| doc.resolve(v)).is_some_and(|v| matches!(*v, Object::Bool(true))))
    {
        part1_issue("6.9", "The form asks viewers to make appearances (NeedAppearances)", None, None);
    }
}

/// 6.2.2, 6.2.3.3, 6.2.8 and 6.2.9: the output intents, device colour and graphics states.
fn check_colour(
    doc: &Document,
    objs: &[(ObjRef, std::sync::Arc<Object>)],
    pages: &[Page],
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
        && !icc_output_profile(doc, &profile)
    {
        part1_issue("6.2.2", "The output intent's profile isn't an ICC version 2 printer or display profile in grey, RGB or CMYK", None, None);
    }
    let (used, inline) = colour::uncovered_device_colour(doc, objs, pages);
    if inline.interpolate {
        part1_issue("6.2.4", "An inline image asks to be interpolated", None, None);
    }
    if inline.bits {
        part1_issue("6.2.4", "An inline image has a BitsPerComponent other than 1, 2, 4 or 8 (1 for a mask)", None, None);
    }
    if used.incomplete {
        part1_issue("6.2.3.3", "Some content could not be read for device colour", None, None);
    }
    let (gray, rgb, cmyk) = (used.gray, used.rgb, used.cmyk);
    match super::pdfa_intent_profile(doc) {
        None if gray || rgb || cmyk => {
            part1_issue("6.2.3.3", "Device colour is used, but there is no PDF/A output intent", None, None);
        }
        Some(n) => {
            if cmyk && n != Some(4) {
                part1_issue("6.2.3.3", "DeviceCMYK is used, but the PDF/A output intent isn't a CMYK profile", None, None);
            }
            if rgb && n != Some(3) {
                part1_issue("6.2.3.3", "DeviceRGB is used, but the PDF/A output intent isn't an RGB profile", None, None);
            }
        }
        None => {}
    }
    // 6.4 for every graphics state the document reaches, a shading pattern's and those of a Type 3
    // font written straight into a resource dictionary included; the object walk adds the rest.
    let gs = gstate::check_with(doc, |state| check_graphics_state(doc, state, part1_issue));
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

/// Whether `profile` is a stream holding a valid ICC version 2 (or earlier) printer or display
/// profile in grey, RGB or CMYK, with matching `/N` and PCS-to-device conversion data (`BToA` for
/// printer RGB/CMYK, `BToA` or matrix/TRC for display RGB, `BToA` or `kTRC` for grey).
fn icc_output_profile(doc: &Document, profile: &Object) -> bool {
    use craft_cms_v2::{ColorSpace, Profile, ProfileClass};
    let Object::Stream(s) = profile else { return false };
    let Ok(data) = s.decoded_within(super::MAX_ICC_PROFILE) else { return false };
    let Ok(parsed) = Profile::parse(&data) else { return false };
    let ok_class = matches!(parsed.class, ProfileClass::Output | ProfileClass::Display);
    let ok_space = matches!(parsed.color_space, ColorSpace::Gray | ColorSpace::Rgb | ColorSpace::Cmyk);
    let ok_n = s.dict.get(b"N").map(|n| doc.resolve(n)).and_then(|n| n.as_int()) == Some(parsed.channels() as i64);
    let ok_b2a = parsed.can_be_destination()
        && (parsed.class != ProfileClass::Output || parsed.color_space == ColorSpace::Gray || parsed.b2a.iter().any(Option::is_some));
    parsed.version.0 < 3 && ok_class && ok_space && ok_n && ok_b2a
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

/// One object, `(d, o)` its dictionary and itself. `checked` holds the indirect resource,
/// `ExtGState` and graphics state dictionaries already checked, each with the role it was read in,
/// so a shared one is read once in each role.
fn check_object(
    doc: &Document,
    (d, o): (&Dict, &Object),
    seen: Seen,
    checked: &mut HashSet<(&'static str, ObjRef)>,
    part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>),
) {
    let ty = d.name(b"Type");
    let subtype = d.name(b"Subtype");
    // 6.6.1: the actions the object points at, written in or referenced, whatever else they hold,
    // and the actions their /Next entries chain on. Additional actions (allowed on pages and most
    // annotations in part 1) hold actions too.
    let additional = d.get(b"AA").map(|aa| doc.resolve(aa));
    let additional = additional.as_deref().and_then(Object::as_dict).into_iter().flat_map(|aa| aa.iter().map(|(_, a)| a));
    for action in [b"A".as_slice(), b"OpenAction"].into_iter().filter_map(|key| d.get(key)).chain(additional) {
        if let Some(action) = doc.resolve(action).as_dict()
            && let Some(name) = super::chain_forbidden_by(doc, action, forbidden_action)
        {
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
    // An indirect holder is checked once in each of its roles, however many objects name it.
    let mut first = |role: &'static str, o: &Object| o.as_ref().is_none_or(|r| checked.insert((role, r)));
    if let Some(resources) = d.get(b"Resources").filter(|r| first("Resources", r))
        && let Some(states) = doc.resolve(resources).as_dict().and_then(|r| r.get(b"ExtGState").cloned())
        && first("ExtGState", &states)
    {
        let states = doc.resolve(&states);
        for (_, gs) in states.as_dict().map(Dict::iter).into_iter().flatten() {
            if first("state", gs)
                && let Some(gs) = doc.resolve(gs).as_dict()
            {
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
    if s.dict.get(b"SMask").is_some() && (s.dict.name(b"Type") == Some(b"XObject") || matches!(subtype, Some(b"Image" | b"Form" | b"PS"))) {
        part1_issue("6.4", "An XObject has a soft mask (SMask)", None, None);
    }
    match subtype {
        Some(b"Image") => check_image_stream(doc, &s.dict, part1_issue),
        Some(b"Form") => {
            if s.dict.name(b"Subtype2") == Some(b"PS") || s.dict.get(b"PS").is_some() {
                part1_issue("6.2.5", "A form XObject holds PostScript (PS or Subtype2 PS)", None, None);
            }
            if s.dict.get(b"Ref").is_some() {
                part1_issue("6.2.6", "Reference XObjects aren't allowed", None, None);
            }
            if s.dict.get(b"OPI").is_some() {
                part1_issue("6.2.4", "A form XObject has OPI information", None, None);
            }
        }
        Some(b"PS") => part1_issue("6.2.7", "PostScript XObjects aren't allowed", None, None),
        _ => {}
    }
}

/// 6.2.4: one image XObject's interpolation, alternates/OPI and bits per component.
fn check_image_stream(doc: &Document, d: &Dict, part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>)) {
    if let Some(interp) = d.get(b"Interpolate").map(|i| doc.resolve(i)).filter(|i| !matches!(**i, Object::Null)) {
        match *interp {
            Object::Bool(false) => {}
            Object::Bool(true) => part1_issue("6.2.4", "An image asks to be interpolated", None, None),
            _ => part1_issue("6.2.4", "An image's Interpolate entry isn't false", None, None),
        }
    }
    if d.get(b"Alternates").is_some() || d.get(b"OPI").is_some() {
        part1_issue("6.2.4", "An image has alternates or OPI information", None, None);
    }
    let mask = d.get(b"ImageMask").map(|m| doc.resolve(m)).is_some_and(|m| matches!(*m, Object::Bool(true)));
    if let Some(bpc) = d.get(b"BitsPerComponent").map(|b| doc.resolve(b)).filter(|b| !matches!(**b, Object::Null)) {
        match bpc.as_int() {
            Some(1) => {}
            Some(2 | 4 | 8) if !mask => {}
            Some(b) if mask => part1_issue("6.2.4", "An image mask has BitsPerComponent {}, not 1", Some(b.to_string()), None),
            Some(b) => part1_issue("6.2.4", "An image has BitsPerComponent {}, not 1, 2, 4 or 8", Some(b.to_string()), None),
            None => part1_issue("6.2.4", "An image's BitsPerComponent isn't an integer", None, None),
        }
    }
}

/// 6.4: a graphics state paints opaquely, with normal blending and no soft mask.
fn check_graphics_state(doc: &Document, gs: &Dict, part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>)) {
    if let Some(mask) = gs.get(b"SMask").map(|m| doc.resolve(m))
        && mask.as_name() != Some(b"None")
    {
        part1_issue("6.4", "A graphics state has a soft mask (SMask)", None, None);
    }
    if let Some(bm) = gs.get(b"BM").map(|b| doc.resolve(b)).filter(|b| !matches!(**b, Object::Null)) {
        match bm.as_name() {
            Some(b"Normal" | b"Compatible") => {}
            Some(name) => part1_issue(
                "6.4",
                "A graphics state has the blend mode {}: only Normal and Compatible are allowed",
                Some(String::from_utf8_lossy(name).into_owned()),
                None,
            ),
            None => part1_issue("6.4", "A graphics state has a blend mode that isn't a name", None, None),
        }
    }
    let opaque = |key: &[u8]| gs.get(key).map(|a| doc.resolve(a)).is_none_or(|a| is_one(&a));
    if !opaque(b"CA") || !opaque(b"ca") {
        part1_issue("6.4", "A graphics state has a constant alpha (CA or ca) other than 1", None, None);
    }
}

/// Whether `a` is the number 1, within the precision a PDF real carries (`1.0000001` is 1).
fn is_one(a: &Object) -> bool {
    a.as_f64().is_some_and(|v| (v - 1.0).abs() < 1e-5)
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
    let Some(subtype) = subtype else {
        part1_issue("6.5.2", "An annotation has no type (Subtype)", None, page);
        return;
    };
    let name = String::from_utf8_lossy(subtype).into_owned();
    if !ALLOWED_ANNOTS.contains(&subtype) {
        part1_issue("6.5.2", "{} annotations aren't allowed", Some(name), page);
        return;
    }
    if d.get(b"CA").map(|a| doc.resolve(a)).is_some_and(|a| !is_one(&a)) {
        part1_issue("6.5.3", "A {} annotation has a constant alpha (CA) other than 1", Some(name.clone()), page);
    }
    let flags = d.get(b"F").map(|f| doc.resolve(f)).and_then(|f| f.as_int()).unwrap_or(0);
    if flags & 4 == 0 || flags & (1 | 2 | 32) != 0 {
        part1_issue("6.5.3", "A {} annotation isn't set to print, or is hidden", Some(name.clone()), page);
    }
    if (d.get(b"C").is_some() || d.get(b"IC").is_some()) && !seen.rgb_intent {
        part1_issue("6.5.3", "A {} annotation has a colour (C or IC), but the output intent isn't RGB", Some(name.clone()), page);
    }
    let widget = subtype == b"Widget";
    let has_normal = check_appearance(doc, d, &name, widget, page, part1_issue);
    if widget {
        if d.get(b"A").is_some() {
            part1_issue("6.6.1", "A form field performs an action (A)", None, page);
        }
        if d.get(b"AA").is_some() {
            part1_issue("6.6.2", "A {} annotation has additional actions", Some(name.clone()), page);
        }
        if !has_normal {
            part1_issue("6.9", "A {} annotation has no appearance stream", Some(name), page);
        }
    }
}

/// 6.5.3: one annotation's `/AP` dictionary, returning whether it carries a `/N` entry.
fn check_appearance(
    doc: &Document,
    d: &Dict,
    name: &str,
    widget: bool,
    page: Option<usize>,
    part1_issue: &mut impl FnMut(&'static str, &'static str, Option<String>, Option<usize>),
) -> bool {
    let Some(ap) = d.get(b"AP").map(|a| doc.resolve(a)).filter(|a| !matches!(**a, Object::Null)) else {
        return false;
    };
    let Object::Dict(ap) = &*ap else {
        part1_issue("6.5.3", "A {} annotation's normal appearance is the wrong kind of object", Some(name.to_owned()), page);
        return false;
    };
    if ap.iter().any(|(k, _)| k.as_slice() != b"N") {
        part1_issue("6.5.3", "A {} annotation's appearance dictionary holds entries other than N", Some(name.to_owned()), page);
    }
    let Some(normal) = ap.get(b"N") else {
        part1_issue("6.5.3", "A {} annotation's appearance dictionary has no normal appearance (N)", Some(name.to_owned()), page);
        return false;
    };
    let button = widget && field_type(doc, d).as_deref() == Some(b"Btn".as_slice());
    let right = match &*doc.resolve(normal) {
        Object::Dict(states) => button && !states.is_empty() && states.iter().all(|(_, v)| matches!(&*doc.resolve(v), Object::Stream(_))),
        Object::Stream(_) => !button,
        _ => false,
    };
    if !right {
        part1_issue("6.5.3", "A {} annotation's normal appearance is the wrong kind of object", Some(name.to_owned()), page);
    }
    true
}

/// A field's type (`/FT`), its own or the nearest one its `/Parent` fields give it.
fn field_type(doc: &Document, d: &Dict) -> Option<Vec<u8>> {
    let ft = |field: &Dict| field.get(b"FT").map(|f| doc.resolve(f).as_name().map(<[u8]>::to_vec));
    if let Some(found) = ft(d) {
        return found;
    }
    let mut parent = doc.resolve(d.get(b"Parent")?);
    for _ in 1..MAX_FIELD_DEPTH {
        let field = parent.as_dict()?;
        if let Some(found) = ft(field) {
            return found;
        }
        let next = doc.resolve(field.get(b"Parent")?);
        parent = next;
    }
    None
}
