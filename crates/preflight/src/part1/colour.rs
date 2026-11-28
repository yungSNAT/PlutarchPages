//! 6.2.3.3: which device colour no Default colour space covers, and 6.2.4 inline image rules.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_model::Page;

/// Which device spaces a resource dictionary gives a Default colour space.
#[derive(Clone, Copy)]
struct Defaults {
    gray: bool,
    rgb: bool,
    cmyk: bool,
}

impl Defaults {
    const NONE: Defaults = Defaults { gray: false, rgb: false, cmyk: false };
    const ALL: Defaults = Defaults { gray: true, rgb: true, cmyk: true };

    /// The Default spaces of the resource dictionary `resources` itself (PDF 1.4 §4.5.7). Each key
    /// counts only when it is a compatible CIE-based space: `DefaultGray` accepts `CalGray` or
    /// `ICCBased` (`/N 1`), `DefaultRGB` accepts `CalRGB` or `ICCBased` (`/N 3`), and
    /// `DefaultCMYK` accepts `ICCBased` (`/N 4`).
    fn in_dict(doc: &Document, resources: &Dict) -> Defaults {
        let spaces = resources.get(b"ColorSpace").map(|c| doc.resolve(c));
        let Some(spaces) = spaces.as_deref().and_then(Object::as_dict) else { return Defaults::NONE };
        let valid = |key: &[u8], cal: Option<&[u8]>, n: i64| {
            let Some(s) = spaces.get(key).map(|s| doc.resolve(s)) else { return false };
            let Some(a) = s.as_array() else { return false };
            let Some(kind) = a.first().map(|k| doc.resolve(k)) else { return false };
            match kind.as_name() {
                Some(k) if cal == Some(k) => a.get(1).is_some_and(|p| matches!(&*doc.resolve(p), Object::Dict(_))),
                Some(b"ICCBased") => a.get(1).map(|p| doc.resolve(p)).is_some_and(|p| {
                    matches!(&*p, Object::Stream(s) if s.dict.get(b"N").map(|v| doc.resolve(v)).is_some_and(|v| matches!(*v, Object::Int(v) if v == n)))
                }),
                _ => false,
            }
        };
        Defaults {
            gray: valid(b"DefaultGray", Some(b"CalGray"), 1),
            rgb: valid(b"DefaultRGB", Some(b"CalRGB"), 3),
            cmyk: valid(b"DefaultCMYK", None, 4),
        }
    }

    /// The spaces both cover.
    fn and(self, other: Defaults) -> Defaults {
        Defaults { gray: self.gray && other.gray, rgb: self.rgb && other.rgb, cmyk: self.cmyk && other.cmyk }
    }

    /// The device colour of `used` that these don't cover, added to `out`.
    fn uncover(self, used: crate::DeviceColour, out: &mut crate::DeviceColour) {
        out.gray |= used.gray && !self.gray;
        out.rgb |= used.rgb && !self.rgb;
        out.cmyk |= used.cmyk && !self.cmyk;
    }
}

/// What resource dictionaries give the content drawn under them, walking shared `/Resources`,
/// `/XObject` and `/Shading` maps once.
#[derive(Default)]
struct Named {
    /// Objects named in a holder's `/XObject` or `/Shading`, with the Defaults common to every
    /// holder that names each one.
    direct: HashMap<ObjRef, Defaults>,
    /// Objects named by a page's `/XObject` or `/Shading`.
    on_pages: HashSet<ObjRef>,
    /// Indirect `/XObject` or `/Shading` maps not yet expanded, with their combined Defaults and
    /// whether any page references each map.
    maps: HashMap<ObjRef, (Defaults, bool)>,
    /// Indirect `/Resources` dictionaries already recorded, with their `(defaults, falls_back)`.
    /// Pages are recorded before other objects, so any map a page uses is marked `on_page` on its
    /// first visit.
    seen_res: HashMap<ObjRef, (Defaults, bool)>,
    /// Indirect `/Resources` dictionaries used by a page.
    page_resources: HashSet<ObjRef>,
}

impl Named {
    /// Records `resources` (of a page when `on_page` is set) and returns its `(defaults, falls_back)`.
    fn record(&mut self, doc: &Document, resources: Option<&Object>, on_page: bool) -> (Defaults, bool) {
        let res_ref = resources.and_then(Object::as_ref).map(|r| crate::font_object(doc, r));
        if let Some(rr) = res_ref {
            if on_page {
                self.page_resources.insert(rr);
            }
            if let Some(&(defaults, falls_back)) = self.seen_res.get(&rr) {
                return (defaults, falls_back && !on_page);
            }
        }
        let resolved = resources.map(|r| doc.resolve(r));
        let Some(r) = resolved.as_deref().and_then(Object::as_dict) else {
            return (Defaults::NONE, !on_page);
        };
        let defaults = Defaults::in_dict(doc, r);
        let mut falls_back = false;
        for key in [b"XObject".as_slice(), b"Shading"] {
            match r.get(key) {
                Some(Object::Ref(m)) if doc.get(crate::font_object(doc, *m)).as_dict().is_some() => {
                    let m = crate::font_object(doc, *m);
                    self.maps.entry(m).and_modify(|(d, p)| (*d, *p) = (d.and(defaults), *p || on_page)).or_insert((defaults, on_page));
                }
                Some(Object::Dict(named)) => self.add_dict(doc, named, defaults, on_page),
                _ => falls_back = true,
            }
        }
        if let Some(rr) = res_ref {
            self.seen_res.insert(rr, (defaults, falls_back));
        }
        (defaults, falls_back && !on_page)
    }

    fn add_dict(&mut self, doc: &Document, named: &Dict, defaults: Defaults, on_page: bool) {
        for n in named.iter().filter_map(|(_, v)| v.as_ref()).map(|n| crate::font_object(doc, n)) {
            self.direct.entry(n).and_modify(|d| *d = d.and(defaults)).or_insert(defaults);
            if on_page {
                self.on_pages.insert(n);
            }
        }
    }

    /// Expands each shared `/XObject` or `/Shading` map once and returns
    /// `(named, on_pages, page_resources)`.
    fn finish(mut self, doc: &Document) -> (HashMap<ObjRef, Defaults>, HashSet<ObjRef>, HashSet<ObjRef>) {
        let maps = std::mem::take(&mut self.maps);
        for (m, (defaults, on_page)) in maps {
            let o = doc.get(m);
            let Some(named) = o.as_dict() else { continue };
            self.add_dict(doc, named, defaults, on_page);
        }
        (self.direct, self.on_pages, self.page_resources)
    }
}

/// Judges `/ColorSpace`, direct `/Shading` and `/Pattern` entries of every resource dictionary (and
/// of direct Type 3 fonts inside `/Font`), walking shared indirect maps once.
#[derive(Default)]
struct ResourceSpaces {
    read_res: HashSet<ObjRef>,
    read_fonts: HashSet<ObjRef>,
    shared: HashMap<(&'static str, ObjRef), Defaults>,
}

impl ResourceSpaces {
    fn visit(
        &mut self,
        doc: &Document,
        res: &Object,
        on_page: bool,
        page_resources: &HashSet<ObjRef>,
        fallback: Defaults,
        out: &mut crate::DeviceColour,
    ) {
        let on_page = match res.as_ref().map(|r| crate::font_object(doc, r)) {
            Some(rr) => {
                if !self.read_res.insert(rr) {
                    return;
                }
                on_page || page_resources.contains(&rr)
            }
            None => on_page,
        };
        let resolved = doc.resolve(res);
        let Some(resources) = resolved.as_dict() else { return };
        let own = Defaults::in_dict(doc, resources);
        let shading = if on_page { own.and(fallback) } else { own };
        for (key, role, defaults) in [(b"ColorSpace".as_slice(), "ColorSpace", own), (b"Shading", "Shading", shading), (b"Pattern", "Pattern", own)] {
            match resources.get(key) {
                Some(Object::Ref(r)) => {
                    let r = crate::font_object(doc, *r);
                    self.shared.entry((role, r)).and_modify(|d| *d = d.and(defaults)).or_insert(defaults);
                }
                Some(Object::Dict(map)) => scan_map(doc, role, map, defaults, out),
                _ => {}
            }
        }
        // A Type 3 font written straight into `/Font` has resources of its own.
        if let Some(fonts) = resources.get(b"Font").filter(|f| f.as_ref().is_none_or(|r| self.read_fonts.insert(crate::font_object(doc, r))))
            && let Some(fonts) = doc.resolve(fonts).as_dict()
        {
            for (_, f) in fonts.iter() {
                if let Object::Dict(f) = f
                    && f.name(b"Subtype") == Some(b"Type3")
                    && let Some(own_res) = f.get(b"Resources")
                {
                    self.visit(doc, own_res, false, page_resources, fallback, out);
                }
            }
        }
    }

    fn finish(self, doc: &Document, out: &mut crate::DeviceColour) {
        for ((role, r), defaults) in self.shared {
            let o = doc.get(r);
            let Some(map) = o.as_dict() else { continue };
            scan_map(doc, role, map, defaults, out);
        }
    }
}

/// Checks one `/ColorSpace`, `/Shading` or `/Pattern` dictionary under `defaults`.
fn scan_map(doc: &Document, role: &str, map: &Dict, defaults: Defaults, out: &mut crate::DeviceColour) {
    let mut used = crate::DeviceColour::default();
    match role {
        "ColorSpace" => {
            for (k, entry) in map.iter() {
                if !matches!(k.as_slice(), b"DefaultGray" | b"DefaultRGB" | b"DefaultCMYK") {
                    crate::mark_cs_object(doc, entry, &mut used, 0);
                }
            }
        }
        "Shading" => {
            for (_, s) in map.iter() {
                direct_shading_colour(doc, s, &mut used);
            }
        }
        _ => {
            for (_, p) in map.iter() {
                if let Some(s) = crate::dict_of(&doc.resolve(p)).and_then(|p| p.get(b"Shading")) {
                    direct_shading_colour(doc, s, &mut used);
                }
            }
        }
    }
    defaults.uncover(used, out);
}

fn direct_shading_colour(doc: &Document, s: &Object, used: &mut crate::DeviceColour) {
    if let Object::Dict(s) = s
        && let Some(cs) = s.get(b"ColorSpace")
    {
        crate::mark_cs_object(doc, cs, used, 0);
    }
}

/// A page's content streams, following reference aliases on `/Contents` and its array entries
/// (ISO 32000-1 7.3.10).
fn page_streams(doc: &Document, page: &Dict) -> Vec<ObjRef> {
    crate::page_contents(doc, page)
        .into_iter()
        .flat_map(|r| {
            let r = crate::font_object(doc, r);
            match &*doc.get(r) {
                Object::Array(a) => a.iter().filter_map(Object::as_ref).map(|r| crate::font_object(doc, r)).collect(),
                _ => vec![r],
            }
        })
        .collect()
}

/// Streams named by `d`'s `/AP` appearance dictionary (single stream or state subdictionary),
/// following reference aliases, even when a stream omits `/BBox` and `/Subtype`.
fn appearance_streams(doc: &Document, d: &Dict, seen_ap: &mut HashSet<ObjRef>, out: &mut HashSet<ObjRef>) {
    let Some(ap) = d.get(b"AP").filter(|a| a.as_ref().is_none_or(|r| seen_ap.insert(crate::font_object(doc, r)))) else {
        return;
    };
    let ap = doc.resolve(ap);
    let Some(ap) = ap.as_dict() else { return };
    for (_, entry) in ap.iter() {
        match &*doc.resolve(entry) {
            Object::Stream(_) => out.extend(entry.as_ref().map(|r| crate::font_object(doc, r))),
            Object::Dict(states) => {
                out.extend(states.iter().filter_map(|(_, v)| v.as_ref()).map(|r| crate::font_object(doc, r)));
            }
            _ => {}
        }
    }
}

/// Decodes each content stream in `under`, collecting uncovered device colour, inline-image 6.2.4
/// violations and the Defaults common to every content stream that draws and may fall back to a
/// page's resources.
fn scan_content(objs: &[(ObjRef, std::sync::Arc<Object>)], under: &HashMap<ObjRef, (Defaults, bool)>) -> (crate::DeviceColour, Inline, Defaults) {
    let mut out = crate::DeviceColour::default();
    let mut fallback = Defaults::ALL;
    let mut budget = pdfcraft_cos::DecodeBudget::new(crate::MAX_COLOUR_DECODED, usize::MAX);
    let mut inline = Inline::default();
    for (r, o) in objs {
        if let Some(&(own, falls_back)) = under.get(r)
            && let Object::Stream(s) = &**o
        {
            let mut used = crate::DeviceColour::default();
            let draws = match budget.decode(s) {
                Ok(data) => {
                    let _ = crate::content_colour(&data, &mut used);
                    inline_images(&data, &mut inline) && falls_back
                }
                Err(_) => {
                    out.incomplete = true;
                    falls_back
                }
            };
            own.uncover(used, &mut out);
            if draws {
                fallback = fallback.and(own);
            }
        }
    }
    (out, inline, fallback)
}

/// Registers the `/CharProcs` glyph streams of any direct Type 3 font inside `d`'s `/Resources`
/// `/Font` map, walking shared indirect `/Resources` and `/Font` dictionaries once.
fn register_inline_type3(
    doc: &Document,
    d: &Dict,
    seen: &mut (HashSet<ObjRef>, HashSet<ObjRef>),
    named: &mut Named,
    add: &mut impl FnMut(&[ObjRef], (Defaults, bool)),
) {
    if let Some(res) = d.get(b"Resources").filter(|r| r.as_ref().is_none_or(|r| seen.0.insert(crate::font_object(doc, r))))
        && let Some(fonts) = doc
            .resolve(res)
            .as_dict()
            .and_then(|r| r.get(b"Font").cloned())
            .filter(|f| f.as_ref().is_none_or(|r| seen.1.insert(crate::font_object(doc, r))))
        && let Some(fonts) = doc.resolve(&fonts).as_dict()
    {
        for (_, f) in fonts.iter() {
            if let Object::Dict(f) = f {
                register_type3_glyphs(doc, f, named, add);
            }
        }
    }
}

/// Registers the `/CharProcs` glyph streams of a Type 3 font dictionary under that font's own
/// `/Resources` (both indirect Type 3 font objects and direct Type 3 fonts inside `/Font` maps).
fn register_type3_glyphs(doc: &Document, font: &Dict, named: &mut Named, add: &mut impl FnMut(&[ObjRef], (Defaults, bool))) {
    if font.name(b"Subtype") == Some(b"Type3")
        && let Some(procs) = font.get(b"CharProcs").map(|c| doc.resolve(c))
        && let Some(procs) = procs.as_dict()
    {
        let glyphs: Vec<ObjRef> = procs.iter().filter_map(|(_, p)| p.as_ref()).map(|p| crate::font_object(doc, p)).collect();
        add(&glyphs, named.record(doc, font.get(b"Resources"), false));
    }
}

/// Collects every content stream's covering Default colour spaces, named resource holders and
/// page object numbers.
fn collect_content_defaults(
    doc: &Document,
    objs: &[(ObjRef, std::sync::Arc<Object>)],
    pages: &[Page],
) -> (HashMap<ObjRef, (Defaults, bool)>, Named, HashSet<ObjRef>) {
    let mut under: HashMap<ObjRef, (Defaults, bool)> = HashMap::new();
    let mut named = Named::default();
    let mut appearances = HashSet::new();
    let mut seen_ap = HashSet::new();
    let mut add = |content: &[ObjRef], (defaults, falls_back): (Defaults, bool)| {
        for r in content {
            under.entry(*r).and_modify(|(d, f)| (*d, *f) = (d.and(defaults), *f || falls_back)).or_insert((defaults, falls_back));
        }
    };
    let mut page_objs = HashSet::new();
    for p in pages {
        page_objs.insert(p.obj);
        let holder = named.record(doc, p.dict.get(b"Resources"), true);
        add(&page_streams(doc, &p.dict), holder);
        if let Some(annots) = p.dict.get(b"Annots").map(|a| doc.resolve(a))
            && let Some(annots) = annots.as_array()
        {
            for d in annots.iter().filter_map(Object::as_dict) {
                appearance_streams(doc, d, &mut seen_ap, &mut appearances);
            }
        }
    }
    for (_, o) in objs {
        if let Some(d) = crate::dict_of(o) {
            appearance_streams(doc, d, &mut seen_ap, &mut appearances);
        }
    }
    let mut seen_type3 = (HashSet::new(), HashSet::new());
    for (r, o) in objs {
        let Some(d) = crate::dict_of(o) else { continue };
        if matches!(&**o, Object::Stream(s) if crate::is_content(&s.dict) || appearances.contains(r)) {
            add(&[*r], named.record(doc, d.get(b"Resources"), false));
        }
        register_type3_glyphs(doc, d, &mut named, &mut add);
        register_inline_type3(doc, d, &mut seen_type3, &mut named, &mut add);
    }
    // Any other referenced content has no Default space known to apply.
    for r in crate::referenced_content(doc, objs) {
        under.entry(crate::font_object(doc, r)).or_insert((Defaults::NONE, true));
    }
    (under, named, page_objs)
}

/// 6.2.3.3: the device colour that no Default colour space covers (ISO 32000-1 8.6.5.6), and
/// whether some content could not be read; together with 6.2.4 inline image findings. A content
/// stream draws under its own resources: a page's (inherited ones included), or a form's,
/// pattern's, appearance stream's or Type 3 font's own, never those of the page that draws it. A
/// colour space in a resource dictionary is used under that dictionary. An image or a shading
/// (direct in a page's `/Shading` or its own object) is drawn under every resource dictionary that
/// names it, and, when a page names it, under any other content that may fall back to the page's
/// resources and draws something; one that nothing names (a soft mask, say) is judged under every
/// resource dictionary content draws under.
pub(super) fn uncovered_device_colour(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)], pages: &[Page]) -> (crate::DeviceColour, Inline) {
    let (under, named, page_objs) = collect_content_defaults(doc, objs, pages);
    let (named, on_pages, page_resources) = named.finish(doc);
    let everywhere = under.values().map(|(d, _)| *d).reduce(Defaults::and).unwrap_or(Defaults::NONE);
    let (mut out, inline, fallback) = scan_content(objs, &under);
    let mut res_spaces = ResourceSpaces::default();
    for (r, o) in objs {
        let Some(d) = crate::dict_of(o) else { continue };
        if let Some(res) = d.get(b"Resources") {
            let on_page = page_objs.contains(r) || d.name(b"Type") == Some(b"Pages");
            res_spaces.visit(doc, res, on_page, &page_resources, fallback, &mut out);
        }
        if let Some(cs) = d.get(b"ColorSpace")
            && !matches!(&*doc.resolve(cs), Object::Dict(_))
        {
            let mut used = crate::DeviceColour::default();
            crate::mark_cs_object(doc, cs, &mut used, 0);
            let drawn = match named.get(r) {
                Some(&d) if on_pages.contains(r) => d.and(fallback),
                Some(&d) => d,
                None => everywhere,
            };
            drawn.uncover(used, &mut out);
        }
    }
    res_spaces.finish(doc, &mut out);
    (out, inline)
}

/// What the inline images of content break of 6.2.4.
#[derive(Default)]
pub(super) struct Inline {
    /// One asks to be interpolated.
    pub(super) interpolate: bool,
    /// One has a BitsPerComponent other than 1, 2, 4 or 8, or a mask one other than 1.
    pub(super) bits: bool,
}

/// Notes in `inline` what the inline images of the content `data` break, and returns whether the
/// content draws an XObject or a shading (`Do` or `sh`).
fn inline_images(data: &[u8], inline: &mut Inline) -> bool {
    let mut draws = false;
    for op in pdfcraft_content::ops(data) {
        match op.op.as_slice() {
            b"Do" | b"sh" => draws = true,
            b"BI" => {
                let Some((image, _)) = &op.inline else { continue };
                let get = |short: &[u8], long: &[u8]| image.get(short).or_else(|| image.get(long));
                inline.interpolate |= get(b"I", b"Interpolate").is_some_and(|v| !matches!(v, Object::Bool(false) | Object::Null));
                let mask = matches!(get(b"IM", b"ImageMask"), Some(Object::Bool(true)));
                inline.bits |= get(b"BPC", b"BitsPerComponent")
                    .filter(|v| !matches!(v, Object::Null))
                    .is_some_and(|v| !matches!(*v, Object::Int(b) if if mask { b == 1 } else { matches!(b, 1 | 2 | 4 | 8) }));
            }
            _ => {}
        }
    }
    draws
}
