//! Places [`crate::apply`] leaves as they are that may still hold the removed text (#2448
//! review): bookmark titles, attachments, the comments that remain, and XMP on objects other than
//! the catalog and the pages. Removing them is Remove hidden information's job (its bookmarks,
//! attachments, comments and metadata categories); applying redactions only counts them, so the
//! user can be told. Nothing here changes the document.
//!
//! The text is matched as the metadata check matches it ([`crate::metadata::Search`]), with a
//! work limit of its own. Every walk is bounded: outline items and name-tree nodes are each read
//! once (a seen-set for indirect ones) up to a cap, and the XMP walk is Remove hidden
//! information's own.

use std::collections::HashSet;

use pdfcraft_cos::{Document, ObjRef, Object};

use crate::metadata::Search;

/// Places that may still hold the removed text after [`crate::apply`], which it doesn't change.
/// Remove hidden information removes them. Each count is of places, not of occurrences.
///
/// When the removed text couldn't be searched for (`unsearched`: the text of a removed glyph
/// isn't known, a form was removed unread, or there was too much removed text to collect or
/// search for, as for the metadata check), every place holding any text is counted, since any
/// of them may hold it. A count may also include places that couldn't be read or that lie past
/// a walk's cap (one for each walk cut short), which count as holding it, and, once the search's
/// work limit runs out, every place not yet checked that holds any text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Elsewhere {
    /// Bookmarks (outline items) whose title holds it.
    pub bookmarks: usize,
    /// Embedded files whose name in the document's list of attachments (`/EmbeddedFiles`),
    /// file name (`/F`, `/UF`) or description (`/Desc`) holds it, and file attachment
    /// annotations whose file's name or description does.
    pub attachments: usize,
    /// Annotations left on the pages (not form field widgets or pop-ups) whose text
    /// (`/Contents`, rich text `/RC`, subject `/Subj`) or author (`/T`) holds it.
    pub comments: usize,
    /// XMP streams on objects other than the catalog and the pages (images, fonts, form
    /// XObjects, annotations…) whose text holds it, or that can't be read.
    pub xmp: usize,
    /// Document information (`/Info`) entries that hold it.
    pub info: usize,
    /// Links and actions (a link's or bookmark's action, the open action, additional actions:
    /// their address, file name or script, a script stream included) and the names in the
    /// named destination and document script lists.
    pub links: usize,
    /// Form fields and their widgets whose value, default value, tooltip or other text holds it.
    pub fields: usize,
    /// Any other object whose text holds it (structure tree alternate text, page labels, layer
    /// names, article titles…). No Remove hidden information category removes these, so they
    /// are told but never refuse a save ([`Elsewhere::refuses`]).
    pub other: usize,
    /// The removed text couldn't be searched for, so the counts are of every place that holds
    /// any text.
    pub unsearched: bool,
}

impl Elsewhere {
    /// All the places counted.
    pub fn total(&self) -> usize {
        self.removable().saturating_add(self.other)
    }

    /// The places a Remove hidden information category removes.
    fn removable(&self) -> usize {
        [self.attachments, self.comments, self.xmp, self.info, self.links, self.fields].into_iter().fold(self.bookmarks, usize::saturating_add)
    }

    /// Whether a save must refuse: a place Remove hidden information removes still holds the
    /// text (or, when it couldn't be searched for, any text).
    pub fn refuses(&self) -> bool {
        self.removable() > 0
    }
}

/// The most outline items read.
const MAX_OUTLINE_ITEMS: usize = 100_000;
/// The most name-tree nodes read, and how deep the tree is followed.
const MAX_NAME_TREE_NODES: usize = 100_000;
const MAX_NAME_TREE_DEPTH: usize = 32;

/// Count the places that hold the `removed` text (see [`Elsewhere`]). `complete` is false when
/// the collection of removed text was cut short.
pub(crate) fn find(doc: &Document, removed: &[String], complete: bool) -> Elsewhere {
    let search = Search::of(removed, complete);
    if search.is_nothing() {
        return Elsewhere::default();
    }
    // A blank value holds nothing, even when everything counts.
    let hit = |o: Option<&Object>| {
        o.and_then(|o| doc.resolve(o).as_string().map(crate::metadata::text_string)).is_some_and(|t| !t.trim().is_empty() && search.text(&t))
    };
    let swept = sweep(doc, &search);
    Elsewhere {
        bookmarks: bookmarks(doc, &hit),
        attachments: attachments(doc, &hit).saturating_add(swept.attachments),
        comments: comments(doc, &search, &hit),
        xmp: xmp(doc, &search),
        info: info(doc, &hit),
        links: swept.links,
        fields: swept.fields,
        other: swept.other,
        unsearched: search.is_everything(),
    }
}

/// The removed text an [`crate::apply`] collected, kept so that the file about to be saved can be
/// searched for it again ([`survivors`]). It is never written anywhere.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Removed {
    text: Vec<String>,
    /// The collection wasn't cut short. `false` makes every place holding any text count.
    complete: bool,
}

impl Removed {
    pub(crate) fn new(text: Vec<String>, complete: bool) -> Self {
        Self { text, complete }
    }

    /// Nothing that could hold text was removed.
    pub fn is_empty(&self) -> bool {
        self.complete && self.text.is_empty()
    }

    /// Add what a later apply removed.
    pub fn extend(&mut self, other: &Removed) {
        self.complete &= other.complete;
        self.text.extend(other.text.iter().cloned());
    }
}

/// The places in `doc` (a file about to be saved, read back) that still hold the `removed` text,
/// counted as [`crate::apply`] counts [`crate::Report::still_holding`].
pub fn survivors(doc: &Document, removed: &Removed) -> Elsewhere {
    find(doc, &removed.text, removed.complete)
}

/// `/Info` entries whose text holds it.
fn info(doc: &Document, hit: &dyn Fn(Option<&Object>) -> bool) -> usize {
    let Some(info) = doc.trailer().get(b"Info").and_then(|i| doc.resolve(i).as_dict().cloned()) else { return 0 };
    info.iter().filter(|(_, v)| hit(Some(v))).count()
}

/// The most objects the sweep reads; past it, one more place counts under `other`.
const MAX_SWEEP_OBJECTS: usize = 4_000_000;
/// How deep the sweep follows direct arrays and dictionaries inside one object.
const MAX_SWEEP_DEPTH: usize = 32;

/// What [`sweep`] found, as counts of objects.
#[derive(Default)]
struct Swept {
    attachments: usize,
    links: usize,
    fields: usize,
    other: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Place {
    Attachments,
    Links,
    Fields,
    Other,
}

/// Every object of the file whose strings (or script or embedded file data) hold the text, by
/// place, except what the walks above count (outline items, annotations other than links and
/// widgets, XMP, `/Info`, the embedded file list) and strings that aren't text (the encryption
/// dictionary, signatures). Page content is the redaction's own check.
fn sweep(doc: &Document, search: &Search) -> Swept {
    let hit = |t: &str| !t.trim().is_empty() && search.text(t);
    let mut skip = embedded_file_tree(doc);
    skip.extend([b"Info".as_slice(), b"Encrypt"].into_iter().filter_map(|k| doc.trailer().reference(k)));
    let mut out = Swept::default();
    for (read, (r, o)) in doc.scan_objects_checked().enumerate() {
        if read >= MAX_SWEEP_OBJECTS {
            out.other = out.other.saturating_add(1);
            break;
        }
        if skip.contains(&r) {
            continue;
        }
        let mut found = HashSet::new();
        match o {
            Ok(o) => visit(doc, &o, None, 0, &hit, &mut found),
            // An object that can't be read may hold it.
            Err(_) => {
                found.insert(Place::Other);
            }
        }
        for place in found {
            let n = match place {
                Place::Attachments => &mut out.attachments,
                Place::Links => &mut out.links,
                Place::Fields => &mut out.fields,
                Place::Other => &mut out.other,
            };
            *n = n.saturating_add(1);
        }
    }
    out
}

/// Note in `found` the place of each string under `o` that holds the text. `place` is that of the
/// dictionary holding `o`: `None` where a walk above counts it, or its strings aren't text.
fn visit(doc: &Document, o: &Object, place: Option<Place>, depth: usize, hit: &dyn Fn(&str) -> bool, found: &mut HashSet<Place>) {
    if depth > MAX_SWEEP_DEPTH {
        found.insert(place.unwrap_or(Place::Other));
        return;
    }
    match o {
        Object::String(s) => {
            if let Some(p) = place
                && hit(&crate::metadata::text_string(s))
            {
                found.insert(p);
            }
        }
        Object::Array(items) => items.iter().for_each(|i| visit(doc, i, place, depth + 1, hit, found)),
        Object::Dict(d) => visit_dict(doc, d, place, depth, hit, found),
        Object::Stream(s) => {
            if s.dict.name(b"Type") == Some(b"EmbeddedFile".as_slice()) && stream_holds(doc, s, hit) {
                found.insert(Place::Attachments);
            }
            visit_dict(doc, &s.dict, place, depth, hit, found);
        }
        _ => {}
    }
}

fn visit_dict(doc: &Document, d: &pdfcraft_cos::Dict, place: Option<Place>, depth: usize, hit: &dyn Fn(&str) -> bool, found: &mut HashSet<Place>) {
    let own = place_of(d, place, depth);
    // A script may be a stream of its own, which says nothing of what it is.
    if own == Some(Place::Links)
        && let Some(js) = d.get(b"JS")
        && let Object::Stream(s) = &*doc.resolve(js)
        && stream_holds(doc, s, hit)
    {
        found.insert(Place::Links);
    }
    for (_, v) in d.iter() {
        visit(doc, v, own, depth + 1, hit, found);
    }
}

/// The place of a dictionary's own strings, from what it is; `parent` is the place of the
/// dictionary holding it directly, and `depth` 0 for an object of its own.
fn place_of(d: &pdfcraft_cos::Dict, parent: Option<Place>, depth: usize) -> Option<Place> {
    let has = |k: &[u8]| d.get(k).is_some();
    let kind = d.name(b"Type");
    // Signatures and the encryption dictionary hold binary strings; XMP is counted on its own.
    if matches!(kind, Some(b"Sig" | b"DocTimeStamp" | b"Metadata")) || has(b"ByteRange") || (has(b"Filter") && has(b"O") && has(b"U")) {
        return None;
    }
    if has(b"Rect") && has(b"Subtype") {
        // Annotations: comments and file attachments are counted on their own.
        return match d.name(b"Subtype") {
            Some(b"Link") => Some(Place::Links),
            Some(b"Widget") => Some(Place::Fields),
            _ => None,
        };
    }
    if has(b"FT") || (has(b"T") && (has(b"Kids") || has(b"Parent"))) {
        return Some(Place::Fields);
    }
    // Outline items: their titles are counted on their own (an action they hold is not).
    if has(b"Title") && [b"Parent".as_slice(), b"Next", b"Prev", b"First", b"Last"].into_iter().any(has) {
        return None;
    }
    // A file specification: one inside what a walk above counts (a file attachment annotation's)
    // is counted there, one with its file (an associated file) is an attachment, and any other is
    // a file an action names. Those in the embedded file list are skipped with it.
    if kind == Some(b"Filespec".as_slice()) {
        return match (parent, depth) {
            (None, 1..) => None,
            _ if has(b"EF") => Some(Place::Attachments),
            _ => Some(Place::Links),
        };
    }
    // An action (a structure element has a parent), a name tree's names.
    let action = has(b"S") && !has(b"P") && kind != Some(b"StructElem".as_slice());
    if action || matches!(d.get(b"Names"), Some(Object::Array(_))) || has(b"Limits") || has(b"URI") || has(b"JS") {
        return Some(Place::Links);
    }
    Some(parent.unwrap_or(Place::Other))
}

/// Whether a stream's data, read as text, holds it. Data that can't be read, or is too large to
/// search, counts as holding it.
fn stream_holds(doc: &Document, s: &pdfcraft_cos::Stream, hit: &dyn Fn(&str) -> bool) -> bool {
    if s.filters().len() > 1 {
        return true;
    }
    match doc.with_direct_parms(s).decoded_first_filter_strict(crate::metadata::MAX_XMP_BYTES) {
        Ok(bytes) => hit(&String::from_utf8_lossy(&bytes)) || hit(&crate::metadata::text_string(&pdfcraft_cos::PdfString { bytes, hex: false })),
        Err(_) => true,
    }
}

/// The nodes of the `/EmbeddedFiles` name tree and the file specifications they list, whose
/// names [`attachments`] counts.
fn embedded_file_tree(doc: &Document) -> HashSet<ObjRef> {
    let mut nodes = HashSet::new();
    let tree = doc
        .root()
        .and_then(|r| doc.get(r).as_dict()?.get(b"Names").cloned())
        .and_then(|names| doc.resolve(&names).as_dict()?.get(b"EmbeddedFiles").cloned());
    let mut next: Vec<(Object, usize)> = tree.into_iter().map(|t| (t, 0)).collect();
    while let Some((o, depth)) = next.pop() {
        let Object::Ref(r) = o else { continue };
        if depth > MAX_NAME_TREE_DEPTH || nodes.len() >= MAX_NAME_TREE_NODES || !nodes.insert(r) {
            continue;
        }
        let node = doc.get(r);
        let Some(node) = node.as_dict() else { continue };
        if let Some(kids) = node.get(b"Kids").and_then(|k| doc.resolve(k).as_array().cloned()) {
            next.extend(kids.into_iter().map(|k| (k, depth.saturating_add(1))));
        }
        // The file specifications it lists, whose names and descriptions it counts.
        if let Some(leaves) = node.get(b"Names").and_then(|l| doc.resolve(l).as_array().cloned()) {
            nodes.extend(leaves.iter().skip(1).step_by(2).filter_map(Object::as_ref));
        }
    }
    nodes
}

/// Outline items whose `/Title` holds the text.
fn bookmarks(doc: &Document, hit: &dyn Fn(Option<&Object>) -> bool) -> usize {
    let Some(root) = doc.root() else { return 0 };
    let Some(first) =
        doc.get(root).as_dict().and_then(|c| c.get(b"Outlines").cloned()).and_then(|o| doc.resolve(&o).as_dict()?.get(b"First").cloned())
    else {
        return 0;
    };
    let (mut n, mut read) = (0usize, 0usize);
    let mut seen = HashSet::new();
    let mut next = vec![first];
    while let Some(o) = next.pop() {
        if let Object::Ref(r) = &o
            && !seen.insert(*r)
        {
            continue;
        }
        let Some(item) = doc.resolve(&o).as_dict().cloned() else { continue };
        if read >= MAX_OUTLINE_ITEMS {
            // Items past the cap aren't read: count one for them.
            return n.saturating_add(1);
        }
        read += 1;
        n += usize::from(hit(item.get(b"Title")));
        next.extend([b"Next".as_slice(), b"First"].into_iter().filter_map(|k| item.get(k).cloned()));
    }
    n
}

/// Embedded files in the `/EmbeddedFiles` name tree whose name, file name or description holds
/// the text, and file attachment annotations whose file's do.
fn attachments(doc: &Document, hit: &dyn Fn(Option<&Object>) -> bool) -> usize {
    let file = |spec: Option<&Object>| {
        spec.and_then(|s| doc.resolve(s).as_dict().cloned()).is_some_and(|d| [b"F".as_slice(), b"UF", b"Desc"].iter().any(|k| hit(d.get(k))))
    };
    let mut n = 0usize;
    let tree = doc
        .root()
        .and_then(|r| doc.get(r).as_dict()?.get(b"Names").cloned())
        .and_then(|names| doc.resolve(&names).as_dict()?.get(b"EmbeddedFiles").cloned());
    if let Some(tree) = tree {
        let mut seen = HashSet::new();
        let mut next = vec![(tree, 0usize)];
        let mut read = 0usize;
        while let Some((o, depth)) = next.pop() {
            if let Object::Ref(r) = &o
                && !seen.insert(*r)
            {
                continue;
            }
            let Some(node) = doc.resolve(&o).as_dict().cloned() else { continue };
            if read >= MAX_NAME_TREE_NODES || depth > MAX_NAME_TREE_DEPTH {
                // A node past the cap isn't read: count one for what it holds.
                n = n.saturating_add(1);
                break;
            }
            read += 1;
            if let Some(leaves) = node.get(b"Names").map(|l| doc.resolve(l)).and_then(|l| l.as_array().cloned()) {
                for pair in leaves.chunks(2) {
                    n += usize::from(hit(pair.first()) || file(pair.get(1)));
                }
            }
            if let Some(kids) = node.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
                next.extend(kids.into_iter().map(|k| (k, depth.saturating_add(1))));
            }
        }
    }
    for page in pdfcraft_model::pages(doc) {
        for a in crate::annots_of(doc, &page.dict) {
            let Some(a) = doc.resolve(&a).as_dict().cloned() else { continue };
            if a.name(b"Subtype") == Some(b"FileAttachment".as_slice()) {
                n += usize::from(file(a.get(b"FS")));
            }
        }
    }
    n
}

/// Annotations on the pages, other than form field widgets and pop-ups (which show their
/// parent's text), whose text or author holds it.
fn comments(doc: &Document, search: &Search, hit: &dyn Fn(Option<&Object>) -> bool) -> usize {
    let mut seen = HashSet::new();
    let mut n = 0usize;
    for page in pdfcraft_model::pages(doc) {
        for a in crate::annots_of(doc, &page.dict) {
            if let Some(r) = a.as_ref()
                && !seen.insert(r)
            {
                continue;
            }
            let Some(a) = doc.resolve(&a).as_dict().cloned() else { continue };
            if matches!(a.name(b"Subtype"), Some(b"Widget" | b"Popup")) {
                continue;
            }
            // Rich text is XHTML: its text is searched, not its markup. A rich text stream that
            // can't be read counts as holding it.
            let rich = a.get(b"RC").is_some_and(|o| rich_text(doc, o).is_none_or(|t| !t.trim().is_empty() && search.text(&t)));
            n += usize::from(rich || [b"Contents".as_slice(), b"T", b"Subj"].iter().any(|k| hit(a.get(k))));
        }
    }
    n
}

/// The text of a rich text string or stream (`/RC`), markup left out; `None` when a stream
/// can't be read (strictly, as UTF-8, within the XMP limit).
fn rich_text(doc: &Document, o: &Object) -> Option<String> {
    let xml = match &*doc.resolve(o) {
        Object::String(s) => crate::metadata::text_string(s),
        Object::Stream(s) if s.filters().len() <= 1 => {
            String::from_utf8(doc.with_direct_parms(s).decoded_first_filter_strict(crate::metadata::MAX_XMP_BYTES).ok()?).ok()?
        }
        Object::Stream(_) => return None,
        _ => return Some(String::new()),
    };
    Some(crate::metadata::xml_values(&xml).join(" "))
}

/// XMP streams on anything reachable from the catalog other than the catalog's and the pages'
/// own, each counted once, that hold the text or can't be read.
fn xmp(doc: &Document, search: &Search) -> usize {
    let scrubbed: HashSet<ObjRef> = crate::metadata::catalog_and_pages(doc).into_iter().collect();
    let Ok(values) = crate::sanitize::owned_values(doc, b"Metadata") else {
        // A graph too deep to walk: some XMP may be there, unread.
        return 1;
    };
    let mut seen = HashSet::new();
    let mut n = 0usize;
    for (holder, own, v) in values {
        if own && scrubbed.contains(&holder) {
            continue;
        }
        if let Some(r) = v.as_ref()
            && !seen.insert(r)
        {
            continue;
        }
        // An entry that isn't a stream holds no XMP.
        if !matches!(&*doc.resolve(&v), Object::Stream(_)) {
            continue;
        }
        n += usize::from(search.xmp(doc, &v));
    }
    n
}
