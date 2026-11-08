// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Convert Colors to grayscale (PDF-XChange's Color Converter; Acrobat Print Production ▸ Convert
//! Colors). Fill and stroke colours set in DeviceRGB, DeviceCMYK, CalRGB, CalGray or ICC-based
//! spaces become DeviceGray by luminance (RGB: 0.30 R, 0.59 G and 0.11 B summed; CMYK: one minus
//! the same weighted sum plus K, at most one), in page content and the forms it draws. RGB images
//! PdfCraft decodes (8 bits, Flate or JPEG, default decode, no colour-key mask) become 8-bit gray
//! images. Separation, DeviceN, Indexed, Lab, patterns, shadings and inline images are left as they
//! are and counted, as are images past the run's decoding allowance ([`ImageBudget`]). Shared forms
//! and images are converted once into new objects; the originals stay for anything else that uses
//! them (a full save drops them when nothing does).

use std::collections::HashMap;

use pdfcraft_content::{Op, num, parse, serialize_ops};
use pdfcraft_cos::{Dict, Document, ImageBudget, ObjRef, Object, Stream};

use crate::OptimizeError;
use crate::images::{components, is_identity_decode, pixels};

/// How deep forms inside forms are followed.
const MAX_DEPTH: usize = 12;
/// How deep the graphics state stack is tracked.
const MAX_STACK: usize = 64;
/// Images with more pixels than this are left alone (memory).
const MAX_PIXELS: u64 = 80_000_000;

/// What a conversion did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorReport {
    pub pages: usize,
    /// Colour operators rewritten as gray.
    pub operators: usize,
    /// Images and forms converted.
    pub images: usize,
    pub forms: usize,
    /// Colours, images and shadings left as they were (spaces or encodings not converted).
    pub left: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Space {
    Gray,
    Rgb,
    Cmyk,
    Other,
}

fn device(name: &[u8]) -> Option<Space> {
    match name {
        b"DeviceGray" | b"G" | b"CalGray" => Some(Space::Gray),
        b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(Space::Rgb),
        b"DeviceCMYK" | b"CMYK" => Some(Space::Cmyk),
        _ => None,
    }
}

/// The kind of colour space `name` (an operand of `cs` / `CS`) names in `resources`.
fn space_of(doc: &Document, resources: &Dict, name: &[u8]) -> Space {
    if let Some(s) = device(name) {
        return s;
    }
    let spaces = resources.get(b"ColorSpace").map(|c| doc.resolve(c)).and_then(|c| c.as_dict().cloned()).unwrap_or_default();
    let Some(cs) = spaces.get(name).map(|c| doc.resolve(c)) else { return Space::Other };
    match &*cs {
        Object::Name(n) => device(n).unwrap_or(Space::Other),
        Object::Array(a) => match a.first().and_then(Object::as_name) {
            Some(b"ICCBased") => match a.get(1).map(|s| doc.resolve(s)).as_deref() {
                Some(Object::Stream(icc)) => match icc.dict.int(b"N") {
                    Some(1) => Space::Gray,
                    Some(3) => Space::Rgb,
                    Some(4) => Space::Cmyk,
                    _ => Space::Other,
                },
                _ => Space::Other,
            },
            Some(n) => device(n).unwrap_or(Space::Other),
            None => Space::Other,
        },
        _ => Space::Other,
    }
}

fn gray_of(space: Space, v: &[f64]) -> Option<f64> {
    let g = match (space, v) {
        (Space::Gray, [g]) => *g,
        (Space::Rgb, [r, g, b]) => 0.30 * r + 0.59 * g + 0.11 * b,
        (Space::Cmyk, [c, m, y, k]) => 1.0 - (0.30 * c + 0.59 * m + 0.11 * y + k).min(1.0),
        _ => return None,
    };
    g.is_finite().then_some(g.clamp(0.0, 1.0))
}

fn numbers(op: &Op) -> Option<Vec<f64>> {
    op.operands.iter().map(Object::as_f64).collect()
}

/// Forms and images already walked, shared across pages: old → new (itself when it stays), with
/// the depth of a walk that `MAX_DEPTH` cut short. A form without its own /Resources draws from
/// its caller's, so it is keyed by those resources too: their own object when indirect (shared
/// by every page using them), otherwise the page or form holding them.
struct Converted {
    xobjects: HashMap<(ObjRef, Option<ObjRef>), (ObjRef, Option<usize>)>,
    /// Whether the walk in progress reached a form past `MAX_DEPTH`.
    cut: bool,
    /// What decoding the images may still spend.
    images: ImageBudget,
}

/// Rewrite the colour operators of one content stream. Returns the new bytes, whether anything
/// changed, and the XObject names it draws.
fn convert_ops(doc: &Document, data: &[u8], resources: &Dict, report: &mut ColorReport) -> (Vec<u8>, bool, Vec<Vec<u8>>) {
    let parsed = parse(data);
    let mut out = Vec::with_capacity(parsed.ops.len());
    let mut changed = false;
    let mut drawn = Vec::new();
    // (fill, stroke) colour spaces; q/Q save and restore them.
    let mut state = (Space::Gray, Space::Gray);
    let mut stack = Vec::new();
    for op in parsed.ops {
        let gray = |o: &str, v: f64| Op::new(o, vec![num((v * 10_000.0).round() / 10_000.0)]);
        let replaced = match op.op.as_slice() {
            b"q" => {
                if stack.len() < MAX_STACK {
                    stack.push(state);
                }
                None
            }
            b"Q" => {
                state = stack.pop().unwrap_or(state);
                None
            }
            b"g" => {
                state.0 = Space::Gray;
                None
            }
            b"G" => {
                state.1 = Space::Gray;
                None
            }
            o @ (b"rg" | b"RG" | b"k" | b"K") => {
                let (space, fill) = match o {
                    b"rg" => (Space::Rgb, true),
                    b"RG" => (Space::Rgb, false),
                    b"k" => (Space::Cmyk, true),
                    _ => (Space::Cmyk, false),
                };
                if fill {
                    state.0 = Space::Gray;
                } else {
                    state.1 = Space::Gray;
                }
                numbers(&op).and_then(|v| gray_of(space, &v)).map(|g| gray(if fill { "g" } else { "G" }, g))
            }
            o @ (b"cs" | b"CS") => {
                let fill = o == b"cs";
                let space = op.name(0).map_or(Space::Other, |n| space_of(doc, resources, n));
                // Remember the original space: the `sc` values that follow are in it.
                if fill {
                    state.0 = space;
                } else {
                    state.1 = space;
                }
                if space == Space::Other {
                    report.left += 1;
                    None
                } else {
                    Some(Op::new(if fill { "cs" } else { "CS" }, vec![Object::name("DeviceGray")]))
                }
            }
            o @ (b"sc" | b"scn" | b"SC" | b"SCN") => {
                let fill = o.first() == Some(&b's');
                let space = if fill { state.0 } else { state.1 };
                match numbers(&op).and_then(|v| gray_of(space, &v)) {
                    Some(g) if space != Space::Gray || op.operands.len() == 1 => Some(gray(if fill { "sc" } else { "SC" }, g)),
                    _ => {
                        if space == Space::Other {
                            report.left += 1;
                        }
                        None
                    }
                }
            }
            b"Do" => {
                if let Some(n) = op.name(0) {
                    drawn.push(n.to_vec());
                }
                None
            }
            b"sh" | b"BI" => {
                report.left += 1;
                None
            }
            _ => None,
        };
        match replaced {
            Some(new) if new.op != op.op || new.operands != op.operands => {
                // A `cs` that already named DeviceGray, or a gray `sc`, is not a change.
                report.operators += 1;
                changed = true;
                out.push(new);
            }
            _ => out.push(op),
        }
    }
    (serialize_ops(&out), changed, drawn)
}

/// A converted copy of image `r`, or `None` when it is gray already or can't be converted here.
fn convert_image(doc: &Document, s: &Stream, budget: &mut ImageBudget) -> Option<Stream> {
    let d = &s.dict;
    if matches!(d.get(b"ImageMask"), Some(Object::Bool(true))) || d.int(b"BitsPerComponent") != Some(8) {
        return None;
    }
    if matches!(d.get(b"Mask"), Some(Object::Array(_))) {
        return None; // a colour-key mask is in the image's colour values
    }
    let n = components(doc, d.get(b"ColorSpace")?)?;
    if n != 3 || d.get(b"Decode").is_some_and(|dec| !is_identity_decode(doc, dec, n)) {
        return None;
    }
    let (w, h) = (u64::try_from(d.int(b"Width")?).ok()?, u64::try_from(d.int(b"Height")?).ok()?);
    if w == 0 || h == 0 || w.saturating_mul(h) > MAX_PIXELS {
        return None;
    }
    let px = pixels(doc, s, 3, budget)?;
    let gray: Vec<u8> = px
        .as_chunks::<3>()
        .0
        .iter()
        .map(|[r, g, b]| (0.30 * f64::from(*r) + 0.59 * f64::from(*g) + 0.11 * f64::from(*b)).round().clamp(0.0, 255.0) as u8)
        .collect();
    let mut nd = d.clone();
    for k in [&b"Filter"[..], b"DecodeParms", b"Decode", b"Length", b"DL"] {
        nd.remove(k);
    }
    nd.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
    Some(Stream::flate(nd, &gray))
}

/// Convert the XObjects named in `names` (from `resources`, identified by `owner` as in `Converted`),
/// returning the resources with the converted ones swapped in (or `None` if nothing changed).
fn convert_xobjects(
    doc: &mut Document,
    resources: &Dict,
    owner: ObjRef,
    names: &[Vec<u8>],
    depth: usize,
    done: &mut Converted,
    report: &mut ColorReport,
) -> Option<Dict> {
    let xo = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())?;
    let mut new_xo = xo.clone();
    let mut changed = false;
    for name in names {
        let Some(Object::Ref(r)) = xo.get(name).cloned() else { continue };
        let inherits = matches!(&*doc.get(r), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Form") && !has_own_resources(doc, s));
        let key = (r, inherits.then_some(owner));
        // A walk cut short deeper down is redone from here, so each form is walked at most once
        // per depth and context.
        let n = match done.xobjects.get(&key) {
            Some(&(n, cut)) if cut.is_none_or(|d| d <= depth) => {
                done.cut |= cut.is_some();
                n
            }
            _ => convert_xobject(doc, key, owner, resources, depth, done, report),
        };
        if n != r {
            new_xo.set(name.clone(), Object::Ref(n));
            changed = true;
        }
    }
    changed.then(|| {
        let mut res = resources.clone();
        res.set(b"XObject".to_vec(), Object::Dict(new_xo));
        res
    })
}

/// The image or form at `key.0` converted into a new object, or itself if it stays, remembered
/// in `done` so later draws reuse it.
fn convert_xobject(
    doc: &mut Document,
    key: (ObjRef, Option<ObjRef>),
    owner: ObjRef,
    parent: &Dict,
    depth: usize,
    done: &mut Converted,
    report: &mut ColorReport,
) -> ObjRef {
    let r = key.0;
    let s = match &*doc.get(r) {
        // Past the depth limit before copying: such a form may be drawn many times.
        Object::Stream(s) if depth >= MAX_DEPTH && s.dict.name(b"Subtype") == Some(b"Form") => {
            done.cut = true;
            return r;
        }
        Object::Stream(s) => s.clone(),
        _ => return r,
    };
    match s.dict.name(b"Subtype") {
        Some(b"Image") => {
            let n = match convert_image(doc, &s, &mut done.images) {
                Some(new) => {
                    report.images += 1;
                    doc.add(Object::Stream(new))
                }
                None => {
                    let gray = s.dict.get(b"ColorSpace").and_then(|c| components(doc, c)) == Some(1) || s.dict.get(b"ImageMask").is_some();
                    if !gray {
                        report.left += 1;
                    }
                    r
                }
            };
            done.xobjects.insert(key, (n, None));
            n
        }
        Some(b"Form") => {
            // Placeholder first, so a form that draws itself stops here. Unchanged forms are
            // remembered too: re-walking them on every draw grows as fan-out^depth.
            done.xobjects.insert(key, (r, None));
            let outer = std::mem::take(&mut done.cut);
            let n = convert_form(doc, &s, r, owner, parent, depth, done, report).unwrap_or(r);
            let cut = done.cut;
            done.cut |= outer;
            done.xobjects.insert(key, (n, cut.then_some(depth)));
            n
        }
        _ => r,
    }
}

/// What identifies the resources of `holder` (object `r`): their object when indirect, else `r`.
fn resources_id(holder: &Dict, r: ObjRef) -> ObjRef {
    holder.get(b"Resources").and_then(Object::as_ref).unwrap_or(r)
}

/// The form's own resource dictionary; without one it draws from its caller's.
fn own_resources(doc: &Document, s: &Stream) -> Option<Dict> {
    s.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())
}

/// Whether `own_resources` finds one, without copying it (checked on every draw).
fn has_own_resources(doc: &Document, s: &Stream) -> bool {
    match s.dict.get(b"Resources") {
        Some(Object::Ref(r)) => doc.get(*r).as_dict().is_some(),
        Some(o) => o.as_dict().is_some(),
        None => false,
    }
}

/// A converted copy of the form stream `s` (object `r`) as a new object, or `None` if it stays.
#[allow(clippy::too_many_arguments)]
fn convert_form(
    doc: &mut Document,
    s: &Stream,
    r: ObjRef,
    owner: ObjRef,
    parent: &Dict,
    depth: usize,
    done: &mut Converted,
    report: &mut ColorReport,
) -> Option<ObjRef> {
    let (resources, owner) = match own_resources(doc, s) {
        Some(res) => (res, resources_id(&s.dict, r)),
        None => (parent.clone(), owner),
    };
    let data = s.decoded().ok()?;
    let (bytes, changed, drawn) = convert_ops(doc, &data, &resources, report);
    let new_res = convert_xobjects(doc, &resources, owner, &drawn, depth + 1, done, report);
    if !changed && new_res.is_none() {
        return None;
    }
    let mut d = s.dict.clone();
    for k in [&b"Filter"[..], b"DecodeParms", b"Length", b"DL"] {
        d.remove(k);
    }
    if let Some(res) = new_res {
        d.set(b"Resources".to_vec(), Object::Dict(res));
    }
    report.forms += 1;
    Some(doc.add(Object::Stream(Stream::flate(d, &bytes))))
}

fn content_bytes(doc: &Document, contents: &Object) -> Option<Vec<u8>> {
    match &*doc.resolve(contents) {
        Object::Stream(s) => s.decoded().ok(),
        Object::Array(a) => {
            let mut v = Vec::new();
            for c in a {
                if let Object::Stream(s) = &*doc.resolve(c) {
                    v.extend(s.decoded().ok()?);
                    v.push(b'\n');
                }
            }
            Some(v)
        }
        _ => None,
    }
}

/// Convert `pages` (0-based; `None` for all) to grayscale.
pub fn convert_to_gray(doc: &mut Document, pages: Option<&[usize]>) -> Result<ColorReport, OptimizeError> {
    let all = pdfcraft_model::pages(doc);
    if all.is_empty() {
        return Err(OptimizeError::NoPages);
    }
    let mut report = ColorReport::default();
    let mut done = Converted { xobjects: HashMap::new(), cut: false, images: ImageBudget::new(doc) };
    for (i, page) in all.iter().enumerate() {
        if pages.is_some_and(|p| !p.contains(&i)) {
            continue;
        }
        report.pages += 1;
        let resources = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
        let Some(data) = page.dict.get(b"Contents").and_then(|c| content_bytes(doc, c)) else { continue };
        let (bytes, changed, drawn) = convert_ops(doc, &data, &resources, &mut report);
        let new_res = convert_xobjects(doc, &resources, resources_id(&page.dict, page.obj), &drawn, 0, &mut done, &mut report);
        if changed || new_res.is_some() {
            // New objects: the old contents and resources may be shared with other pages.
            let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), &bytes)));
            let res = new_res.unwrap_or(resources);
            doc.update_dict(page.obj, |p| {
                p.set(b"Contents".to_vec(), Object::Ref(contents));
                p.set(b"Resources".to_vec(), Object::Dict(res));
            })?;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops_of(bytes: &[u8]) -> Vec<String> {
        parse(bytes).ops.iter().map(|o| String::from_utf8_lossy(&serialize_ops(std::slice::from_ref(o))).trim().to_string()).collect()
    }

    #[test]
    fn device_colours_become_gray_and_others_stay() {
        let doc = Document::new_empty();
        let mut res = Dict::new();
        let mut spaces = Dict::new();
        spaces.set(b"Sep".to_vec(), Object::Array(vec![Object::name("Separation"), Object::name("Spot"), Object::name("DeviceCMYK"), Object::Null]));
        res.set(b"ColorSpace".to_vec(), Object::Dict(spaces));
        let src = b"1 0 0 rg 0 0 1 RG 0 0 0 1 k q /DeviceRGB cs 0 1 0 sc Q 0.5 g /Sep cs 0.7 scn";
        let mut report = ColorReport::default();
        let (out, changed, _) = convert_ops(&doc, src, &res, &mut report);
        assert!(changed);
        let ops = ops_of(&out);
        assert_eq!(&ops[..6], ["0.3 g", "0.11 G", "0 g", "q", "/DeviceGray cs", "0.59 sc"], "{ops:?}");
        assert!(ops.contains(&"/Sep cs".to_string()) && ops.contains(&"0.7 scn".to_string()), "a separation stays: {ops:?}");
        assert_eq!(report.operators, 5);
        assert_eq!(report.left, 2, "the separation's cs and its scn");
    }

    #[test]
    fn rgb_images_become_gray() {
        let mut doc = Document::new_empty();
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"Width".to_vec(), Object::Int(2));
        d.set(b"Height".to_vec(), Object::Int(1));
        d.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        let img = Stream::flate(d, &[255, 0, 0, 0, 0, 255]);
        let gray = convert_image(&doc, &img, &mut ImageBudget::with_work(u64::MAX)).unwrap();
        assert_eq!(gray.dict.name(b"ColorSpace"), Some(&b"DeviceGray"[..]));
        assert_eq!(gray.decoded().unwrap(), vec![77, 28]);
        // Odd images are left alone.
        let mut bad = img.clone();
        bad.dict.set(b"Width".to_vec(), Object::Int(-4));
        assert!(convert_image(&doc, &bad, &mut ImageBudget::with_work(u64::MAX)).is_none());
        // Past the run's allowance an image is left as it is, and not decoded.
        let mut spent = ImageBudget::with_work(0);
        assert!(convert_image(&doc, &img, &mut spent).is_none());
        assert_eq!(spent.refused(), 1);
        let _ = doc.add(Object::Null);
    }

    #[test]
    fn odd_streams_do_not_panic() {
        let doc = Document::new_empty();
        for s in [&b"rg"[..], b"1 rg", b"/X cs sc scn", b"Q Q Q q", b"1 2 3 4 5 k", b"(a) rg"] {
            let _ = convert_ops(&doc, s, &Dict::new(), &mut ColorReport::default());
        }
    }
}
