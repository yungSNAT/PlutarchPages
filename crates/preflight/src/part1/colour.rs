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

    /// The Default spaces of the resource dictionary `resources` (a `/Resources` value). An entry
    /// counts only when it is a colour space array that is no device space: a null, a dangling
    /// reference, a number or `/DeviceRGB` itself changes nothing.
    fn of(doc: &Document, resources: Option<&Object>) -> Defaults {
        let resources = resources.map(|r| doc.resolve(r));
        resources.as_deref().and_then(Object::as_dict).map_or(Defaults::NONE, |r| Defaults::in_dict(doc, r))
    }

    /// [`Defaults::of`] the resource dictionary `resources` itself.
    fn in_dict(doc: &Document, resources: &Dict) -> Defaults {
        let spaces = resources.get(b"ColorSpace").map(|c| doc.resolve(c));
        let Some(spaces) = spaces.as_deref().and_then(Object::as_dict) else { return Defaults::NONE };
        let real = |key: &[u8]| {
            spaces.get(key).map(|s| doc.resolve(s)).is_some_and(|s| {
                s.as_array()
                    .and_then(|a| a.first())
                    .map(|k| doc.resolve(k))
                    .is_some_and(|k| k.as_name().is_some_and(|k| !matches!(k, b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK")))
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

/// 6.2.3.3: the device colour that no Default colour space covers (ISO 32000-1 8.6.5.6), and
/// whether some content could not be read. A content stream draws under its own resources: a
/// page's (inherited ones included), or a form's, pattern's, appearance stream's or Type 3 font's
/// own, never those of the page that draws it. A colour space in a resource dictionary is used
/// under that dictionary. Which content draws an image or a shading isn't followed, so its
/// `/ColorSpace` is covered only when every resource dictionary that content draws under covers it.
pub(super) fn uncovered_device_colour(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> crate::DeviceColour {
    // What each content stream draws under: a stream drawn under several holds only what all cover.
    let mut under: HashMap<ObjRef, Defaults> = HashMap::new();
    let mut add = |r: ObjRef, d: Defaults| {
        under.entry(r).and_modify(|e| *e = e.and(d)).or_insert(d);
    };
    for p in pdfcraft_model::pages(doc) {
        let d = Defaults::of(doc, p.dict.get(b"Resources"));
        for r in crate::page_contents(doc, &p.dict) {
            add(r, d);
        }
    }
    for (r, o) in objs {
        let Some(d) = crate::dict_of(o) else { continue };
        if matches!(&**o, Object::Stream(s) if crate::is_content(&s.dict)) {
            add(*r, Defaults::of(doc, d.get(b"Resources")));
        }
        if d.name(b"Subtype") == Some(b"Type3")
            && let Some(procs) = d.get(b"CharProcs").map(|c| doc.resolve(c))
            && let Some(procs) = procs.as_dict()
        {
            let own = Defaults::of(doc, d.get(b"Resources"));
            for p in procs.iter().filter_map(|(_, p)| p.as_ref()) {
                add(p, own);
            }
        }
    }
    // Any other content (the glyphs of a Type 3 font written straight into a resource dictionary)
    // has no Default space known to apply.
    for r in crate::referenced_content(doc, objs) {
        under.entry(r).or_insert(Defaults::NONE);
    }
    let everywhere = under.values().copied().reduce(Defaults::and).unwrap_or(Defaults::NONE);
    let mut out = crate::DeviceColour::default();
    let mut budget = pdfcraft_cos::DecodeBudget::new(crate::MAX_COLOUR_DECODED, usize::MAX);
    // A shared resource dictionary is read once.
    let mut read = HashSet::new();
    for (r, o) in objs {
        let Some(d) = crate::dict_of(o) else { continue };
        if let Some(resources) = d.get(b"Resources").filter(|res| res.as_ref().is_none_or(|res| read.insert(res))).map(|res| doc.resolve(res))
            && let Some(resources) = resources.as_dict()
            && let Some(spaces) = resources.get(b"ColorSpace").map(|c| doc.resolve(c))
            && let Some(spaces) = spaces.as_dict()
        {
            let mut used = crate::DeviceColour::default();
            for (_, entry) in spaces.iter().filter(|(k, _)| !matches!(k.as_slice(), b"DefaultGray" | b"DefaultRGB" | b"DefaultCMYK")) {
                crate::mark_cs_object(doc, entry, &mut used, 0);
            }
            Defaults::in_dict(doc, resources).uncover(used, &mut out);
        }
        if let Some(cs) = d.get(b"ColorSpace")
            && !matches!(&*doc.resolve(cs), Object::Dict(_))
        {
            let mut used = crate::DeviceColour::default();
            crate::mark_cs_object(doc, cs, &mut used, 0);
            everywhere.uncover(used, &mut out);
        }
        // Content is the expensive part: once all three spaces are uncovered no more is read.
        if !(out.gray && out.rgb && out.cmyk)
            && let Some(&own) = under.get(r)
            && let Object::Stream(s) = &**o
        {
            let mut used = crate::DeviceColour::default();
            match budget.decode(s) {
                Ok(data) => {
                    let _ = crate::content_colour(&data, &mut used);
                }
                Err(_) => out.incomplete = true,
            }
            own.uncover(used, &mut out);
        }
    }
    out
}
