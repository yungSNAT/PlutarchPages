//! Places [`crate::apply`] leaves as they are that may still hold the removed text (#2448
//! review): bookmark titles, attachments, the comments that remain, XMP on objects other than
//! the catalog and the pages, and (fork) document information, links, actions and scripts, form
//! fields and any other object. Removing them is Remove hidden information's job, or the user's,
//! one item at a time; applying redactions only counts them, so the user can be told. Nothing here
//! changes the document.
//!
//! The text is matched as the metadata check matches it ([`crate::metadata::Search`]), with a
//! work limit of its own. Every walk is bounded: outline items and name-tree nodes are each read
//! once (a seen-set for indirect ones) up to a cap, and the XMP walk is Remove hidden
//! information's own.
//!
//! Before a save (fork, [`survivors`]) the same walks list each place that still holds the text,
//! so the save can refuse and say where. That search is stricter than the count's: a place holds
//! the text when a whole removed run of at least 4 letters or digits is in it, beginning and
//! ending at word edges, or when its whole value is part of a removed run. A single word of a
//! removed sentence is told when applying, never a reason to refuse. Not searched at all: page
//! content (the redaction's own check), appearance streams, form XObjects, images and marked
//! content (`/ActualText`), and attachment data that isn't text.

use std::collections::{HashMap, HashSet};

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
    /// file name (`/F`, `/UF`), description (`/Desc`), portfolio fields or text data holds it,
    /// and file attachment annotations whose file's do.
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
    /// their address, file name or script, a script stream included).
    pub links: usize,
    /// Form fields and their widgets (value, default value, tooltip or other text, a
    /// signature's name, reason, location or contact) and XFA form data that hold it.
    pub fields: usize,
    /// Any other object whose text holds it (structure tree alternate text, page labels, layer
    /// names, article titles, the names in name trees such as named destinations, private data,
    /// attachment data that couldn't be searched…), and objects that couldn't be read.
    pub other: usize,
    /// The removed text couldn't be searched for, so the counts are of every place that holds
    /// any text.
    pub unsearched: bool,
}

impl Elsewhere {
    /// All the places counted.
    pub fn total(&self) -> usize {
        [self.attachments, self.comments, self.xmp, self.info, self.links, self.fields, self.other]
            .into_iter()
            .fold(self.bookmarks, usize::saturating_add)
    }
}

/// What a place is, for [`Survivor`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// An outline item's title.
    Bookmark,
    /// An attachment: its name, file name, description, portfolio fields or text data.
    Attachment,
    /// A comment's text, subject or author.
    Comment,
    /// An XMP stream on an object other than the catalog and the pages.
    Xmp,
    /// An entry of the document information (`/Info`).
    Info,
    /// A link, an action or a script.
    Link,
    /// A form field, its widget, a signature's details or XFA form data.
    Field,
    /// Private data of another application (`/PieceInfo`).
    PrivateData,
    /// Any other object: told when applying, never a reason to refuse a save.
    Other,
    /// An object that couldn't be read or searched in full (past a cap or the search's work
    /// limit), so it may hold the text.
    Unread,
}

/// One place a save found still holding the redacted text ([`survivors`]). It never carries the
/// text: what is there is the user's to find from where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Survivor {
    pub place: Place,
    /// The object holding it, when it is an object of its own (or is held directly by one).
    pub object: Option<ObjRef>,
    /// The page (0-based) of an annotation or widget, or of a page's own entries.
    pub page: Option<usize>,
    /// Its position among the places of its kind: a bookmark's in outline order (0-based).
    pub nth: Option<usize>,
}

/// The places a file about to be saved still holds the text redacted since the last save in
/// ([`survivors`]), at most [`MAX_ITEMS`] of them listed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Survivors {
    pub items: Vec<Survivor>,
    /// Places past the [`MAX_ITEMS`] listed.
    pub more: usize,
    /// The removed text couldn't be searched for: every place listed holds some text.
    pub unsearched: bool,
}

impl Survivors {
    /// Whether the save must refuse: some place still holds the text.
    pub fn refuses(&self) -> bool {
        !self.items.is_empty() || self.more > 0
    }

    /// How many places hold it.
    pub fn len(&self) -> usize {
        self.items.len().saturating_add(self.more)
    }

    /// Nothing holds it.
    pub fn is_empty(&self) -> bool {
        !self.refuses()
    }
}

/// The most places [`Survivors`] lists one by one.
pub const MAX_ITEMS: usize = 200;

/// What the walks find: counts for [`Elsewhere`], or for a save the places one by one, other
/// objects left out (they never refuse) and so never searched.
struct Found {
    counts: Elsewhere,
    survivors: Survivors,
    save: bool,
}

impl Found {
    fn add(&mut self, place: Place, object: Option<ObjRef>, page: Option<usize>, nth: Option<usize>) {
        let c = &mut self.counts;
        let n = match place {
            Place::Bookmark => &mut c.bookmarks,
            Place::Attachment => &mut c.attachments,
            Place::Comment => &mut c.comments,
            Place::Xmp => &mut c.xmp,
            Place::Info => &mut c.info,
            Place::Link => &mut c.links,
            Place::Field => &mut c.fields,
            Place::PrivateData | Place::Other | Place::Unread => &mut c.other,
        };
        *n = n.saturating_add(1);
        if !self.save || place == Place::Other {
            return;
        }
        if self.survivors.items.len() < MAX_ITEMS {
            self.survivors.items.push(Survivor { place, object, page, nth });
        } else {
            self.survivors.more = self.survivors.more.saturating_add(1);
        }
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
    walk(doc, &search, false).counts
}

/// Run every walk with `search`; `save` lists the places for [`survivors`].
fn walk(doc: &Document, search: &Search, save: bool) -> Found {
    let mut found = Found { counts: Elsewhere::default(), survivors: Survivors::default(), save };
    if search.is_nothing() {
        return found;
    }
    // A blank value holds nothing, even when everything counts.
    let hit = |o: Option<&Object>| {
        o.and_then(|o| doc.resolve(o).as_string().map(crate::metadata::text_string)).is_some_and(|t| !t.trim().is_empty() && search.text(&t))
    };
    sweep(doc, search, &mut found);
    bookmarks(doc, &hit, &mut found);
    attachments(doc, search, &hit, &mut found);
    comments(doc, search, &hit, &mut found);
    xmp(doc, search, &mut found);
    info(doc, search, &mut found);
    found.counts.unsearched = search.is_everything();
    found.survivors.unsearched = search.is_everything();
    found
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
    /// What an apply removed: its runs of text, and whether they are all of it (`complete`).
    pub fn new(text: Vec<String>, complete: bool) -> Self {
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

/// The places in `doc`, the document a save is about to write (before the save stamps its date
/// and identifier, which never hold removed text), that still hold the `removed` text, one by
/// one. Only what a full save writes is searched: objects nothing refers to are left out of it.
pub fn survivors(doc: &Document, removed: &Removed) -> Survivors {
    let search = Search::whole(&removed.text, removed.complete);
    walk(doc, &search, true).survivors
}

/// `/Info` entries holding it, strings in arrays and dictionaries included.
fn info(doc: &Document, search: &Search, found: &mut Found) {
    let info_ref = doc.trailer().reference(b"Info");
    let Some(info) = doc.trailer().get(b"Info").and_then(|i| doc.resolve(i).as_dict().cloned()) else { return };
    let hit = |t: &str| !t.trim().is_empty() && search.text(t);
    for (_, v) in info.iter() {
        if any_string(doc, v, 0, &hit) {
            found.add(Place::Info, info_ref, None, None);
        }
    }
}

/// Whether a string in `o` (followed through direct arrays and dictionaries, and references
/// from them, to [`MAX_SWEEP_DEPTH`]) satisfies `hit`.
fn any_string(doc: &Document, o: &Object, depth: usize, hit: &dyn Fn(&str) -> bool) -> bool {
    if depth > MAX_SWEEP_DEPTH {
        return true;
    }
    match &*doc.resolve(o) {
        Object::String(s) => hit(&crate::metadata::text_string(s)),
        Object::Array(items) => items.iter().any(|i| any_string(doc, i, depth + 1, hit)),
        Object::Dict(d) => d.iter().any(|(_, v)| any_string(doc, v, depth + 1, hit)),
        _ => false,
    }
}

/// The most objects the sweep reads; past it, one more place counts as unread.
const MAX_SWEEP_OBJECTS: usize = 4_000_000;
/// How deep the sweep follows direct arrays and dictionaries inside one object.
const MAX_SWEEP_DEPTH: usize = 32;

/// Keys whose strings aren't text anyone reads: a field's or annotation's default appearance and
/// style, an annotation's name (often a UUID), a name tree node's bounds (copies of its names).
const NOT_TEXT: [&[u8]; 4] = [b"DA", b"DS", b"NM", b"Limits"];
/// Keys of a signature dictionary that hold binary data rather than text.
const SIGNATURE_DATA: [&[u8]; 4] = [b"Contents", b"Cert", b"ByteRange", b"Reference"];

/// Every object a full save would write whose strings (or script, XFA or embedded file data)
/// hold the text, by place, except what the walks count (outline items, annotations other than
/// links and widgets, XMP, `/Info`, the embedded file list and file attachment annotations'
/// files) and strings that aren't text (the encryption dictionary, a signature's data). Page
/// content is the redaction's own check.
fn sweep(doc: &Document, search: &Search, out: &mut Found) {
    let mut skip = embedded_file_tree(doc);
    skip.extend([b"Info".as_slice(), b"Encrypt"].into_iter().filter_map(|k| doc.trailer().reference(k)));
    let pages = page_of(doc, &mut skip);
    // A file the next save writes in full leaves out what nothing refers to; when that can't be
    // worked out, every object is searched.
    let reachable = pdfcraft_cos::reachable_objects(doc).ok();
    for (read, (r, o)) in doc.scan_objects_checked().enumerate() {
        if read >= MAX_SWEEP_OBJECTS {
            out.add(Place::Unread, None, None, None);
            break;
        }
        if skip.contains(&r) || reachable.as_ref().is_some_and(|k| !k.contains(&r.num)) {
            continue;
        }
        let mut found = HashSet::new();
        match o {
            Ok(o) => visit(doc, &o, None, 0, search, &mut found),
            // An object that can't be read may hold it.
            Err(_) => {
                found.insert(Place::Unread);
            }
        }
        for place in found {
            out.add(place, Some(r), pages.get(&r).copied(), None);
        }
    }
}

/// The page of each page object and annotation (0-based). File attachment annotations' files are
/// added to `skip`: [`attachments`] counts them.
fn page_of(doc: &Document, skip: &mut HashSet<ObjRef>) -> HashMap<ObjRef, usize> {
    let mut pages = HashMap::new();
    for (i, page) in pdfcraft_model::pages(doc).into_iter().enumerate() {
        pages.insert(page.obj, i);
        for a in crate::annots_of(doc, &page.dict) {
            if let Some(r) = a.as_ref() {
                pages.entry(r).or_insert(i);
            }
            let a = doc.resolve(&a);
            if let Some(d) = a.as_dict()
                && d.name(b"Subtype") == Some(b"FileAttachment".as_slice())
                && let Some(fs) = d.get(b"FS").and_then(Object::as_ref)
            {
                skip.insert(fs);
            }
        }
    }
    pages
}

/// Note in `found` the place of each string under `o` that holds the text. `place` is that of the
/// dictionary holding `o`: `None` where a walk counts it, or its strings aren't text.
fn visit(doc: &Document, o: &Object, place: Option<Place>, depth: usize, search: &Search, found: &mut HashSet<Place>) {
    if depth > MAX_SWEEP_DEPTH {
        found.insert(Place::Unread);
        return;
    }
    match o {
        Object::String(s) => {
            // Other places never refuse a save, so a save's search spends nothing on them.
            if let Some(p) = place
                && !(search.is_whole() && p == Place::Other)
            {
                let t = crate::metadata::text_string(s);
                if !t.trim().is_empty() && search.text(&t) {
                    found.insert(if search.exhausted() { Place::Unread } else { p });
                }
            }
        }
        Object::Array(items) => items.iter().for_each(|i| visit(doc, i, place, depth + 1, search, found)),
        Object::Dict(d) => visit_dict(doc, d, place, depth, search, found),
        Object::Stream(s) => visit_dict(doc, &s.dict, place, depth, search, found),
        _ => {}
    }
}

fn visit_dict(doc: &Document, d: &pdfcraft_cos::Dict, place: Option<Place>, depth: usize, search: &Search, found: &mut HashSet<Place>) {
    let own = place_of(d, place, depth);
    let mut note = |p: Place, holds: Option<bool>| match holds {
        Some(true) => {
            found.insert(if search.exhausted() { Place::Unread } else { p });
        }
        Some(false) => {}
        // Data that can't be searched: a script or form data may hold it; an attachment's is
        // told (Remove hidden information removes it), since most attached files aren't text.
        None if p == Place::Attachment => {
            found.insert(Place::Other);
        }
        None => {
            found.insert(p);
        }
    };
    // A script may be a stream of its own, which says nothing of what it is.
    if own == Some(Place::Link)
        && let Some(Object::Stream(s)) = d.get(b"JS").map(|js| doc.resolve(js)).as_deref()
    {
        note(Place::Link, data_holds(doc, s, search, false));
    }
    // An embedded file's data, by its use under a file specification's `/EF` (its `/Type` is
    // optional).
    if own == Some(Place::Attachment)
        && let Some(ef) = d.get(b"EF").and_then(|ef| doc.resolve(ef).as_dict().cloned())
    {
        for (_, f) in ef.iter() {
            if let Object::Stream(s) = &*doc.resolve(f) {
                note(Place::Attachment, data_holds(doc, s, search, false));
            }
        }
    }
    // XFA form data and template: XML, searched by its text.
    if let Some(xfa) = d.get(b"XFA").map(|x| doc.resolve(x)) {
        let parts: Vec<Object> = match &*xfa {
            Object::Array(a) => a.iter().skip(1).step_by(2).cloned().collect(),
            other => vec![other.clone()],
        };
        for part in parts {
            if let Object::Stream(s) = &*doc.resolve(&part) {
                note(Place::Field, data_holds(doc, s, search, true));
            }
        }
    }
    let signature = is_signature(d);
    for (k, v) in d.iter() {
        let k = k.as_slice();
        if NOT_TEXT.contains(&k) || (signature && SIGNATURE_DATA.contains(&k)) {
            continue;
        }
        let child = if k == b"PieceInfo" && own.is_some() { Some(Place::PrivateData) } else { own };
        visit(doc, v, child, depth + 1, search, found);
    }
}

fn is_signature(d: &pdfcraft_cos::Dict) -> bool {
    matches!(d.name(b"Type"), Some(b"Sig" | b"DocTimeStamp")) || d.get(b"ByteRange").is_some()
}

/// The place of a dictionary's own strings, from what it is; `parent` is the place of the
/// dictionary holding it directly, and `depth` 0 for an object of its own.
fn place_of(d: &pdfcraft_cos::Dict, parent: Option<Place>, depth: usize) -> Option<Place> {
    let has = |k: &[u8]| d.get(k).is_some();
    let kind = d.name(b"Type");
    // The encryption dictionary holds binary strings; XMP is counted on its own.
    if kind == Some(b"Metadata".as_slice()) || (has(b"Filter") && has(b"O") && has(b"U")) {
        return None;
    }
    // A signature's name, reason, location and contact (its data is skipped): the signature
    // field's, removed with it.
    if is_signature(d) {
        return Some(Place::Field);
    }
    if has(b"Rect") && has(b"Subtype") {
        // Annotations: comments and file attachments are counted on their own.
        return match d.name(b"Subtype") {
            Some(b"Link") => Some(Place::Link),
            Some(b"Widget") => Some(Place::Field),
            _ => None,
        };
    }
    if has(b"FT") || (has(b"T") && (has(b"Kids") || has(b"Parent"))) {
        return Some(Place::Field);
    }
    // Outline items: their titles are counted on their own (an action they hold is not).
    if has(b"Title") && [b"Parent".as_slice(), b"Next", b"Prev", b"First", b"Last"].into_iter().any(has) {
        return None;
    }
    // A file specification: one inside what a walk counts (a file attachment annotation's) is
    // counted there, one with its file (an associated file) is an attachment, and any other is
    // a file an action names. Those in the embedded file list are skipped with it.
    if kind == Some(b"Filespec".as_slice()) {
        return match (parent, depth) {
            (None, 1..) => None,
            _ if has(b"EF") => Some(Place::Attachment),
            _ => Some(Place::Link),
        };
    }
    // An action (a structure element has a parent); a URI action's address. A catalog's base
    // URI dictionary (`/URI << /Base … >>`) is not one.
    let action = has(b"S") && !has(b"P") && kind != Some(b"StructElem".as_slice());
    if action || matches!(d.get(b"URI"), Some(Object::String(_))) || has(b"JS") {
        return Some(Place::Link);
    }
    // A name tree's names (named destinations, scripts' names, templates…) are told: nothing
    // removes one name, and what a script name leads to is searched as an action.
    if matches!(d.get(b"Names"), Some(Object::Array(_))) {
        return Some(Place::Other);
    }
    Some(parent.unwrap_or(Place::Other))
}

/// Whether a stream's data, read as text, holds it; `None` when it can't be searched: data that
/// isn't text, can't be decoded or is too large. `xml` searches XML's text values, not its markup.
fn data_holds(doc: &Document, s: &pdfcraft_cos::Stream, search: &Search, xml: bool) -> Option<bool> {
    if s.filters().len() > 1 {
        return None;
    }
    let bytes = doc.with_direct_parms(s).decoded_first_filter_strict(crate::metadata::MAX_XMP_BYTES).ok()?;
    let text = as_text(&bytes)?;
    let text = if xml { crate::metadata::xml_values(&text).join(" ") } else { text };
    Some(!text.trim().is_empty() && search.data(&text))
}

/// Data as text, or `None` when it isn't: UTF-16 or UTF-8 behind a byte order mark, or else
/// UTF-8 or PDFDocEncoding with no control bytes but tab, line feed, form feed and carriage
/// return. Binary data (an image, a zip) has control bytes almost at once, and reading it as
/// text would find any short term somewhere in it.
pub(crate) fn as_text(bytes: &[u8]) -> Option<String> {
    let utf16 = |rest: &[u8], le: bool| {
        let units = rest.as_chunks::<2>().0.iter().map(|&p| if le { u16::from_le_bytes(p) } else { u16::from_be_bytes(p) });
        char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
    };
    match bytes {
        [0xFE, 0xFF, rest @ ..] => return Some(utf16(rest, false)),
        [0xFF, 0xFE, rest @ ..] => return Some(utf16(rest, true)),
        [0xEF, 0xBB, 0xBF, rest @ ..] => return Some(String::from_utf8_lossy(rest).into_owned()),
        _ => {}
    }
    if bytes.iter().any(|&b| (b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\x0C' | b'\r')) || b == 0x7F) {
        return None;
    }
    match std::str::from_utf8(bytes) {
        Ok(t) => Some(t.to_owned()),
        Err(_) => Some(pdfcraft_cos::PdfString { bytes: bytes.to_vec(), hex: false }.to_text()),
    }
}

/// The nodes of the `/EmbeddedFiles` name tree and the file specifications they list, which
/// [`attachments`] searches.
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
fn bookmarks(doc: &Document, hit: &dyn Fn(Option<&Object>) -> bool, found: &mut Found) {
    let Some(root) = doc.root() else { return };
    let Some(first) =
        doc.get(root).as_dict().and_then(|c| c.get(b"Outlines").cloned()).and_then(|o| doc.resolve(&o).as_dict()?.get(b"First").cloned())
    else {
        return;
    };
    let mut read = 0usize;
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
            found.add(Place::Unread, None, None, None);
            return;
        }
        if hit(item.get(b"Title")) {
            found.add(Place::Bookmark, o.as_ref(), None, Some(read));
        }
        read += 1;
        // `First` is taken before `Next`: outline order.
        next.extend([b"Next".as_slice(), b"First"].into_iter().filter_map(|k| item.get(k).cloned()));
    }
}

/// Embedded files in the `/EmbeddedFiles` name tree whose name, file name, description,
/// portfolio fields or text data holds the text, and file attachment annotations whose file's do.
fn attachments(doc: &Document, search: &Search, hit: &dyn Fn(Option<&Object>) -> bool, found: &mut Found) {
    let text_hit = |t: &str| !t.trim().is_empty() && search.text(t);
    // Some(true): it holds it; None: its data couldn't be searched.
    let file = |spec: Option<&Object>| -> Option<bool> {
        let Some(d) = spec.and_then(|s| doc.resolve(s).as_dict().cloned()) else { return Some(false) };
        if [b"F".as_slice(), b"UF", b"Desc"].iter().any(|k| hit(d.get(k))) || d.get(b"CI").is_some_and(|ci| any_string(doc, ci, 0, &text_hit)) {
            return Some(true);
        }
        let mut unread = false;
        for (_, f) in d.get(b"EF").and_then(|ef| doc.resolve(ef).as_dict().cloned()).unwrap_or_default().iter() {
            if let Object::Stream(s) = &*doc.resolve(f) {
                match data_holds(doc, s, search, false) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => unread = true,
                }
            }
        }
        if unread { None } else { Some(false) }
    };
    let add = |found: &mut Found, holds: Option<bool>, object: Option<ObjRef>, page: Option<usize>, nth: Option<usize>| match holds {
        Some(true) => found.add(if search.exhausted() { Place::Unread } else { Place::Attachment }, object, page, nth),
        None => found.add(Place::Other, object, page, nth),
        Some(false) => {}
    };
    let tree = doc
        .root()
        .and_then(|r| doc.get(r).as_dict()?.get(b"Names").cloned())
        .and_then(|names| doc.resolve(&names).as_dict()?.get(b"EmbeddedFiles").cloned());
    if let Some(tree) = tree {
        let mut seen = HashSet::new();
        let mut next = vec![(tree, 0usize)];
        let (mut read, mut nth) = (0usize, 0usize);
        while let Some((o, depth)) = next.pop() {
            if let Object::Ref(r) = &o
                && !seen.insert(*r)
            {
                continue;
            }
            let Some(node) = doc.resolve(&o).as_dict().cloned() else { continue };
            if read >= MAX_NAME_TREE_NODES || depth > MAX_NAME_TREE_DEPTH {
                // A node past the cap isn't read: count one for what it holds.
                found.add(Place::Unread, o.as_ref(), None, None);
                break;
            }
            read += 1;
            if let Some(leaves) = node.get(b"Names").map(|l| doc.resolve(l)).and_then(|l| l.as_array().cloned()) {
                for pair in leaves.chunks(2) {
                    let holds = if hit(pair.first()) { Some(true) } else { file(pair.get(1)) };
                    add(found, holds, pair.get(1).and_then(Object::as_ref), None, Some(nth));
                    nth += 1;
                }
            }
            if let Some(kids) = node.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
                next.extend(kids.into_iter().map(|k| (k, depth.saturating_add(1))));
            }
        }
    }
    for (i, page) in pdfcraft_model::pages(doc).into_iter().enumerate() {
        for a in crate::annots_of(doc, &page.dict) {
            let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
            if d.name(b"Subtype") == Some(b"FileAttachment".as_slice()) {
                add(found, file(d.get(b"FS")), a.as_ref(), Some(i), None);
            }
        }
    }
}

/// Annotations on the pages, other than form field widgets and pop-ups (which show their
/// parent's text), whose text or author holds it.
fn comments(doc: &Document, search: &Search, hit: &dyn Fn(Option<&Object>) -> bool, found: &mut Found) {
    let mut seen = HashSet::new();
    for (i, page) in pdfcraft_model::pages(doc).into_iter().enumerate() {
        for a in crate::annots_of(doc, &page.dict) {
            if let Some(r) = a.as_ref()
                && !seen.insert(r)
            {
                continue;
            }
            let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
            if matches!(d.name(b"Subtype"), Some(b"Widget" | b"Popup")) {
                continue;
            }
            // Rich text is XHTML: its text is searched, not its markup. A rich text stream that
            // can't be read counts as holding it.
            let rich = d.get(b"RC").is_some_and(|o| rich_text(doc, o).is_none_or(|t| !t.trim().is_empty() && search.text(&t)));
            if rich || [b"Contents".as_slice(), b"T", b"Subj"].iter().any(|k| hit(d.get(k))) {
                found.add(if search.exhausted() { Place::Unread } else { Place::Comment }, a.as_ref(), Some(i), None);
            }
        }
    }
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
fn xmp(doc: &Document, search: &Search, found: &mut Found) {
    let scrubbed: HashSet<ObjRef> = crate::metadata::catalog_and_pages(doc).into_iter().collect();
    let Ok(values) = crate::sanitize::owned_values(doc, b"Metadata") else {
        // A graph too deep to walk: some XMP may be there, unread.
        found.add(Place::Unread, None, None, None);
        return;
    };
    let mut seen = HashSet::new();
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
        if search.xmp(doc, &v) {
            found.add(Place::Xmp, v.as_ref().or(Some(holder)), None, None);
        }
    }
}
