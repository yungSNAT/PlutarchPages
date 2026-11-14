//! Walk page content streams and collect the opaque filled rectangles they draw.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. A single closed `m`/`l` subpath
//! whose four corners form an axis-aligned rectangle in user space is the same shape and counts
//! too (#1510), and so does a shading painted with `sh`, over the box that holds its clip. Graphics
//! state tracked here is deliberately small: the CTM stack (`q`/`Q`/`cm`), the box holding the clip
//! (`W` paths, a form's `/BBox`, the page's crop box), the non-stroking color (`g`/`rg`/`k`, and
//! `cs` with `sc`/`scn` in gray, RGB and CMYK spaces; any other, a pattern's or a spot color's, is
//! one the audit can't read and counts as dark as the text) and the non-stroking alpha from
//! ExtGState (`gs` reading `/ca`). Form XObjects are followed read-only with a depth cap, and so
//! are the appearances a viewer paints for the page's annotations. Other filled paths are out of
//! scope and documented as such.
//!
//! The walk also reads the glyph procedures of the Type 3 fonts the content selects (with `Tf`,
//! or an ExtGState's `/Font`) and the cells of the tiling patterns it paints with, once for each
//! set of resources they read. Placing what those draw would mean painting each glyph or tile
//! where it lands, so the audit reads no text there: one that shows text fails it.
//!
//! Paint order: a page's content streams are one stream in pieces (ISO 32000-2 §7.8.2), so they
//! are parsed joined and walked once with a single graphics state — state set in an earlier
//! stream (a `cm`, a fill color) is still in force in the later ones. Each rectangle records the
//! position of the fill operator that painted it: its layer (0 for the page's content, then one
//! for each annotation, in `/Annots` order), then as `TextLine::paint_order` gives a line's, the
//! index of each `Do` that drew the form it is in, outermost first, and the fill's index in the
//! joined stream or the form's. Positions compare lexicographically in paint order.

use std::collections::HashSet;
use std::sync::Arc;

use pdfcraft_content::{Matrix, Op, Pieces, ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

use crate::AuditError;

/// Maximum form-XObject nesting followed. Deeper nesting (including a cycle) fails the audit: a
/// cover drawn below the cut could otherwise go unseen and the page be reported clean.
const MAX_DEPTH: usize = 32;
/// Bound on total operators walked per page, so a hostile stream cannot spin forever.
/// Hitting it fails the audit: a truncated walk must never report "clean".
const MAX_OPS: usize = 2_000_000;

/// One opaque filled rectangle, in page user space.
#[derive(Clone, Debug, PartialEq)]
pub struct FilledRect {
    /// `[x0, y0, x1, y1]`, normalized so `x0 <= x1` and `y0 <= y1`.
    pub rect: [f64; 4],
    /// The fill color as sRGB-ish `[r, g, b]` (CMYK is converted approximately), or `None` when the
    /// audit can't read it.
    pub rgb: Option<[f64; 3]>,
    /// Paint position: its layer, the `Do` operators that drew it, outermost first, then the fill
    /// operator's index (see the module documentation).
    pub pos: Vec<usize>,
}

/// The appearance a viewer paints for an annotation: its form and where the form lands.
pub struct Appearance {
    /// The annotation's place in paint order, after the page's content (layer 0).
    pub layer: usize,
    /// The entry naming the form, as a page's `/Contents` names a stream.
    pub contents: Object,
    /// The form stream, shared rather than copied: one appearance can be listed many times.
    pub form: Arc<Object>,
    /// The form's `/Matrix` composed with the one that maps its box onto the annotation's.
    pub ctm: Matrix,
}

/// Content the audit cannot place on the page, walked only for the text it shows.
#[derive(Clone, Copy)]
enum Unplaced {
    Pattern,
    Glyph,
}

impl Unplaced {
    fn what(self) -> &'static str {
        match self {
            Unplaced::Pattern => "a tiling pattern",
            Unplaced::Glyph => "a Type 3 glyph",
        }
    }
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    /// The non-stroking color, as [`FilledRect::rgb`] gives it.
    fill: Option<[f64; 3]>,
    /// The components of the non-stroking color space when the audit reads its colors: 1 (gray), 3
    /// (RGB) or 4 (CMYK), else 0.
    space: usize,
    alpha: f64,
    /// A box in page user space that holds the clip.
    clip: [f64; 4],
}

impl Gs {
    fn new(clip: [f64; 4]) -> Self {
        Gs { ctm: Matrix::IDENTITY, fill: Some([0.0; 3]), space: 1, alpha: 1.0, clip }
    }
}

/// The path under construction, as far as the audit cares about it.
enum Path {
    Empty,
    /// A `re` rectangle, in user space.
    Rect([f64; 4]),
    /// One `m`/`l` subpath, corners already mapped to user space; `true` once closed by `h`.
    Lines(Vec<[f64; 2]>, bool),
    /// Anything else: curves, a second subpath, a CTM change mid-path.
    Other,
}

impl Path {
    /// The rectangle a fill of this path paints, if the path is one.
    fn rect(self) -> Option<[f64; 4]> {
        match self {
            Path::Rect(r) => Some(r),
            Path::Lines(pts, _) => lines_rect(pts),
            Path::Empty | Path::Other => None,
        }
    }
}

/// The bounding box of a closed polygon that is an axis-aligned rectangle, or `None`. A repeated
/// start point is dropped first; the remaining four corners must alternate horizontal and
/// vertical edges.
fn lines_rect(mut pts: Vec<[f64; 2]>) -> Option<[f64; 4]> {
    // CTM products leave rounding noise, so corners compare with a tolerance (in points).
    const EPS: f64 = 1e-6;
    let same = |a: f64, b: f64| (a - b).abs() <= EPS;
    if pts.len() == 5 && same(pts[0][0], pts[4][0]) && same(pts[0][1], pts[4][1]) {
        pts.pop();
    }
    let [a, b, c, d] = <[[f64; 2]; 4]>::try_from(pts).ok()?;
    let fits = |first_horizontal: bool| {
        [(a, b), (b, c), (c, d), (d, a)]
            .iter()
            .enumerate()
            .all(|(i, (p, q))| if (i % 2 == 0) == first_horizontal { same(p[1], q[1]) } else { same(p[0], q[0]) })
    };
    if !(fits(true) || fits(false)) {
        return None;
    }
    let (xs, ys) = ([a[0], b[0], c[0], d[0]], [a[1], b[1], c[1], d[1]]);
    let r = [
        xs.into_iter().fold(f64::INFINITY, f64::min),
        ys.into_iter().fold(f64::INFINITY, f64::min),
        xs.into_iter().fold(f64::NEG_INFINITY, f64::max),
        ys.into_iter().fold(f64::NEG_INFINITY, f64::max),
    ];
    (r[2] - r[0] > EPS && r[3] - r[1] > EPS).then_some(r)
}

/// Whose resources a stream reads its names from: the page's (which its appearances without
/// resources of their own read too), or an object's. A Type 3 font or a tiling pattern without
/// resources of its own reads those where it is used, so what its glyphs or cell draw depends on
/// them.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Owner {
    Page,
    Object(ObjRef),
    /// Resources of a stream written in place, which has no object number to tell them apart by:
    /// never taken as seen before.
    Unnumbered,
}

impl Owner {
    /// The owner of the resources `entry` names for the stream `stream`: the resources dictionary
    /// itself when it is an object of its own, which streams can share, otherwise the stream.
    fn of(entry: &Object, stream: Option<ObjRef>) -> Owner {
        entry.as_ref().or(stream).map_or(Owner::Unnumbered, Owner::Object)
    }
}

/// The resources a stream reads names from, and whose they are.
#[derive(Clone, Copy)]
struct Res<'r> {
    dict: &'r Dict,
    owner: Owner,
}

struct Walker<'a> {
    doc: &'a Document,
    out: Vec<FilledRect>,
    ops_seen: usize,
    /// Decoded bytes and form entries left for the page, its forms included: spending either
    /// fails the audit.
    budget: DecodeBudget,
    /// The 0-based page, for errors.
    page: usize,
    /// The layer being walked (see [`FilledRect::pos`]).
    layer: usize,
    /// Set inside a glyph procedure or a pattern cell: rectangles there are not covers, and text
    /// fails the audit.
    inside: Option<Unplaced>,
    /// The fonts and patterns already looked at on this page, with the owner of the resources they
    /// were read with: a Type 3 font selected on every line is walked once for each set.
    walked: HashSet<(ObjRef, Owner)>,
}

impl<'a> Walker<'a> {
    fn bump(&mut self) -> Result<(), AuditError> {
        self.ops_seen += 1;
        if self.ops_seen > MAX_OPS {
            return Err(AuditError::TooManyOps);
        }
        Ok(())
    }

    /// Walk one stream's operators, read one at a time (a stream of millions of short operators
    /// is never held whole), with the given graphics state. `calls` holds the `Do` operators that
    /// drew the form being walked, outermost first (none at page level): with an operator's
    /// index, its paint position.
    fn walk_ops(
        &mut self,
        ops: impl Iterator<Item = Op>,
        gs: &mut Gs,
        stack: &mut Vec<Gs>,
        res: Res<'_>,
        calls: &mut Vec<usize>,
    ) -> Result<(), AuditError> {
        let resources = res.dict;
        let mut path = Path::Empty;
        // The box holding every point of the path, in user space (a curve lies within its control
        // points), and whether `W` makes the path the clip once it is painted.
        let mut path_box: Option<[f64; 4]> = None;
        let mut clipping = false;
        for (i, op) in ops.enumerate() {
            self.bump()?;
            let op = &op;
            match op.op.as_slice() {
                b"m" | b"l" | b"c" | b"v" | b"y" => {
                    for p in op.operands.chunks_exact(2) {
                        if let (Some(x), Some(y)) = (p[0].as_f64(), p[1].as_f64()) {
                            let (x, y) = gs.ctm.apply(x, y);
                            path_box = Some(join(path_box, [x, y, x, y]));
                        }
                    }
                }
                b"re" => {
                    if let Some([x, y, w, h]) = op.nums::<4>() {
                        path_box = Some(join(path_box, gs.ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)])));
                    }
                }
                _ => {}
            }
            match op.op.as_slice() {
                b"q" => stack.push(gs.clone()),
                b"Q" => {
                    // The current path is not graphics state (ISO 32000-2 §8.4.1): `re Q f` still
                    // fills the rectangle, whose corners are already in user space.
                    if let Some(g) = stack.pop() {
                        *gs = g;
                    }
                }
                b"cm" => {
                    let before = gs.ctm;
                    if let Some(m) = Matrix::from_operands(&op.operands)
                        && m.0.iter().all(|v| v.is_finite())
                    {
                        gs.ctm = m.then(&gs.ctm);
                    }
                    // A `cm` that leaves the CTM unchanged (an identity matrix) cannot move the
                    // pending path, so keep it for the fill (#1333).
                    if gs.ctm != before && !matches!(path, Path::Empty) {
                        path = Path::Other;
                    }
                }
                name @ (b"g" | b"rg" | b"k") => {
                    let n = match name {
                        b"g" => 1,
                        b"rg" => 3,
                        _ => 4,
                    };
                    if let Some(fill) = op.operands.len().checked_sub(n).and_then(|k| numbers(&op.operands[k..])).and_then(|v| rgb(&v)) {
                        (gs.fill, gs.space) = (Some(fill), n);
                    }
                }
                b"cs" => {
                    // DeviceCMYK starts black, and every other space the audit reads at 0 (ISO 32000-2
                    // §8.6.8): black, or white for an ICC-based CMYK one.
                    let name = op.name(0).unwrap_or_default();
                    gs.space = components(self.doc, resources, name);
                    gs.fill = rgb(if name == b"DeviceCMYK" { &[0.0, 0.0, 0.0, 1.0] } else { &[0.0; 4][..gs.space] });
                }
                b"sc" | b"scn" => {
                    gs.fill = numbers(&op.operands).filter(|v| v.len() == gs.space).and_then(|v| rgb(&v));
                    if let Some(name) = op.operands.last().and_then(Object::as_name) {
                        self.walk_pattern(name, i, res, calls)?;
                    }
                }
                b"gs" => {
                    if let Some(name) = op.name(0) {
                        gs.alpha = ext_alpha(self.doc, resources, name);
                        self.walk_state_font(name, i, res, calls)?;
                    }
                }
                b"re" => {
                    let rect = op.nums::<4>().and_then(|[x, y, w, h]| {
                        if [x, y, w, h].iter().all(|v| v.is_finite()) && w != 0.0 && h != 0.0 {
                            let r = gs.ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]);
                            (r[2] > r[0] && r[3] > r[1]).then_some(r)
                        } else {
                            None
                        }
                    });
                    path = rect.map_or(Path::Other, Path::Rect);
                }
                b"m" => {
                    path = match (path, user_point(gs, op)) {
                        (Path::Empty, Some(p)) => Path::Lines(vec![p], false),
                        _ => Path::Other,
                    };
                }
                b"l" => {
                    path = match (path, user_point(gs, op)) {
                        // Past a repeated start point it can no longer be a rectangle.
                        (Path::Lines(mut pts, false), Some(p)) if pts.len() < 5 => {
                            pts.push(p);
                            Path::Lines(pts, false)
                        }
                        _ => Path::Other,
                    };
                }
                b"h" => {
                    path = match path {
                        Path::Lines(pts, _) => Path::Lines(pts, true),
                        // `re` already closed its subpath, so a further `h` changes nothing.
                        Path::Rect(r) => Path::Rect(r),
                        _ => Path::Other,
                    };
                }
                // `b`/`b*` close the path before filling it, which the fill does anyway.
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                    if let Some(r) = std::mem::replace(&mut path, Path::Empty).rect()
                        && gs.alpha >= 1.0
                        && self.inside.is_none()
                    {
                        self.paint(r, gs.fill, calls, i);
                    }
                }
                // A shading paints everywhere the clip lets it.
                b"sh" => {
                    if gs.alpha >= 1.0 && self.inside.is_none() && gs.clip[2] > gs.clip[0] && gs.clip[3] > gs.clip[1] {
                        self.paint(gs.clip, None, calls, i);
                    }
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if let Some(unplaced) = self.inside {
                        return Err(AuditError::UncheckedText(self.page, unplaced.what()));
                    }
                }
                b"Tf" => {
                    let fonts = resources.get(b"Font").map(|o| self.doc.resolve(o));
                    if let Some(entry) = op.name(0).and_then(|name| fonts.as_deref().and_then(Object::as_dict)?.get(name)) {
                        self.walk_type3(entry, i, res, calls)?;
                    }
                }
                b"SCN" => {
                    if let Some(name) = op.operands.last().and_then(Object::as_name) {
                        self.walk_pattern(name, i, res, calls)?;
                    }
                }
                b"S" | b"s" | b"n" => path = Path::Empty,
                b"W" | b"W*" => {
                    clipping = true;
                    path = Path::Other;
                }
                b"c" | b"v" | b"y" => path = Path::Other,
                b"Do" => {
                    if let Some(name) = op.name(0) {
                        if calls.len() >= MAX_DEPTH {
                            return Err(AuditError::TooDeep);
                        }
                        calls.push(i);
                        self.walk_xobject(name, gs, res, calls)?;
                        calls.pop();
                    }
                    path = Path::Empty;
                }
                _ => {}
            }
            // A path ends where it is painted, and after `W` it is the clip from then on.
            if matches!(op.op.as_slice(), b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"n") {
                if std::mem::take(&mut clipping)
                    && let Some(b) = path_box
                {
                    gs.clip = meet(gs.clip, b);
                }
                path_box = None;
            }
        }
        Ok(())
    }

    /// Record a cover painted over `rect` in `rgb` by the operator at index `at`.
    fn paint(&mut self, rect: [f64; 4], rgb: Option<[f64; 3]>, calls: &[usize], at: usize) {
        let pos = std::iter::once(self.layer).chain(calls.iter().copied()).chain(std::iter::once(at)).collect();
        self.out.push(FilledRect { rect, rgb, pos });
    }

    /// Follow a form XObject read-only, composing its matrix onto the current CTM. State
    /// changes inside never leak out. `calls` ends with the `Do` that draws it.
    fn walk_xobject(&mut self, name: &[u8], gs: &Gs, res: Res<'_>, calls: &mut Vec<usize>) -> Result<(), AuditError> {
        // Borrowed, not copied: a `Do` must not cost the size of the whole XObject table.
        let xobjects = res.dict.get(b"XObject").map(|o| self.doc.resolve(o));
        let Some(entry) = xobjects.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)) else { return Ok(()) };
        let resolved = self.doc.resolve(entry);
        let Object::Stream(stream) = &*resolved else { return Ok(()) };
        let dict = &stream.dict;
        if crate::subtype(self.doc, dict)?.as_deref() != Some(b"Form") {
            return Ok(());
        }
        let matrix = form_matrix(self.doc, dict);
        // A form without resources of its own reads those it is drawn with (PDF 1.1), as the text
        // walk and the renderer do: a cover in a form it draws must not go unseen.
        let own = dict.get(b"Resources").map(|o| (o, self.doc.resolve(o)));
        let sub = own_resources(own.as_ref(), entry.as_ref()).unwrap_or(res);
        if !self.budget.enter_form() {
            return Err(AuditError::TooManyForms);
        }
        // A form stream that cannot be decoded fails the audit: it might hide a cover.
        let data = self.budget.decode_strict(stream)?;
        // The XObject matrix maps form space into the user space active at `Do`.
        let mut sub_gs = gs.clone();
        sub_gs.ctm = matrix.then(&gs.ctm);
        if let Some(bbox) = form_bbox(self.doc, dict) {
            sub_gs.clip = meet(sub_gs.clip, sub_gs.ctm.bbox(bbox));
        }
        let mut sub_stack = Vec::new();
        self.walk_ops(ops(&data), &mut sub_gs, &mut sub_stack, sub, calls)
    }

    /// Walk the font an ExtGState's `/Font` sets (ISO 32000-2 §8.4.5) as [`Walker::walk_type3`]
    /// walks one `Tf` selects: the `gs` at index `at` names the ExtGState.
    fn walk_state_font(&mut self, name: &[u8], at: usize, res: Res<'_>, calls: &mut Vec<usize>) -> Result<(), AuditError> {
        // Borrowed, not copied: a `gs` must not cost the size of the whole ExtGState table.
        let states = res.dict.get(b"ExtGState").map(|o| self.doc.resolve(o));
        let Some(state) = states.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)).map(|o| self.doc.resolve(o)) else { return Ok(()) };
        let font = state.as_dict().and_then(|d| d.get(b"Font")).map(|o| self.doc.resolve(o));
        match font.as_deref().and_then(Object::as_array).and_then(|a| a.first()) {
            Some(entry) => self.walk_type3(entry, at, res, calls),
            None => Ok(()),
        }
    }

    /// Walk the glyph procedures of the Type 3 font `entry`, selected at index `at`, as unplaced
    /// content, once a page for each set of resources they read.
    fn walk_type3(&mut self, entry: &Object, at: usize, res: Res<'_>, calls: &mut Vec<usize>) -> Result<(), AuditError> {
        let font = self.doc.resolve(entry);
        let Some(font) = font.as_dict() else { return Ok(()) };
        // A font without `/Resources` reads those where it is selected (PDF 1.1), as its glyphs
        // are drawn there: a glyph drawing a form that only some of them name shows its text there
        // alone.
        let own = font.get(b"Resources").map(|o| (o, self.doc.resolve(o)));
        let glyph = own_resources(own.as_ref(), entry.as_ref()).unwrap_or(res);
        if !self.first_walk(entry, glyph.owner) || crate::subtype(self.doc, font)?.as_deref() != Some(b"Type3") {
            return Ok(());
        }
        let procs = font.get(b"CharProcs").map(|o| self.doc.resolve(o));
        for (_, proc) in procs.as_deref().and_then(Object::as_dict).into_iter().flat_map(Dict::iter) {
            // Each entry looked at counts as an operator: fonts sharing one large table of entries
            // that aren't streams would otherwise be read past every bound.
            self.bump()?;
            if let Object::Stream(s) = &*self.doc.resolve(proc) {
                self.walk_unplaced(Unplaced::Glyph, s, at, glyph, calls)?;
            }
        }
        Ok(())
    }

    /// Walk the cell of the tiling pattern `scn` paints with at index `at` as unplaced content,
    /// once a page for each set of resources it reads. Shading patterns are dictionaries and draw
    /// no text.
    fn walk_pattern(&mut self, name: &[u8], at: usize, res: Res<'_>, calls: &mut Vec<usize>) -> Result<(), AuditError> {
        let patterns = res.dict.get(b"Pattern").map(|o| self.doc.resolve(o));
        let Some(entry) = patterns.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)) else { return Ok(()) };
        let pattern = self.doc.resolve(entry);
        let Object::Stream(s) = &*pattern else { return Ok(()) };
        let own = s.dict.get(b"Resources").map(|o| (o, self.doc.resolve(o)));
        let cell = own_resources(own.as_ref(), entry.as_ref()).unwrap_or(res);
        if !self.first_walk(entry, cell.owner) {
            return Ok(());
        }
        self.walk_unplaced(Unplaced::Pattern, s, at, cell, calls)
    }

    /// Whether the font or pattern `entry` is yet to be walked with the resources `owner` holds.
    /// One written in place, or read with resources written in place, is walked each time.
    fn first_walk(&mut self, entry: &Object, owner: Owner) -> bool {
        match entry.as_ref() {
            Some(r) if owner != Owner::Unnumbered => self.walked.insert((r, owner)),
            _ => true,
        }
    }

    /// Walk `stream`, reached from the operator at index `at`, as `unplaced` content: forms it
    /// draws count towards the depth and the page's budget like any other.
    fn walk_unplaced(&mut self, unplaced: Unplaced, stream: &Stream, at: usize, res: Res<'_>, calls: &mut Vec<usize>) -> Result<(), AuditError> {
        if calls.len() >= MAX_DEPTH {
            return Err(AuditError::TooDeep);
        }
        if !self.budget.enter_form() {
            return Err(AuditError::TooManyForms);
        }
        let data = self.budget.decode_strict(stream)?;
        let outer = self.inside.replace(unplaced);
        calls.push(at);
        // Nothing painted here is placed, so the clip is never read.
        self.walk_ops(ops(&data), &mut Gs::new([0.0; 4]), &mut Vec::new(), res, calls)?;
        calls.pop();
        self.inside = outer;
        Ok(())
    }
}

/// The resources a stream has of its own, `own` being its `/Resources` entry and what that
/// resolves to, `stream` its object number: `None` when it has none that are a dictionary, and it
/// reads those where it is used.
fn own_resources<'r>(own: Option<&'r (&Object, Arc<Object>)>, stream: Option<ObjRef>) -> Option<Res<'r>> {
    let (entry, resolved) = own?;
    Some(Res { dict: resolved.as_dict()?, owner: Owner::of(entry, stream) })
}

/// A form's `/BBox`, normalized, or `None` when it has none that is usable.
pub(crate) fn form_bbox(doc: &Document, dict: &Dict) -> Option<[f64; 4]> {
    let bbox = dict.get(b"BBox").map(|o| doc.resolve(o));
    let v: Vec<f64> = bbox.as_deref().and_then(Object::as_array)?.iter().filter_map(|o| doc.resolve(o).as_f64()).collect();
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(v).ok().filter(|b| b.iter().all(|x| x.is_finite()))?;
    Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
}

/// The box holding boxes `a` and `b`.
fn join(a: Option<[f64; 4]>, b: [f64; 4]) -> [f64; 4] {
    a.map_or(b, |a| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])
}

/// The box both `a` and `b` hold, empty (`x0 > x1` or `y0 > y1`) when they don't meet.
fn meet(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

/// Operands that are all finite numbers, as numbers.
fn numbers(operands: &[Object]) -> Option<Vec<f64>> {
    operands.iter().map(|o| o.as_f64().filter(|v| v.is_finite())).collect()
}

/// A color of 1 (gray), 3 (RGB) or 4 (CMYK) components as sRGB-ish `[r, g, b]`, CMYK converted
/// naively: good enough to tell black from white. `None` for any other number of components.
fn rgb(v: &[f64]) -> Option<[f64; 3]> {
    match *v {
        [g] => Some([g.clamp(0.0, 1.0); 3]),
        [r, g, b] => Some([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]),
        [c, m, y, k] => {
            let (c, m, y, k) = (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k.clamp(0.0, 1.0));
            Some([(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)])
        }
        _ => None,
    }
}

/// The components of the color space `cs` names when the audit reads its colors as gray (1), RGB
/// (3) or CMYK (4): a device space, or one of `resources` that is calibrated or ICC-based. 0 for any
/// other (a pattern, a spot color, an indexed or Lab space).
fn components(doc: &Document, resources: &Dict, name: &[u8]) -> usize {
    let device = |name: &[u8]| match name {
        b"DeviceGray" => 1,
        b"DeviceRGB" => 3,
        b"DeviceCMYK" => 4,
        _ => 0,
    };
    if device(name) > 0 {
        return device(name);
    }
    let spaces = resources.get(b"ColorSpace").map(|o| doc.resolve(o));
    let Some(space) = spaces.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)).map(|o| doc.resolve(o)) else { return 0 };
    let Some(family) = space.as_array() else { return space.as_name().map_or(0, device) };
    let part = |i: usize| family.get(i).map(|o| doc.resolve(o));
    match part(0).as_deref().and_then(Object::as_name) {
        Some(b"CalGray") => 1,
        Some(b"CalRGB") => 3,
        Some(b"ICCBased") => {
            let n = part(1).as_deref().and_then(|s| s.as_dict()?.get(b"N").map(|o| doc.resolve(o))).and_then(|n| n.as_int());
            n.filter(|n| matches!(n, 1 | 3 | 4)).map_or(0, |n| n as usize)
        }
        _ => 0,
    }
}

/// A form's `/Matrix`, or the identity when it has none that is usable.
pub(crate) fn form_matrix(doc: &Document, dict: &Dict) -> Matrix {
    dict.get(b"Matrix")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_array().map(|a| a.to_vec()))
        .and_then(|a| {
            let v: Vec<f64> = a.iter().filter_map(|o| doc.resolve(o).as_f64()).collect();
            (v.len() == 6 && v.iter().all(|x| x.is_finite())).then(|| Matrix([v[0], v[1], v[2], v[3], v[4], v[5]]))
        })
        .unwrap_or(Matrix::IDENTITY)
}

/// The `x y` operands of `m`/`l`, mapped to user space.
fn user_point(gs: &Gs, op: &Op) -> Option<[f64; 2]> {
    let [x, y] = op.nums::<2>().filter(|v| v.iter().all(|n| n.is_finite()))?;
    let (x, y) = gs.ctm.apply(x, y);
    Some([x, y])
}

/// The non-stroking alpha (`/ca`) of a named ExtGState, defaulting to opaque.
fn ext_alpha(doc: &Document, resources: &Dict, name: &[u8]) -> f64 {
    // Borrowed, not copied: a `gs` must not cost the size of the whole ExtGState table.
    let states = resources.get(b"ExtGState").map(|o| doc.resolve(o));
    let state = states.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)).map(|o| doc.resolve(o));
    let alpha = state.as_deref().and_then(Object::as_dict).and_then(|d| d.get(b"ca").map(|o| doc.resolve(o)).and_then(|o| o.as_f64()));
    alpha.filter(|a| a.is_finite()).map(|a| a.clamp(0.0, 1.0)).unwrap_or(1.0)
}

/// Every opaque filled rectangle drawn by the content of page `n` (and its form XObjects), then by
/// its annotations' `appearances`, with paint positions. A page's content streams are walked once,
/// joined, with one graphics state, so state set in an earlier stream is still in force in the
/// later ones. Any content stream that cannot be decoded, content, forms and appearances that
/// decode or enter past one page's [`DecodeBudget`] in all, or text in a glyph procedure or
/// pattern cell is an audit failure, never an empty result. `crop` is the page's crop box, the clip
/// its content and appearances start with.
pub fn filled_rects(doc: &Document, page: &Dict, crop: [f64; 4], n: usize, appearances: &[&Appearance]) -> Result<Vec<FilledRect>, AuditError> {
    let resources = page.get(b"Resources").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let contents: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut budget = DecodeBudget::page();
    let mut streams = Vec::with_capacity(contents.len());
    for c in &contents {
        if let Object::Stream(s) = &*doc.resolve(c) {
            streams.push(budget.decode_strict(s)?);
        }
    }
    let pieces = Pieces::join(&streams);
    let mut w = Walker { doc, out: Vec::new(), ops_seen: 0, budget, page: n, layer: 0, inside: None, walked: HashSet::new() };
    let mut gs = Gs::new(crop);
    let mut stack = Vec::new();
    // Read one operator at a time: the joined stream can hold millions of them within the
    // decode budget, and the walk only ever needs the current one.
    let page = Res { dict: &resources, owner: Owner::Page };
    w.walk_ops(ops(pieces.data()), &mut gs, &mut stack, page, &mut Vec::new())?;
    for a in appearances {
        let Object::Stream(form) = &*a.form else { return Err(AuditError::BadAppearance(n)) };
        if !w.budget.enter_form() {
            return Err(AuditError::TooManyForms);
        }
        let data = w.budget.decode_strict(form)?;
        // An appearance without resources of its own reads the page's, as a form does.
        let own = form.dict.get(b"Resources").map(|o| (o, doc.resolve(o)));
        w.layer = a.layer;
        // A viewer clips the appearance to its box, as a form's.
        let clip = form_bbox(doc, &form.dict).map_or(crop, |bbox| meet(crop, a.ctm.bbox(bbox)));
        let mut gs = Gs { ctm: a.ctm, ..Gs::new(clip) };
        w.walk_ops(ops(&data), &mut gs, &mut Vec::new(), own_resources(own.as_ref(), a.contents.as_ref()).unwrap_or(page), &mut Vec::new())?;
    }
    Ok(w.out)
}
