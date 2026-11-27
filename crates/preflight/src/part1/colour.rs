//! 6.2.3.3: which device colour no Default colour space covers.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

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

    /// The Default spaces of the resource dictionary `resources` itself. An entry counts only when
    /// it is a device-independent colour space (CalGray, CalRGB, Lab or ICCBased): a null, a
    /// dangling reference, a number, a device space, or an Indexed, Separation, DeviceN or Pattern
    /// space (which draw in a device space of their own) changes nothing.
    fn in_dict(doc: &Document, resources: &Dict) -> Defaults {
        let spaces = resources.get(b"ColorSpace").map(|c| doc.resolve(c));
        let Some(spaces) = spaces.as_deref().and_then(Object::as_dict) else { return Defaults::NONE };
        let real = |key: &[u8]| {
            spaces.get(key).map(|s| doc.resolve(s)).is_some_and(|s| {
                s.as_array()
                    .and_then(|a| a.first())
                    .map(|k| doc.resolve(k))
                    .is_some_and(|k| k.as_name().is_some_and(|k| matches!(k, b"CalGray" | b"CalRGB" | b"Lab" | b"ICCBased")))
            })
        };
        Defaults { gray: real(b"DefaultGray"), rgb: real(b"DefaultRGB"), cmyk: real(b"DefaultCMYK") }
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

/// What a content stream's resources give it.
struct Holder {
    defaults: Defaults,
    /// The objects its `/XObject` and `/Shading` entries name.
    names: Vec<ObjRef>,
    /// It has no `/XObject` or no `/Shading` of its own, so a `Do` or `sh` may name the page's.
    falls_back: bool,
}

impl Holder {
    /// The holder whose `/Resources` value is `resources`.
    fn of(doc: &Document, resources: Option<&Object>) -> Holder {
        let resources = resources.map(|r| doc.resolve(r));
        let Some(r) = resources.as_deref().and_then(Object::as_dict) else {
            return Holder { defaults: Defaults::NONE, names: Vec::new(), falls_back: true };
        };
        let (mut names, mut falls_back) = (Vec::new(), false);
        for key in [b"XObject".as_slice(), b"Shading"] {
            match r.get(key).map(|x| doc.resolve(x)).as_deref().and_then(Object::as_dict) {
                Some(named) => names.extend(named.iter().filter_map(|(_, v)| v.as_ref())),
                None => falls_back = true,
            }
        }
        Holder { defaults: Defaults::in_dict(doc, r), names, falls_back }
    }
}

/// The colour spaces the resource dictionary `resources` uses, judged under its own Defaults: its
/// `/ColorSpace` entries, and those of the shadings written straight into its `/Shading` entries
/// or into the shading patterns of its `/Pattern` entries (a shading of its own object is judged
/// as an object).
fn resource_spaces(doc: &Document, resources: &Dict, out: &mut crate::DeviceColour) {
    let mut used = crate::DeviceColour::default();
    each(doc, resources, b"ColorSpace", |k, entry| {
        if !matches!(k, b"DefaultGray" | b"DefaultRGB" | b"DefaultCMYK") {
            crate::mark_cs_object(doc, entry, &mut used, 0);
        }
    });
    let mut shading = |s: &Object| {
        if let Object::Dict(s) = s
            && let Some(cs) = s.get(b"ColorSpace")
        {
            crate::mark_cs_object(doc, cs, &mut used, 0);
        }
    };
    each(doc, resources, b"Shading", |_, s| shading(s));
    each(doc, resources, b"Pattern", |_, p| {
        if let Some(s) = crate::dict_of(&doc.resolve(p)).and_then(|p| p.get(b"Shading")) {
            shading(s);
        }
    });
    Defaults::in_dict(doc, resources).uncover(used, out);
}

/// Calls `f` with each entry of the dictionary that `resources` holds under `key`.
fn each(doc: &Document, resources: &Dict, key: &[u8], mut f: impl FnMut(&[u8], &Object)) {
    let held = resources.get(key).map(|e| doc.resolve(e));
    for (k, v) in held.as_deref().and_then(Object::as_dict).into_iter().flat_map(Dict::iter) {
        f(k.as_slice(), v);
    }
}

/// 6.2.3.3: the device colour that no Default colour space covers (ISO 32000-1 8.6.5.6), and
/// whether some content could not be read. A content stream draws under its own resources: a
/// page's (inherited ones included), or a form's, pattern's, appearance stream's or Type 3 font's
/// own, never those of the page that draws it. A colour space in a resource dictionary is used
/// under that dictionary. An image or a shading is drawn under every resource dictionary that
/// names it, and, when a page names it, under any other content that may fall back to the page's
/// resources and draws something; one that nothing names (a soft mask, say) is judged under every
/// resource dictionary content draws under.
pub(super) fn uncovered_device_colour(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> crate::DeviceColour {
    // What each content stream draws under (a stream drawn under several holds only what all
    // cover) and whether it may fall back; what each named object is drawn under.
    let mut under: HashMap<ObjRef, (Defaults, bool)> = HashMap::new();
    let mut named: HashMap<ObjRef, Defaults> = HashMap::new();
    let mut add = |content: &[ObjRef], h: &Holder| {
        for r in content {
            under.entry(*r).and_modify(|(d, f)| (*d, *f) = (d.and(h.defaults), *f || h.falls_back)).or_insert((h.defaults, h.falls_back));
        }
        for n in &h.names {
            named.entry(*n).and_modify(|d| *d = d.and(h.defaults)).or_insert(h.defaults);
        }
    };
    let mut on_pages = HashSet::new();
    for p in pdfcraft_model::pages(doc) {
        let mut h = Holder::of(doc, p.dict.get(b"Resources"));
        h.falls_back = false;
        on_pages.extend(h.names.iter().copied());
        add(&crate::page_contents(doc, &p.dict), &h);
    }
    for (r, o) in objs {
        let Some(d) = crate::dict_of(o) else { continue };
        if matches!(&**o, Object::Stream(s) if crate::is_content(&s.dict)) {
            add(&[*r], &Holder::of(doc, d.get(b"Resources")));
        }
        if d.name(b"Subtype") == Some(b"Type3")
            && let Some(procs) = d.get(b"CharProcs").map(|c| doc.resolve(c))
            && let Some(procs) = procs.as_dict()
        {
            let glyphs: Vec<ObjRef> = procs.iter().filter_map(|(_, p)| p.as_ref()).collect();
            add(&glyphs, &Holder::of(doc, d.get(b"Resources")));
        }
    }
    // Any other content (the glyphs of a Type 3 font written straight into a resource dictionary)
    // has no Default space known to apply.
    for r in crate::referenced_content(doc, objs) {
        under.entry(r).or_insert((Defaults::NONE, true));
    }
    let everywhere = under.values().map(|(d, _)| *d).reduce(Defaults::and).unwrap_or(Defaults::NONE);
    let mut out = crate::DeviceColour::default();
    // What content that may fall back to a page's resources, and draws, is drawn under.
    let mut fallback = Defaults::ALL;
    let mut budget = pdfcraft_cos::DecodeBudget::new(crate::MAX_COLOUR_DECODED, usize::MAX);
    for (r, o) in objs {
        // Content is the expensive part: once all three spaces are uncovered no more is read.
        if !(out.gray && out.rgb && out.cmyk)
            && let Some(&(own, falls_back)) = under.get(r)
            && let Object::Stream(s) = &**o
        {
            let mut used = crate::DeviceColour::default();
            let draws = match budget.decode(s) {
                Ok(data) => {
                    let _ = crate::content_colour(&data, &mut used);
                    falls_back && pdfcraft_content::ops(&data).any(|op| matches!(op.op.as_slice(), b"Do" | b"sh"))
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
    // A shared resource dictionary is read once.
    let mut read = HashSet::new();
    for (r, o) in objs {
        let Some(d) = crate::dict_of(o) else { continue };
        if let Some(resources) = d.get(b"Resources").filter(|res| res.as_ref().is_none_or(|res| read.insert(res))).map(|res| doc.resolve(res))
            && let Some(resources) = resources.as_dict()
        {
            resource_spaces(doc, resources, &mut out);
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
    out
}
