//! Post-processing of Chrome's PDF: everything a browser cannot emit.

use anyhow::{Context, Result, bail};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, dictionary};

use super::annots::{Annotator, Kind, Rect, embedded_file};
use super::form::{self, FormInputs};
use super::pdf::{Content, Font, Fonts, Stamp, colors, name, text};

/// Links to this prefix in showcase.html mark where annotations go.
const MARKER_PREFIX: &str = "https://mark.pdfcraft.invalid/";

const TITLE: &str = "PlutarchPages Showcase";
const AUTHOR: &str = "PdfCraft";
const SUBJECT: &str = "A specimen document exercising typography, world scripts, vector graphics, \
                       MathML, forms, annotations, layers and attachments.";
const KEYWORDS: &[&str] = &["PDF", "typography", "OpenType", "AcroForm", "annotations", "optional content", "PdfCraft"];
const CREATOR: &str = "pdfcraft xtask demo-pdf";

/// Chrome-printed page indices (0-based) that the post-processor refers to.
const PAGE_CONTENTS: usize = 1;
const PAGE_SETTING_TEXT: usize = 3;
const PAGE_REVIEW: usize = 11;
const CHROME_PAGES: usize = 12;

pub struct Summary {
    pub pages: usize,
    pub annotations: usize,
    pub fields: usize,
    pub outline: String,
}

pub fn enhance(doc: &mut Document, now: Stamp) -> Result<Summary> {
    doc.version = "1.7".into();
    let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
    if pages.len() != CHROME_PAGES {
        bail!(
            "Chrome produced {} pages but the showcase layout expects {CHROME_PAGES}; \
             a page probably overflowed - check dist/demo/build/chrome.pdf",
            pages.len()
        );
    }
    let fonts = Fonts::add_to(doc);
    let mut annotator = Annotator::new(fonts.clone(), now.clone());
    let mut annotation_count = 0;

    // 1. Marker links -> real annotations.
    for (index, &page) in pages.iter().enumerate() {
        let mut annots = page_annots(doc, page)?;
        let mut seen: Vec<String> = Vec::new();
        let mut keep = Vec::with_capacity(annots.len());
        let mut extra = Vec::new();
        for id in annots.drain(..) {
            let Some((uri, r)) = marker_link(doc, id) else {
                keep.push(id);
                continue;
            };
            if seen.contains(&uri) {
                continue; // a marker that wrapped onto two lines: keep only the first box
            }
            seen.push(uri.clone());
            let kind_str = uri[MARKER_PREFIX.len()..].split('/').next().unwrap_or_default();
            let kind = Kind::from_marker(kind_str).with_context(|| format!("unknown annotation marker {uri} on page {}", index + 1))?;
            let struct_parent = doc.get_dictionary(id)?.get(b"StructParent").ok().cloned();
            let built = annotator.build(doc, page, id, kind, r);
            let mut dict = built.dict;
            if let Some(sp) = struct_parent {
                dict.set("StructParent", sp);
            }
            doc.objects.insert(id, Object::Dictionary(dict));
            keep.push(id);
            annotation_count += 1 + built.extra.len();
            extra.extend(built.extra);
        }
        keep.extend(extra);
        if index == PAGE_SETTING_TEXT {
            keep.push(annotator.simple_note(
                doc,
                page,
                572.0,
                704.0,
                "Drop cap, small-caps first line and old-style figures are all plain glyph placement.",
            ));
            annotation_count += 2;
        }
        set_page_annots(doc, page, keep)?;
    }

    // 2. Optional content: a visible draft watermark and hidden print-only notes.
    let ocg_draft = doc.add_object(dictionary! {
        "Type" => "OCG",
        "Name" => text("Draft watermark"),
        "Intent" => "View",
        "Usage" => dictionary! {
            "CreatorInfo" => dictionary! { "Creator" => text(CREATOR), "Subtype" => "Artwork" },
            "Watermark" => dictionary! { "Subtype" => "Watermark" },
        },
    });
    let ocg_notes = doc.add_object(dictionary! {
        "Type" => "OCG",
        "Name" => text("Print-only notes"),
        "Usage" => dictionary! {
            "Print" => dictionary! { "Subtype" => "Watermark", "PrintState" => "ON" },
            "View" => dictionary! { "ViewState" => "OFF" },
        },
    });
    add_watermark(doc, pages[PAGE_REVIEW], ocg_draft, &fonts)?;

    // 3. Appended AcroForm page.
    let pages_root = doc.catalog()?.get(b"Pages")?.as_reference()?;
    let form_page = form::build(
        doc,
        &mut annotator,
        FormInputs { fonts: &fonts, pages_root, contents_page: pages[PAGE_CONTENTS], print_only_ocg: ocg_notes, folio: "10" },
    );
    {
        let root = doc.get_dictionary_mut(pages_root)?;
        let count = root.get(b"Count")?.as_i64()?;
        root.get_mut(b"Kids")?.as_array_mut()?.push(Object::Reference(form_page.page));
        root.set("Count", count + 1);
    }
    annotation_count += 3; // two links and a note (the rest are widgets)
    let field_count = form_page.fields.len();
    let acroform = doc.add_object(form::acroform(&fonts, &form_page.fields));

    let mut all_pages = pages.clone();
    all_pages.push(form_page.page);

    // 4. Outline.
    let outline = extend_outline(doc, &all_pages, form_page.page)?;

    // 5. Embedded file, metadata and catalog entries.
    let csv = include_str!("../../../assets/demo/showcase-data.csv");
    let csv_spec = embedded_file(doc, "showcase-data.csv", "text/csv", csv.as_bytes(), "Sample data behind the charts on page 7", &now);
    doc.get_dictionary_mut(csv_spec)?.set("AFRelationship", "Data");
    add_embedded_file_name(doc, "showcase-data.csv", csv_spec)?;

    let producer = producer(doc);
    let info = doc.add_object(info_dict(&producer, &now));
    doc.trailer.set("Info", info);
    let xmp_stream = Stream::new(dictionary! { "Type" => "Metadata", "Subtype" => "XML" }, xmp(&producer, &now).into_bytes());
    let xmp = doc.add_object(xmp_stream.with_compression(false));

    let catalog = doc.catalog_mut()?;
    catalog.set("Metadata", xmp);
    catalog.set("AcroForm", acroform);
    catalog.set("PageMode", "UseOutlines");
    catalog.set("ViewerPreferences", dictionary! { "DisplayDocTitle" => true });
    catalog.set("AF", vec![Object::Reference(csv_spec)]);
    if !catalog.has(b"Lang") {
        catalog.set("Lang", Object::string_literal("en-US"));
    }
    catalog.set(
        "PageLabels",
        dictionary! {
            "Nums" => vec![
                0.into(), dictionary! { "P" => text("Cover") }.into(),
                1.into(), dictionary! { "S" => "r" }.into(),
                3.into(), dictionary! { "S" => "D" }.into(),
            ],
        },
    );
    catalog.set(
        "OCProperties",
        dictionary! {
            "OCGs" => vec![Object::Reference(ocg_draft), Object::Reference(ocg_notes)],
            "D" => dictionary! {
                "Name" => text("Showcase layers"),
                "Creator" => text(CREATOR),
                "BaseState" => "ON",
                "ON" => vec![Object::Reference(ocg_draft)],
                "OFF" => vec![Object::Reference(ocg_notes)],
                "Order" => vec![Object::Reference(ocg_draft), Object::Reference(ocg_notes)],
                "AS" => vec![
                    dictionary! { "Event" => "Print", "Category" => vec![name("Print")], "OCGs" => vec![Object::Reference(ocg_notes)] }.into(),
                    dictionary! { "Event" => "View", "Category" => vec![name("View")], "OCGs" => vec![Object::Reference(ocg_notes)] }.into(),
                ],
            },
        },
    );

    Ok(Summary { pages: all_pages.len(), annotations: annotation_count, fields: field_count, outline })
}

/// If `id` is one of Chrome's link annotations pointing at a marker URI, return it with its rect.
fn marker_link(doc: &Document, id: ObjectId) -> Option<(String, Rect)> {
    let dict = doc.get_dictionary(id).ok()?;
    if dict.get(b"Subtype").and_then(Object::as_name).ok()? != b"Link" {
        return None;
    }
    let action = dict.get_deref(b"A", doc).and_then(Object::as_dict).ok()?;
    let uri = action.get_deref(b"URI", doc).and_then(Object::as_str).ok()?;
    let uri = String::from_utf8_lossy(uri).into_owned();
    if !uri.starts_with(MARKER_PREFIX) {
        return None;
    }
    let r = Rect::from_obj(dict.get(b"Rect").ok()?)?;
    Some((uri, r))
}

fn page_annots(doc: &Document, page: ObjectId) -> Result<Vec<ObjectId>> {
    let dict = doc.get_dictionary(page)?;
    let Ok(obj) = dict.get_deref(b"Annots", doc) else {
        return Ok(Vec::new());
    };
    Ok(obj.as_array()?.iter().filter_map(|o| o.as_reference().ok()).collect())
}

fn set_page_annots(doc: &mut Document, page: ObjectId, annots: Vec<ObjectId>) -> Result<()> {
    let dict = doc.get_dictionary_mut(page)?;
    if annots.is_empty() {
        dict.remove(b"Annots");
    } else {
        dict.set("Annots", annots.into_iter().map(Object::Reference).collect::<Vec<_>>());
        dict.set("Tabs", "S");
    }
    Ok(())
}

/// Mutable access to a page's own `/Resources` dictionary (direct or indirect).
fn resources_mut(doc: &mut Document, page: ObjectId) -> Result<&mut Dictionary> {
    match doc.get_dictionary(page)?.get(b"Resources").ok().cloned() {
        Some(Object::Reference(id)) => Ok(doc.get_dictionary_mut(id)?),
        Some(Object::Dictionary(_)) => Ok(doc.get_dictionary_mut(page)?.get_mut(b"Resources")?.as_dict_mut()?),
        _ => {
            let page_dict = doc.get_dictionary_mut(page)?;
            page_dict.set("Resources", Dictionary::new());
            Ok(page_dict.get_mut(b"Resources")?.as_dict_mut()?)
        }
    }
}

/// Insert `key => value` into the `category` sub-dictionary of a page's resources.
fn add_resource(doc: &mut Document, page: ObjectId, category: &str, key: &str, value: Object) -> Result<()> {
    let indirect = match resources_mut(doc, page)?.get(category.as_bytes()) {
        Ok(Object::Reference(id)) => Some(*id),
        _ => None,
    };
    let sub = match indirect {
        Some(id) => doc.get_dictionary_mut(id)?,
        None => {
            let res = resources_mut(doc, page)?;
            if !res.has(category.as_bytes()) {
                res.set(category, Dictionary::new());
            }
            res.get_mut(category.as_bytes())?.as_dict_mut()?
        }
    };
    sub.set(key, value);
    Ok(())
}

/// Wrap the page's existing content in q/Q and append a diagonal "DRAFT" on layer `ocg`.
fn add_watermark(doc: &mut Document, page: ObjectId, ocg: ObjectId, fonts: &Fonts) -> Result<()> {
    add_resource(doc, page, "Properties", "PCDraft", Object::Reference(ocg))?;
    add_resource(doc, page, "Font", "PCDraftFont", Object::Reference(fonts.id(Font::HelvBold)))?;
    add_resource(doc, page, "ExtGState", "PCDraftGS", dictionary! { "Type" => "ExtGState", "ca" => 0.09, "CA" => 0.09 }.into())?;

    let size = 150.0f32;
    let word = "DRAFT";
    let tw = Font::HelvBold.width(word, size);
    let cap = 0.72 * size;
    let (sin, cos) = 52f32.to_radians().sin_cos();
    let (cx, cy) = (306.0, 396.0);
    let x = cx - (cos * tw / 2.0 - sin * cap / 2.0);
    let y = cy - (sin * tw / 2.0 + cos * cap / 2.0);
    let mut c = Content::new();
    c.raw("Q").save();
    c.raw("/OC /PCDraft BDC /Artifact << /Type /Pagination /Subtype /Watermark >> BDC");
    c.gs("PCDraftGS").fill_color(colors::ACCENT);
    c.raw(&format!("BT /PCDraftFont {size} Tf {cos:.4} {sin:.4} {:.4} {cos:.4} {x:.2} {y:.2} Tm (DRAFT) Tj ET", -sin));
    c.raw("EMC EMC").restore();

    let open = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let close = doc.add_object(Stream::new(Dictionary::new(), c.into_bytes()));
    let mut contents = vec![Object::Reference(open)];
    let page_dict = doc.get_dictionary(page)?;
    match page_dict.get(b"Contents")? {
        Object::Reference(id) => match doc.get_object(*id)? {
            Object::Array(items) => contents.extend(items.iter().cloned()),
            _ => contents.push(Object::Reference(*id)),
        },
        Object::Array(items) => contents.extend(items.iter().cloned()),
        other => bail!("unexpected /Contents {other:?}"),
    }
    contents.push(Object::Reference(close));
    doc.get_dictionary_mut(page)?.set("Contents", contents);
    Ok(())
}

/// Keep Chrome's heading-based outline (tidying its titles) and add the form
/// page next to the other chapters; if Chrome produced none, build one.
fn extend_outline(doc: &mut Document, pages: &[ObjectId], form_page: ObjectId) -> Result<String> {
    let dest = |page: ObjectId, top: i64| -> Object { vec![Object::Reference(page), name("XYZ"), 0.into(), top.into(), Object::Null].into() };
    let existing = doc
        .catalog()?
        .get(b"Outlines")
        .and_then(Object::as_reference)
        .ok()
        .filter(|id| doc.get_dictionary(*id).map(|d| d.has(b"First")).unwrap_or(false));

    let (chapter_parent, description) = match existing {
        Some(root) => {
            let mut items = Vec::new();
            collect_outline(doc, root, &mut items);
            let mut parent = root;
            for id in items {
                let dict = doc.get_dictionary_mut(id)?;
                let title = decode_text(dict.get(b"Title").and_then(Object::as_str).unwrap_or_default());
                if title == "Review & Markup" {
                    parent = dict.get(b"Parent").and_then(Object::as_reference).unwrap_or(root);
                }
                let tidy = tidy_title(&title);
                if tidy != title {
                    dict.set("Title", text(&tidy));
                }
            }
            (parent, "Chrome headings (tidied) + form page")
        }
        None => {
            let root = doc.add_object(dictionary! { "Type" => "Outlines", "Count" => 0 });
            let chapters = [
                "Cover",
                "Contents",
                "Foreword",
                "Setting Text",
                "OpenType Features",
                "Expressive Type",
                "Scripts of the World",
                "Mathematics",
                "Vector Graphics",
                "Data & Tables",
                "Code & Images",
                "Review & Markup",
            ];
            for (index, title) in chapters.into_iter().enumerate() {
                append_outline_item(doc, root, title, dest(pages[index], 792), None)?;
            }
            doc.catalog_mut()?.set("Outlines", root);
            (root, "built by xtask (Chrome emitted none)")
        }
    };

    let form_item = append_outline_item(doc, chapter_parent, "Interactive Form", dest(form_page, 792), Some(colors::ACCENT.array()))?;
    for (title, top) in [("Contact", 620), ("Preferences", 426), ("Actions & signature", 240)] {
        append_outline_item(doc, form_item, title, dest(form_page, top), None)?;
    }
    let root = doc.catalog()?.get(b"Outlines")?.as_reference()?;
    let total = count_outline(doc, root);
    Ok(format!("{description}, {total} entries"))
}

/// Every outline item below `node`, depth first.
fn collect_outline(doc: &Document, node: ObjectId, out: &mut Vec<ObjectId>) {
    let next = |id: ObjectId, key: &[u8]| doc.get_dictionary(id).ok().and_then(|d| d.get(key).and_then(Object::as_reference).ok());
    let mut cur = next(node, b"First");
    while let Some(id) = cur {
        out.push(id);
        collect_outline(doc, id, out);
        cur = next(id, b"Next");
    }
}

/// Decode a PDF text string (UTF-16BE with BOM, else treated as Latin-1).
fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Chrome copies CSS `text-transform: uppercase` into bookmark titles; turn
/// all-caps headings back into sentence case (keeping acronyms).
fn tidy_title(title: &str) -> String {
    const ACRONYMS: &[&str] = &["PDF"];
    // Chrome drops the <br> between the two lines of the cover title.
    if title == "PdfCraftShowcase" {
        return TITLE.to_string();
    }
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().any(char::is_lowercase) || !title.chars().any(char::is_uppercase) {
        return title;
    }
    title
        .split(' ')
        .enumerate()
        .map(|(i, word)| {
            let bare = word.trim_matches(|c: char| !c.is_alphanumeric());
            if ACRONYMS.contains(&bare) {
                return word.to_string();
            }
            let lower = word.to_lowercase();
            if i == 0 {
                let mut chars = lower.chars();
                chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
            } else {
                lower
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Append a child item to outline node `parent`, updating First/Last and the
/// /Count of every open ancestor.
fn append_outline_item(doc: &mut Document, parent: ObjectId, title: &str, dest: Object, color: Option<Object>) -> Result<ObjectId> {
    let last = doc.get_dictionary(parent)?.get(b"Last").and_then(Object::as_reference).ok();
    let mut item = dictionary! { "Title" => text(title), "Parent" => parent, "Dest" => dest };
    if let Some(color) = color {
        item.set("C", color);
        item.set("F", 2); // bold
    }
    if let Some(last) = last {
        item.set("Prev", last);
    }
    let id = doc.add_object(item);
    if let Some(last) = last {
        doc.get_dictionary_mut(last)?.set("Next", id);
    }
    let p = doc.get_dictionary_mut(parent)?;
    if last.is_none() {
        p.set("First", id);
    }
    p.set("Last", id);

    // Open nodes count visible descendants (positive); a closed node's negative
    // count grows in magnitude and hides the change from its ancestors.
    let mut node = Some(parent);
    while let Some(n) = node {
        let d = doc.get_dictionary_mut(n)?;
        let count = d.get(b"Count").and_then(Object::as_i64).unwrap_or(0);
        if count < 0 {
            d.set("Count", count - 1);
            break;
        }
        d.set("Count", count + 1);
        node = d.get(b"Parent").and_then(Object::as_reference).ok();
    }
    Ok(id)
}

fn count_outline(doc: &Document, node: ObjectId) -> usize {
    let mut n = 0;
    let mut cur = doc.get_dictionary(node).ok().and_then(|d| d.get(b"First").and_then(Object::as_reference).ok());
    while let Some(id) = cur {
        n += 1 + count_outline(doc, id);
        cur = doc.get_dictionary(id).ok().and_then(|d| d.get(b"Next").and_then(Object::as_reference).ok());
    }
    n
}

fn add_embedded_file_name(doc: &mut Document, file_name: &str, spec: ObjectId) -> Result<()> {
    let names_ref = doc.catalog()?.get(b"Names").and_then(Object::as_reference).ok();
    let names: &mut Dictionary = match names_ref {
        Some(id) => doc.get_dictionary_mut(id)?,
        None => {
            let catalog = doc.catalog_mut()?;
            if !catalog.has(b"Names") {
                catalog.set("Names", Dictionary::new());
            }
            catalog.get_mut(b"Names")?.as_dict_mut()?
        }
    };
    names.set("EmbeddedFiles", dictionary! { "Names" => vec![Object::string_literal(file_name), Object::Reference(spec)] });
    Ok(())
}

/// Chrome's producer string with this tool appended.
fn producer(doc: &Document) -> String {
    let chrome = doc
        .trailer
        .get_deref(b"Info", doc)
        .and_then(Object::as_dict)
        .and_then(|d| d.get(b"Producer"))
        .and_then(Object::as_str)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .unwrap_or_else(|_| "Chrome".into());
    format!("{chrome}; lopdf 0.45 via pdfcraft xtask")
}

fn info_dict(producer: &str, now: &Stamp) -> Dictionary {
    dictionary! {
        "Title" => text(TITLE),
        "Author" => text(AUTHOR),
        "Subject" => text(SUBJECT),
        "Keywords" => text(&KEYWORDS.join(", ")),
        "Creator" => text(CREATOR),
        "Producer" => text(producer),
        "CreationDate" => Object::string_literal(now.pdf.clone()),
        "ModDate" => Object::string_literal(now.pdf.clone()),
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn xmp(producer: &str, now: &Stamp) -> String {
    // A stable-looking document id derived from the timestamp (FNV-1a).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in now.iso.bytes().chain(TITLE.bytes()) {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let uuid = format!("{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}", h >> 32, (h >> 16) & 0xffff, h & 0xfff, (h >> 20) & 0xfff, h & 0xffff_ffff_ffff);
    let keywords: String = KEYWORDS.iter().map(|k| format!("<rdf:li>{}</rdf:li>", xml_escape(k))).collect();
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\"\n\
    xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\">\n\
   <dc:format>application/pdf</dc:format>\n\
   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{title}</rdf:li></rdf:Alt></dc:title>\n\
   <dc:creator><rdf:Seq><rdf:li>{author}</rdf:li></rdf:Seq></dc:creator>\n\
   <dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{subject}</rdf:li></rdf:Alt></dc:description>\n\
   <dc:subject><rdf:Bag>{keywords}</rdf:Bag></dc:subject>\n\
   <xmp:CreatorTool>{creator}</xmp:CreatorTool>\n\
   <xmp:CreateDate>{date}</xmp:CreateDate>\n\
   <xmp:ModifyDate>{date}</xmp:ModifyDate>\n\
   <xmp:MetadataDate>{date}</xmp:MetadataDate>\n\
   <pdf:Keywords>{kw_flat}</pdf:Keywords>\n\
   <pdf:Producer>{producer}</pdf:Producer>\n\
   <xmpMM:DocumentID>uuid:{uuid}</xmpMM:DocumentID>\n\
   <xmpMM:InstanceID>uuid:{uuid}</xmpMM:InstanceID>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n\
{padding}\n\
<?xpacket end=\"w\"?>",
        title = xml_escape(TITLE),
        author = xml_escape(AUTHOR),
        subject = xml_escape(SUBJECT),
        creator = xml_escape(CREATOR),
        date = now.iso,
        kw_flat = xml_escape(&KEYWORDS.join(", ")),
        producer = xml_escape(producer),
        padding = " ".repeat(1024),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo_pdf::pdf::rect;

    const KINDS: [&str; 15] = [
        "highlight",
        "underline",
        "strikeout",
        "squiggly",
        "caret",
        "note",
        "freetext",
        "line",
        "square",
        "circle",
        "polygon",
        "polyline",
        "ink",
        "stamp",
        "attach",
    ];

    /// How the review page stores its `/Contents` and `/Resources`, and what else the catalog carries.
    #[derive(Clone, Copy)]
    struct Shape {
        contents: u8,
        resources: u8,
        outline: bool,
        names: u8,
        producer: bool,
        lang: bool,
    }

    fn link(doc: &mut Document, uri: &str, rect: Object) -> ObjectId {
        doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "Rect" => rect,
            "A" => dictionary! { "S" => "URI", "URI" => Object::string_literal(uri) },
        })
    }

    fn utf16(s: &str) -> Object {
        let mut bytes = vec![0xfe, 0xff];
        bytes.extend(s.encode_utf16().flat_map(u16::to_be_bytes));
        Object::String(bytes, lopdf::StringFormat::Hexadecimal)
    }

    /// A twelve-page document shaped like Chrome's print of the showcase.
    fn chrome_like(shape: Shape) -> (Document, Vec<ObjectId>) {
        let mut doc = Document::with_version("1.4");
        let pages_root = doc.new_object_id();
        let mut pages = Vec::new();
        for index in 0..CHROME_PAGES {
            let content = doc.add_object(Stream::new(Dictionary::new(), format!("BT ({index}) Tj ET").into_bytes()));
            let mut page = dictionary! {
                "Type" => "Page",
                "Parent" => pages_root,
                "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                "Contents" => content,
            };
            if index == PAGE_REVIEW {
                match shape.contents {
                    0 => {}
                    1 => {
                        let array = doc.add_object(vec![Object::Reference(content)]);
                        page.set("Contents", array);
                    }
                    2 => page.set("Contents", vec![Object::Reference(content)]),
                    _ => page.set("Contents", 7),
                }
                match shape.resources {
                    0 => {}
                    1 => page.set("Resources", dictionary! { "Font" => dictionary! { "F1" => Object::Null } }),
                    _ => {
                        let fonts = doc.add_object(dictionary! { "F1" => Object::Null });
                        let res = doc.add_object(dictionary! { "Font" => fonts });
                        page.set("Resources", res);
                    }
                }
            }
            pages.push(doc.add_object(page));
        }

        // Page 1: one marker per kind, a second box of a wrapped marker, an ordinary link, a
        // marker whose rectangle is malformed, and a non-link annotation.
        let mut annots: Vec<Object> = Vec::new();
        for (i, kind) in KINDS.iter().enumerate() {
            let y = 700.0 - i as f32 * 40.0;
            annots.push(link(&mut doc, &format!("{MARKER_PREFIX}{kind}/1"), rect(72.0, y, 300.0, y + 30.0)).into());
        }
        let structured = annots[0].as_reference().unwrap();
        doc.get_dictionary_mut(structured).unwrap().set("StructParent", 42);
        annots.push(link(&mut doc, &format!("{MARKER_PREFIX}highlight/1"), rect(72.0, 50.0, 100.0, 60.0)).into());
        let plain = link(&mut doc, "https://example.com/", rect(10.0, 10.0, 20.0, 20.0));
        let bad_rect = link(&mut doc, &format!("{MARKER_PREFIX}ink/2"), vec![1.into(), 2.into(), 3.into()].into());
        let text_annot = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Text", "Rect" => rect(0.0, 0.0, 1.0, 1.0) });
        let no_action = doc.add_object(dictionary! { "Type" => "Annot", "Subtype" => "Link", "Rect" => rect(0.0, 0.0, 1.0, 1.0) });
        annots.extend([plain, bad_rect, text_annot, no_action].map(Object::Reference));
        doc.get_dictionary_mut(pages[0]).unwrap().set("Annots", annots);
        // Page 2 keeps its annotations in an indirect array that holds no markers.
        let indirect = doc.add_object(vec![Object::Reference(plain)]);
        doc.get_dictionary_mut(pages[1]).unwrap().set("Annots", indirect);

        doc.objects.insert(
            pages_root,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => pages.iter().map(|&p| Object::Reference(p)).collect::<Vec<_>>(),
                "Count" => CHROME_PAGES as i64,
            }),
        );
        let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_root };
        if shape.lang {
            catalog.set("Lang", Object::string_literal("en-GB"));
        }
        match shape.names {
            0 => {}
            1 => catalog.set("Names", dictionary! { "Dests" => Dictionary::new() }),
            _ => {
                let names = doc.add_object(dictionary! { "Dests" => Dictionary::new() });
                catalog.set("Names", names);
            }
        }
        if shape.outline {
            // Root > CHAPTERS (closed) > [PdfCraftShowcase, "Review & Markup" in UTF-16, ALL-CAPS PDF heading].
            let root = doc.new_object_id();
            let chapters = doc.new_object_id();
            let (cover, review, caps) = (doc.new_object_id(), doc.new_object_id(), doc.new_object_id());
            let item = |title: Object, parent: ObjectId, prev: Option<ObjectId>, next: Option<ObjectId>| {
                let mut d = dictionary! { "Title" => title, "Parent" => parent };
                if let Some(p) = prev {
                    d.set("Prev", p);
                }
                if let Some(n) = next {
                    d.set("Next", n);
                }
                d
            };
            let mut chapters_dict = item(Object::string_literal("CHAPTERS"), root, None, None);
            chapters_dict.extend(&dictionary! { "First" => cover, "Last" => caps, "Count" => -3 });
            doc.objects.insert(chapters, chapters_dict.into());
            doc.objects.insert(cover, item(Object::string_literal("PdfCraftShowcase"), chapters, None, Some(review)).into());
            doc.objects.insert(review, item(utf16("Review & Markup"), chapters, Some(cover), Some(caps)).into());
            doc.objects.insert(caps, item(Object::string_literal("THE  PDF FORMAT, PLAINLY"), chapters, Some(review), None).into());
            doc.objects.insert(root, dictionary! { "Type" => "Outlines", "First" => chapters, "Last" => chapters, "Count" => 1 }.into());
            catalog.set("Outlines", root);
        }
        let catalog = doc.add_object(catalog);
        doc.trailer.set("Root", catalog);
        if shape.producer {
            let info = doc.add_object(dictionary! { "Producer" => Object::string_literal("Skia/PDF m150") });
            doc.trailer.set("Info", info);
        }
        (doc, pages)
    }

    fn now() -> Stamp {
        Stamp::from_unix(1_790_000_000)
    }

    fn subtype(doc: &Document, id: ObjectId) -> String {
        String::from_utf8_lossy(doc.get_dictionary(id).unwrap().get(b"Subtype").unwrap().as_name().unwrap()).into_owned()
    }

    fn refs(obj: &Object) -> Vec<ObjectId> {
        obj.as_array().unwrap().iter().map(|o| o.as_reference().unwrap()).collect()
    }

    fn string(obj: &Object) -> String {
        decode_text(obj.as_str().unwrap())
    }

    fn content_of(doc: &Document, id: ObjectId) -> String {
        String::from_utf8_lossy(&doc.get_object(id).unwrap().as_stream().unwrap().content).into_owned()
    }

    const FULL: Shape = Shape { contents: 0, resources: 2, outline: true, names: 2, producer: true, lang: true };

    #[test]
    fn markers_become_annotations_with_appearances_and_the_form_page_is_appended() {
        let (mut doc, pages) = chrome_like(FULL);
        let summary = enhance(&mut doc, now()).unwrap();
        assert_eq!(doc.version, "1.7");
        assert_eq!(summary.pages, CHROME_PAGES + 1);
        // 15 markers, the note's popup, reply and review state, the page-4 note and its popup,
        // and the form page's two links and note.
        assert_eq!(summary.annotations, 15 + 3 + 2 + 3);
        assert_eq!(summary.fields, 13);
        assert_eq!(summary.outline, "Chrome headings (tidied) + form page, 8 entries");
        assert_marker_pages(&doc, &pages);
        assert_form_and_catalog(&doc);
        assert_watermark_and_outline(&doc, pages[PAGE_REVIEW]);

        // The result survives a save and reload.
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        assert_eq!(Document::load_mem(&bytes).unwrap().get_pages().len(), 13);
    }

    fn assert_marker_pages(doc: &Document, pages: &[ObjectId]) {
        let page1 = doc.get_dictionary(pages[0]).unwrap();
        assert_eq!(page1.get(b"Tabs").unwrap().as_name().unwrap(), b"S");
        let annots = refs(page1.get(b"Annots").unwrap());
        let kinds: Vec<String> = annots.iter().map(|&id| subtype(doc, id)).collect();
        let want = [
            "Highlight",
            "Underline",
            "StrikeOut",
            "Squiggly",
            "Caret",
            "Text",
            "FreeText",
            "Line",
            "Square",
            "Circle",
            "Polygon",
            "PolyLine",
            "Ink",
            "Stamp",
            "FileAttachment",
            // Kept as they were: the ordinary link, the malformed marker, the Text, the action-less link.
            "Link",
            "Link",
            "Text",
            "Link",
            // The sticky note's popup, reply and review state.
            "Popup",
            "Text",
            "Text",
        ];
        assert_eq!(kinds, want);
        // Every converted marker carries an appearance stream; the first kept Chrome's StructParent.
        for &id in &annots[..15] {
            let ap = doc.get_dictionary(id).unwrap().get(b"AP").unwrap().as_dict().unwrap().get(b"N").unwrap().as_reference().unwrap();
            assert!(doc.get_object(ap).unwrap().as_stream().is_ok(), "{}", subtype(doc, id));
        }
        assert_eq!(doc.get_dictionary(annots[0]).unwrap().get(b"StructParent").unwrap().as_i64().unwrap(), 42);
        let highlight = doc.get_dictionary(annots[0]).unwrap();
        assert_eq!(string(highlight.get(b"NM").unwrap()), "pdfcraft-showcase-001-highlight");
        assert_eq!(highlight.get(b"QuadPoints").unwrap().as_array().unwrap().len(), 8);
        let reply = doc.get_dictionary(annots[20]).unwrap();
        assert_eq!(reply.get(b"IRT").unwrap().as_reference().unwrap(), annots[5]);
        let state = doc.get_dictionary(annots[21]).unwrap();
        assert_eq!(state.get(b"State").unwrap().as_str().unwrap(), b"Accepted");
        let attachment = doc.get_dictionary(annots[14]).unwrap();
        let spec = doc.get_dictionary(attachment.get(b"FS").unwrap().as_reference().unwrap()).unwrap();
        assert_eq!(string(spec.get(b"F").unwrap()), "review-notes.txt");

        // Page 2's indirect array had no markers and stays as it was; pages with none keep none.
        assert_eq!(refs(doc.get_dictionary(pages[1]).unwrap().get(b"Annots").unwrap()).len(), 1);
        assert!(!doc.get_dictionary(pages[2]).unwrap().has(b"Annots"));
        let page4 = refs(doc.get_dictionary(pages[PAGE_SETTING_TEXT]).unwrap().get(b"Annots").unwrap());
        assert_eq!(page4.len(), 1);
        assert_eq!(subtype(doc, page4[0]), "Text");
    }

    fn assert_form_and_catalog(doc: &Document) {
        // The form page is the thirteenth kid, with every field in the AcroForm.
        let catalog = doc.catalog().unwrap();
        let pages_root = catalog.get(b"Pages").unwrap().as_reference().unwrap();
        let root = doc.get_dictionary(pages_root).unwrap();
        assert_eq!(root.get(b"Count").unwrap().as_i64().unwrap(), 13);
        let form_page = refs(root.get(b"Kids").unwrap())[12];
        let acroform = doc.get_dictionary(catalog.get(b"AcroForm").unwrap().as_reference().unwrap()).unwrap();
        assert_eq!(acroform.get(b"Fields").unwrap().as_array().unwrap().len(), 13);
        let form_annots = refs(doc.get_dictionary(form_page).unwrap().get(b"Annots").unwrap());
        let links: Vec<_> = form_annots.iter().filter(|&&id| subtype(doc, id) == "Link").collect();
        assert_eq!(links.len(), 2);

        // Catalog entries, metadata and the embedded data file.
        assert_eq!(catalog.get(b"Lang").unwrap().as_str().unwrap(), b"en-GB", "an existing /Lang is kept");
        assert_eq!(catalog.get(b"PageMode").unwrap().as_name().unwrap(), b"UseOutlines");
        let names = doc.get_dictionary(catalog.get(b"Names").unwrap().as_reference().unwrap()).unwrap();
        assert!(names.has(b"Dests"));
        let embedded = names.get(b"EmbeddedFiles").unwrap().as_dict().unwrap().get(b"Names").unwrap().as_array().unwrap();
        assert_eq!(embedded[0].as_str().unwrap(), b"showcase-data.csv");
        let info = doc.get_dictionary(doc.trailer.get(b"Info").unwrap().as_reference().unwrap()).unwrap();
        assert_eq!(string(info.get(b"Producer").unwrap()), "Skia/PDF m150; lopdf 0.45 via pdfcraft xtask");
        assert_eq!(string(info.get(b"Title").unwrap()), TITLE);
        let xmp = content_of(doc, catalog.get(b"Metadata").unwrap().as_reference().unwrap());
        assert!(xmp.contains("<pdf:Producer>Skia/PDF m150; lopdf 0.45 via pdfcraft xtask</pdf:Producer>"), "{xmp}");
        assert!(xmp.contains("<xmp:CreateDate>2026-09-21T14:13:20Z</xmp:CreateDate>"));
        let ocgs = catalog.get(b"OCProperties").unwrap().as_dict().unwrap().get(b"OCGs").unwrap().as_array().unwrap();
        assert_eq!(ocgs.len(), 2);
    }

    fn assert_watermark_and_outline(doc: &Document, review: ObjectId) {
        // The watermark wraps the review page's own content in q/Q.
        let review = doc.get_dictionary(review).unwrap();
        let contents = refs(review.get(b"Contents").unwrap());
        assert_eq!(contents.len(), 3);
        assert_eq!(content_of(doc, contents[0]), "q\n");
        assert_eq!(content_of(doc, contents[1]), "BT (11) Tj ET");
        assert!(content_of(doc, contents[2]).contains("(DRAFT) Tj"));
        let res = doc.get_dictionary(review.get(b"Resources").unwrap().as_reference().unwrap()).unwrap();
        assert!(res.get(b"Properties").unwrap().as_dict().unwrap().has(b"PCDraft"));
        let fonts = doc.get_dictionary(res.get(b"Font").unwrap().as_reference().unwrap()).unwrap();
        assert!(fonts.has(b"F1") && fonts.has(b"PCDraftFont"), "the indirect font dictionary gains the watermark font");

        let catalog = doc.catalog().unwrap();
        // Outline: titles tidied, the form chapter added beside "Review & Markup" in its closed parent.
        let outline_root = doc.get_dictionary(catalog.get(b"Outlines").unwrap().as_reference().unwrap()).unwrap();
        assert_eq!(outline_root.get(b"Count").unwrap().as_i64().unwrap(), 1, "a closed child hides the change from the root");
        let chapters = doc.get_dictionary(outline_root.get(b"First").unwrap().as_reference().unwrap()).unwrap();
        assert_eq!(chapters.get(b"Count").unwrap().as_i64().unwrap(), -7, "each added item grows the closed count");
        assert_eq!(string(chapters.get(b"Title").unwrap()), "Chapters");
        let mut titles = Vec::new();
        let mut cur = chapters.get(b"First").unwrap().as_reference().ok();
        while let Some(id) = cur {
            let d = doc.get_dictionary(id).unwrap();
            titles.push(string(d.get(b"Title").unwrap()));
            cur = d.get(b"Next").and_then(Object::as_reference).ok();
        }
        assert_eq!(titles, [TITLE, "Review & Markup", "The PDF format, plainly", "Interactive Form"]);
    }

    #[test]
    fn an_outline_is_built_when_chrome_emits_none_and_other_storage_shapes_are_handled() {
        for (contents, resources, names) in [(1, 0, 0), (2, 1, 1)] {
            let shape = Shape { contents, resources, outline: false, names, producer: false, lang: false };
            let (mut doc, pages) = chrome_like(shape);
            let summary = enhance(&mut doc, now()).unwrap();
            assert_eq!(summary.outline, "built by xtask (Chrome emitted none), 16 entries");

            let catalog = doc.catalog().unwrap();
            assert_eq!(catalog.get(b"Lang").unwrap().as_str().unwrap(), b"en-US");
            let root = doc.get_dictionary(catalog.get(b"Outlines").unwrap().as_reference().unwrap()).unwrap();
            assert_eq!(root.get(b"Count").unwrap().as_i64().unwrap(), 16);
            let last = doc.get_dictionary(root.get(b"Last").unwrap().as_reference().unwrap()).unwrap();
            assert_eq!(string(last.get(b"Title").unwrap()), "Interactive Form");
            assert_eq!(last.get(b"Count").unwrap().as_i64().unwrap(), 3);
            assert_eq!(last.get(b"F").unwrap().as_i64().unwrap(), 2);

            let names = catalog.get(b"Names").unwrap().as_dict().unwrap();
            assert!(names.has(b"EmbeddedFiles"));
            assert_eq!(names.has(b"Dests"), shape.names != 0);
            let info = doc.get_dictionary(doc.trailer.get(b"Info").unwrap().as_reference().unwrap()).unwrap();
            assert_eq!(string(info.get(b"Producer").unwrap()), "Chrome; lopdf 0.45 via pdfcraft xtask");

            let review = doc.get_dictionary(pages[PAGE_REVIEW]).unwrap();
            let contents = refs(review.get(b"Contents").unwrap());
            assert_eq!(contents.len(), 3, "an array of one stream, direct or indirect, is spliced in");
            assert_eq!(content_of(&doc, contents[1]), "BT (11) Tj ET");
            let res = review.get(b"Resources").unwrap().as_dict().unwrap();
            assert!(res.get(b"Font").unwrap().as_dict().unwrap().has(b"PCDraftFont"));
            assert!(res.get(b"ExtGState").unwrap().as_dict().unwrap().has(b"PCDraftGS"));
        }
    }

    fn failure(doc: &mut Document) -> String {
        enhance(doc, now()).err().expect("enhance refuses a malformed document").to_string()
    }

    #[test]
    fn layout_and_marker_errors_are_reported() {
        let (mut doc, pages) = chrome_like(FULL);
        let pages_root = doc.catalog().unwrap().get(b"Pages").unwrap().as_reference().unwrap();
        let root = doc.get_dictionary_mut(pages_root).unwrap();
        root.set("Kids", vec![Object::Reference(pages[0])]);
        root.set("Count", 1);
        let err = failure(&mut doc);
        assert!(err.starts_with("Chrome produced 1 pages but the showcase layout expects 12"), "{err}");

        let (mut doc, pages) = chrome_like(FULL);
        let unknown = link(&mut doc, &format!("{MARKER_PREFIX}sparkle/1"), rect(0.0, 0.0, 10.0, 10.0));
        doc.get_dictionary_mut(pages[2]).unwrap().set("Annots", vec![Object::Reference(unknown)]);
        let err = failure(&mut doc);
        assert_eq!(err, format!("unknown annotation marker {MARKER_PREFIX}sparkle/1 on page 3"));

        let (mut doc, _) = chrome_like(Shape { contents: 3, ..FULL });
        let err = failure(&mut doc);
        assert!(err.starts_with("unexpected /Contents"), "{err}");
    }

    #[test]
    fn titles_and_text_are_decoded_and_tidied() {
        assert_eq!(decode_text(b"caf\xe9"), "café");
        assert_eq!(decode_text(&[0xfe, 0xff, 0x00, 0x41, 0x20, 0xac]), "A€");
        assert_eq!(tidy_title("PdfCraftShowcase"), TITLE);
        assert_eq!(tidy_title("OPENTYPE  FEATURES"), "Opentype features");
        assert_eq!(tidy_title("WHY PDF/A?"), "Why pdf/a?");
        assert_eq!(tidy_title("ABOUT PDF"), "About PDF");
        assert_eq!(tidy_title("Mixed Case"), "Mixed Case");
        assert_eq!(tidy_title("2026 — 27"), "2026 — 27");
        assert_eq!(xml_escape("a<b>&c"), "a&lt;b&gt;&amp;c");
    }
}
