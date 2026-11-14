//! Walk page content streams and collect the opaque filled rectangles they draw.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. A single closed `m`/`l` subpath
//! whose four corners form an axis-aligned rectangle in user space is the same shape and counts
//! too (#1510). Graphics state tracked here is deliberately small: the CTM stack (`q`/`Q`/`cm`),
//! the non-stroking color (`g`/`rg`/`k`) and the non-stroking alpha from ExtGState (`gs` reading
//! `/ca`). Form XObjects are followed read-only with a depth cap, and so are the appearances a
//! viewer paints for the page's annotations. Anything fancier (other filled paths, shadings,
//! patterns) is out of scope and documented as such.
//!
//! The walk also reads the glyph procedures of the Type 3 fonts the content selects and the cells
//! of the tiling patterns it paints with. Placing what those draw would mean painting each glyph
//! or tile where it lands, so the audit reads no text there: one that shows text fails it.
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
    /// The fill color as sRGB-ish `[r, g, b]` (CMYK is converted approximately).
    pub rgb: [f64; 3],
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
    fill: [f64; 3],
    alpha: f64,
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

impl Default for Gs {
    fn default() -> Self {
        Gs { ctm: Matrix::IDENTITY, fill: [0.0, 0.0, 0.0], alpha: 1.0 }
    }
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
    /// The fonts and patterns already looked at on this page: a Type 3 font selected on every
    /// line is walked once.
    walked: HashSet<ObjRef>,
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
        resources: &Dict,
        calls: &mut Vec<usize>,
    ) -> Result<(), AuditError> {
        let mut path = Path::Empty;
        for (i, op) in ops.enumerate() {
            self.bump()?;
            let op = &op;
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
                b"g" => {
                    if let Some(v) = op.num(0).filter(|v| v.is_finite()) {
                        let v = v.clamp(0.0, 1.0);
                        gs.fill = [v, v, v];
                    }
                }
                b"rg" => {
                    if let Some([r, g, b]) = op.nums::<3>()
                        && [r, g, b].iter().all(|v| v.is_finite())
                    {
                        gs.fill = [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)];
                    }
                }
                b"k" => {
                    if let Some([c, m, y, k]) = op.nums::<4>()
                        && [c, m, y, k].iter().all(|v| v.is_finite())
                    {
                        // Naive CMYK -> RGB; good enough to tell black from white.
                        let (c, m, y, k) = (c.clamp(0.0, 1.0), m.clamp(0.0, 1.0), y.clamp(0.0, 1.0), k.clamp(0.0, 1.0));
                        gs.fill = [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)];
                    }
                }
                b"gs" => {
                    if let Some(name) = op.name(0) {
                        gs.alpha = ext_alpha(self.doc, resources, name);
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
                        let pos = std::iter::once(self.layer).chain(calls.iter().copied()).chain(std::iter::once(i)).collect();
                        self.out.push(FilledRect { rect: r, rgb: gs.fill, pos });
                    }
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if let Some(unplaced) = self.inside {
                        return Err(AuditError::UncheckedText(self.page, unplaced.what()));
                    }
                }
                b"Tf" => {
                    if let Some(name) = op.name(0) {
                        self.walk_type3(name, i, resources, calls)?;
                    }
                }
                b"scn" | b"SCN" => {
                    if let Some(name) = op.operands.last().and_then(Object::as_name) {
                        self.walk_pattern(name, i, resources, calls)?;
                    }
                }
                b"S" | b"s" | b"n" => path = Path::Empty,
                b"c" | b"v" | b"y" | b"W" | b"W*" => path = Path::Other,
                b"Do" => {
                    if let Some(name) = op.name(0) {
                        if calls.len() >= MAX_DEPTH {
                            return Err(AuditError::TooDeep);
                        }
                        calls.push(i);
                        self.walk_xobject(name, gs, resources, calls)?;
                        calls.pop();
                    }
                    path = Path::Empty;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Follow a form XObject read-only, composing its matrix onto the current CTM. State
    /// changes inside never leak out. `calls` ends with the `Do` that draws it.
    fn walk_xobject(&mut self, name: &[u8], gs: &Gs, resources: &Dict, calls: &mut Vec<usize>) -> Result<(), AuditError> {
        // Borrowed, not copied: a `Do` must not cost the size of the whole XObject table.
        let xobjects = resources.get(b"XObject").map(|o| self.doc.resolve(o));
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
        let sub_resources = dict.get(b"Resources").map(|o| self.doc.resolve(o));
        let sub_resources = sub_resources.as_deref().and_then(Object::as_dict).unwrap_or(resources);
        if !self.budget.enter_form() {
            return Err(AuditError::TooManyForms);
        }
        // A form stream that cannot be decoded fails the audit: it might hide a cover.
        let data = self.budget.decode_strict(stream)?;
        // The XObject matrix maps form space into the user space active at `Do`.
        let mut sub_gs = gs.clone();
        sub_gs.ctm = matrix.then(&gs.ctm);
        let mut sub_stack = Vec::new();
        self.walk_ops(ops(&data), &mut sub_gs, &mut sub_stack, sub_resources, calls)
    }

    /// Walk the glyph procedures of the Type 3 font `Tf` selects at index `at` as unplaced content,
    /// once a page.
    fn walk_type3(&mut self, name: &[u8], at: usize, resources: &Dict, calls: &mut Vec<usize>) -> Result<(), AuditError> {
        let fonts = resources.get(b"Font").map(|o| self.doc.resolve(o));
        let Some(entry) = fonts.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)) else { return Ok(()) };
        if entry.as_ref().is_some_and(|r| !self.walked.insert(r)) {
            return Ok(());
        }
        let font = self.doc.resolve(entry);
        let Some(font) = font.as_dict() else { return Ok(()) };
        if crate::subtype(self.doc, font)?.as_deref() != Some(b"Type3") {
            return Ok(());
        }
        // shortcut: a font without `/Resources` reads those where it is first selected on the page,
        // and is not walked again with others. Key `walked` by those resources too if a glyph's text
        // depends on them.
        let own = font.get(b"Resources").map(|o| self.doc.resolve(o));
        let glyph_resources = own.as_deref().and_then(Object::as_dict).unwrap_or(resources);
        let procs = font.get(b"CharProcs").map(|o| self.doc.resolve(o));
        for (_, glyph) in procs.as_deref().and_then(Object::as_dict).into_iter().flat_map(Dict::iter) {
            // Each entry looked at counts as an operator: fonts sharing one large table of entries
            // that aren't streams would otherwise be read past every bound.
            self.bump()?;
            if let Object::Stream(s) = &*self.doc.resolve(glyph) {
                self.walk_unplaced(Unplaced::Glyph, s, at, glyph_resources, calls)?;
            }
        }
        Ok(())
    }

    /// Walk the cell of the tiling pattern `scn` paints with at index `at` as unplaced content,
    /// once a page. Shading patterns are dictionaries and draw no text.
    fn walk_pattern(&mut self, name: &[u8], at: usize, resources: &Dict, calls: &mut Vec<usize>) -> Result<(), AuditError> {
        let patterns = resources.get(b"Pattern").map(|o| self.doc.resolve(o));
        let Some(entry) = patterns.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)) else { return Ok(()) };
        if entry.as_ref().is_some_and(|r| !self.walked.insert(r)) {
            return Ok(());
        }
        let pattern = self.doc.resolve(entry);
        let Object::Stream(s) = &*pattern else { return Ok(()) };
        let own = s.dict.get(b"Resources").map(|o| self.doc.resolve(o));
        self.walk_unplaced(Unplaced::Pattern, s, at, own.as_deref().and_then(Object::as_dict).unwrap_or(resources), calls)
    }

    /// Walk `stream`, reached from the operator at index `at`, as `unplaced` content: forms it
    /// draws count towards the depth and the page's budget like any other.
    fn walk_unplaced(&mut self, unplaced: Unplaced, stream: &Stream, at: usize, resources: &Dict, calls: &mut Vec<usize>) -> Result<(), AuditError> {
        if calls.len() >= MAX_DEPTH {
            return Err(AuditError::TooDeep);
        }
        if !self.budget.enter_form() {
            return Err(AuditError::TooManyForms);
        }
        let data = self.budget.decode_strict(stream)?;
        let outer = self.inside.replace(unplaced);
        calls.push(at);
        self.walk_ops(ops(&data), &mut Gs::default(), &mut Vec::new(), resources, calls)?;
        calls.pop();
        self.inside = outer;
        Ok(())
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
/// pattern cell is an audit failure, never an empty result.
pub fn filled_rects(doc: &Document, page: &Dict, n: usize, appearances: &[&Appearance]) -> Result<Vec<FilledRect>, AuditError> {
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
    let mut gs = Gs::default();
    let mut stack = Vec::new();
    // Read one operator at a time: the joined stream can hold millions of them within the
    // decode budget, and the walk only ever needs the current one.
    w.walk_ops(ops(pieces.data()), &mut gs, &mut stack, &resources, &mut Vec::new())?;
    for a in appearances {
        let Object::Stream(form) = &*a.form else { return Err(AuditError::BadAppearance(n)) };
        if !w.budget.enter_form() {
            return Err(AuditError::TooManyForms);
        }
        let data = w.budget.decode_strict(form)?;
        // An appearance without resources of its own reads the page's, as a form does.
        let own = form.dict.get(b"Resources").map(|o| doc.resolve(o));
        w.layer = a.layer;
        let mut gs = Gs { ctm: a.ctm, ..Gs::default() };
        w.walk_ops(ops(&data), &mut gs, &mut Vec::new(), own.as_deref().and_then(Object::as_dict).unwrap_or(&resources), &mut Vec::new())?;
    }
    Ok(w.out)
}
