//! Walk page content streams and collect the opaque filled rectangles they draw.
//!
//! This is the geometric half of the faux-redaction audit: a rectangle filled with an opaque
//! color (`re` + `f`) is the classic way a "redaction" is faked. A single closed `m`/`l` subpath
//! whose four corners form an axis-aligned rectangle in user space is the same shape and counts
//! too (#1510), as do several `re` rectangles filled together by the nonzero rule when all run one
//! way, each a box of its own; and so does a shading painted with `sh`, over the box that holds its
//! clip and within its `/BBox`, and an image without a mask of its own, over the unit square the
//! CTM places it on, within that box. A straight stroke counts too: one subpath whose points,
//! control points included, lie on one horizontal or vertical line on the page, stroked solid (no
//! dashes) under a CTM that keeps the axes, covers the band its line width paints between its
//! ends and past them under a round or square cap, painted in the stroking color and alpha, whether stroked alone
//! (`S`, `s`) or over a fill (`B`, `B*`, `b`, `b*`). Each counts only where what it paints lands
//! on a box on the page, as a rectangle, a `/BBox` or an image's square sheared by the CTM does
//! not, and only under a clip that is a box on the page: a triangle or a sheared box may clip it
//! away from what it seems to cover. Graphics state tracked
//! here is deliberately small: the CTM stack (`q`/`Q`/`cm`), the box holding the clip (`W` paths, a form's `/BBox`, the
//! page's crop box), the non-stroking color (`g`/`rg`/`k`, and
//! `cs` with `sc`/`scn` in gray, RGB and CMYK spaces; any other, a pattern's or a spot color's, is
//! one the audit can't read and counts as dark as the text) and the stroking one likewise
//! (`G`/`RG`/`K`, `CS` with `SC`/`SCN`), the line width (`w`) and whether a dash pattern is set
//! (`d`), and from ExtGState (`gs`) the non-stroking and stroking alpha (`/ca`, `/CA`), the line
//! width (`/LW`), the dash pattern (`/D`), the blend mode (`/BM`) and whether a soft mask (`/SMask`)
//! is set. A fully transparent fill paints no cover. One painted fully opaque in the Normal blend mode without
//! a soft mask is opaque, and hides text whatever its colour. A translucent one in the Normal blend
//! mode keeps its alpha, so the audit can mix it with the text and what lies around the text. A
//! blended one, or one painted through a soft mask, counts as a cover as dark as it is: how much it
//! hides depends on colours and a mask the audit doesn't read, and so does one in a transparency
//! group (a form with `/Group`) drawn translucent, blended or masked, whatever its own opacity. A
//! plain form's content is painted as the page's own. Form XObjects are followed read-only with a depth cap, and so
//! are the appearances a viewer paints for the page's annotations. Other filled paths and other
//! strokes are out of scope and documented as such; the layers whose content fills one are
//! reported.
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
/// Most graphics states saved at once (`q` not yet restored) on a page, its forms included: the
/// renderer's own limit, past which it stops saving them. Each saved state is a copy, so a run of
/// `q` within the operator bound would otherwise hold millions. Hitting it fails the audit.
const MAX_SAVED: usize = 4096;

/// One filled rectangle, in page user space.
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
    /// Painted fully opaque, in the Normal blend mode and without a soft mask, and not inside a
    /// transparency group drawn otherwise: it hides what lies under it whatever its colour (ISO
    /// 32000-2 §11.3.5).
    pub opaque: bool,
    /// The alpha it is painted with, when that alone decides how much of what lies under it shows:
    /// in the Normal blend mode without a soft mask. `None` when blended or masked, or inside a
    /// transparency group drawn translucent, blended or masked.
    pub alpha: Option<f64>,
    /// Painted by an image.
    pub image: bool,
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
    /// The stroking color, its color space's components and alpha (`/CA`), as `fill`, `space` and
    /// `alpha` give the non-stroking ones.
    stroke: Option<[f64; 3]>,
    stroke_space: usize,
    stroke_alpha: f64,
    /// The line width (`w`, `/LW`), in user space.
    line_width: f64,
    /// The line cap (`J`, `/LC`) is round or projecting square: a stroke paints half its width past
    /// each end.
    capped: bool,
    /// A dash pattern (`d`, `/D`) other than a solid line is in force.
    dashed: bool,
    /// A blend mode other than Normal (`/BM`) is in force: what lies under a fill can show through.
    blended: bool,
    /// A soft mask (`/SMask`) is in force: the fill can show what lies under it.
    masked: bool,
    /// Inside a transparency group drawn translucent, blended or through a soft mask: the group is
    /// composited with that state, so a fill here shows with an alpha the audit doesn't work out,
    /// whatever opacity it sets (ISO 32000-2 §11.6.6). A plain form's content is painted with the
    /// state at `Do`, as the page's own would be.
    grouped: bool,
    /// A box in page user space that holds the clip.
    clip: [f64; 4],
    /// The clip is that box exactly: every clip set was a box on the page (a path of one `re` or one
    /// four-cornered subpath, or a `/BBox`, whose corners land on a box), and no text was shown in a
    /// clipping render mode. Only then can what is painted within `clip` be taken as what shows.
    clip_exact: bool,
    /// The text render mode (`Tr`) is one that adds the glyphs' outlines to the clip (4 to 7).
    text_clip: bool,
    /// The kind of the font text is shown in, set by `Tf` or an ExtGState's `/Font`.
    font: FontKind,
}

/// What a font's glyphs add to the clip in a clipping render mode (ISO 32000-1 §9.3.6).
#[derive(Clone, Copy)]
enum FontKind {
    /// A simple font (`Type1`, `MMType1` or `TrueType`) whose encoding leaves code 20h the space,
    /// which has no outline.
    Spaced,
    /// Any byte may draw a glyph: a Type 0 font, a simple font that gives 20h another glyph, a font
    /// of another or no subtype, or one not selected or found, which a viewer replaces with one of
    /// its own.
    Drawn,
    /// A Type 3 font: only mode 3 affects its glyphs, so it adds nothing.
    Type3,
}

impl FontKind {
    /// Whether the text-showing operator `op` shows a glyph in this font that adds to the clip: a
    /// string, or a string in a `TJ` array, with a byte other than a space in a [`FontKind::Spaced`]
    /// font, or any byte in a [`FontKind::Drawn`] one. Another glyph without an outline is taken to
    /// clip.
    fn clips(self, op: &Op) -> bool {
        let shows = |s: &Object| {
            s.as_string().is_some_and(|s| match self {
                FontKind::Spaced => s.bytes.iter().any(|&b| b != b' '),
                FontKind::Drawn => !s.bytes.is_empty(),
                FontKind::Type3 => false,
            })
        };
        match op.back(0) {
            Some(Object::Array(parts)) => parts.iter().any(shows),
            Some(last) => shows(last),
            None => false,
        }
    }
}

impl Gs {
    fn new(clip: [f64; 4]) -> Self {
        Gs {
            ctm: Matrix::IDENTITY,
            fill: Some([0.0; 3]),
            space: 1,
            alpha: 1.0,
            stroke: Some([0.0; 3]),
            stroke_space: 1,
            stroke_alpha: 1.0,
            line_width: 1.0,
            capped: false,
            dashed: false,
            blended: false,
            masked: false,
            grouped: false,
            clip,
            clip_exact: true,
            text_clip: false,
            font: FontKind::Drawn,
        }
    }
}

/// The path under construction, as far as the audit cares about it.
enum Path {
    Empty,
    /// `re` rectangles, a subpath each: each one's box in user space, whether it lands on that box
    /// (the CTM keeps its edges horizontal and vertical), and whether it runs anticlockwise there.
    Rects(Vec<Rect>),
    /// One `m`/`l` subpath, corners already mapped to user space; `true` once closed by `h`.
    Lines(Vec<[f64; 2]>, bool),
    /// Anything else: curves, a second subpath that is not a `re`, a CTM change mid-path.
    Other,
}

/// A `re` rectangle as [`Path::Rects`] holds it.
#[derive(Clone, Copy)]
struct Rect {
    area: [f64; 4],
    boxed: bool,
    anticlockwise: bool,
}

impl Path {
    /// The boxes a fill of this path paints whole, by the even-odd rule or the nonzero one, if it
    /// paints nothing else. Rectangles that each land on a box and that all run one way, filled by
    /// the nonzero rule, paint each box whole; otherwise one can cut a hole in another.
    fn boxes(self, even_odd: bool) -> Option<Vec<[f64; 4]>> {
        match self {
            Path::Rects(rects)
                if rects.iter().all(|r| r.boxed)
                    && (rects.len() == 1 || (!even_odd && rects.iter().all(|r| r.anticlockwise == rects[0].anticlockwise))) =>
            {
                Some(rects.iter().map(|r| r.area).collect())
            }
            Path::Lines(pts, _) => lines_rect(&pts).map(|r| vec![r]),
            Path::Rects(_) | Path::Empty | Path::Other => None,
        }
    }
}

/// The path under construction as a stroke paints it, as far as the audit reads it: one subpath
/// along one horizontal or vertical line on the page.
enum Line {
    Empty,
    /// The box of the points where its segments start and end, and the box of all its points,
    /// control points included, in user space.
    Straight {
        ends: [f64; 4],
        all: [f64; 4],
    },
    /// Anything else: a second subpath, a rectangle, a point that isn't a number.
    Other,
}

impl Line {
    /// `self` after the path operator `op`, its points mapped by `ctm`. Other operators leave it.
    fn then(self, op: &Op, ctm: &Matrix) -> Line {
        let points = match op.op.as_slice() {
            b"m" | b"l" => 1,
            b"v" | b"y" => 2,
            b"c" => 3,
            b"re" => return Line::Other,
            _ => return self,
        };
        let mapped: Option<Vec<[f64; 4]>> = op.operands[op.operands.len().saturating_sub(2 * points)..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|[x, y]| {
                let (x, y) = (x.as_f64().filter(|v| v.is_finite())?, y.as_f64().filter(|v| v.is_finite())?);
                let (x, y) = ctm.apply(x, y);
                Some([x, y, x, y])
            })
            .collect();
        // The last point ends the segment; those before it are control points.
        let Some((&end, _)) = mapped.as_deref().filter(|m| m.len() == points).and_then(<[_]>::split_last) else { return Line::Other };
        match (self, op.op.as_slice()) {
            (Line::Empty, b"m") => Line::Straight { ends: end, all: end },
            (Line::Straight { ends, all }, b"l" | b"c" | b"v" | b"y") => {
                Line::Straight { ends: join(Some(ends), end), all: mapped.into_iter().flatten().fold(all, |a, p| join(Some(a), p)) }
            }
            _ => Line::Other,
        }
    }

    /// The band a stroke of this path paints in `gs`: between its ends, and past them by half the
    /// line width under a round or projecting square cap (a round cap counted by its box), as wide
    /// as the line, when its points all lie on one horizontal or vertical line and the CTM keeps
    /// the axes, so the band is a box on the page. A curve whose control points lie on that
    /// line too stays on it, and passes every point between its ends. `None` for a dashed line, a
    /// line of width 0 (the thinnest a device draws), or any other path.
    fn band(&self, gs: &Gs) -> Option<[f64; 4]> {
        // CTM products leave rounding noise, so points compare with a tolerance (in points).
        const EPS: f64 = 1e-6;
        let Line::Straight { ends, all } = self else { return None };
        let [a, b, c, d, ..] = gs.ctm.0;
        let keeps_axes = (b.abs() <= EPS && c.abs() <= EPS) || (a.abs() <= EPS && d.abs() <= EPS);
        if !keeps_axes || gs.dashed || gs.line_width <= 0.0 {
            return None;
        }
        // The pen is a circle of the line's width in user space; the CTM stretches it by the
        // length of the image of a unit step across the line.
        let half = gs.line_width / 2.0;
        // The pen's extent along the line, as across it.
        let cap = if gs.capped { half } else { 0.0 };
        if all[3] - all[1] <= EPS && ends[2] - ends[0] > EPS {
            let (h, l) = (half * b.hypot(d), cap * a.hypot(c));
            Some([ends[0] - l, ends[1] - h, ends[2] + l, ends[1] + h])
        } else if all[2] - all[0] <= EPS && ends[3] - ends[1] > EPS {
            let (w, l) = (half * a.hypot(c), cap * b.hypot(d));
            Some([ends[0] - w, ends[1] - l, ends[0] + w, ends[3] + l])
        } else {
            None
        }
    }
}

/// Whether the box `r` (`[x0, y0, x1, y1]`) lands on a box on the page under `ctm`: its corners,
/// mapped, are those of an axis-aligned rectangle.
fn box_on_page(ctm: &Matrix, r: [f64; 4]) -> bool {
    let corner = |x, y| {
        let (x, y) = ctm.apply(x, y);
        [x, y]
    };
    lines_rect(&[corner(r[0], r[1]), corner(r[2], r[1]), corner(r[2], r[3]), corner(r[0], r[3])]).is_some()
}

/// The bounding box of a closed polygon that is an axis-aligned rectangle, or `None`. A repeated
/// start point is dropped first; the remaining four corners must alternate horizontal and
/// vertical edges.
fn lines_rect(pts: &[[f64; 2]]) -> Option<[f64; 4]> {
    // CTM products leave rounding noise, so corners compare with a tolerance (in points).
    const EPS: f64 = 1e-6;
    let same = |a: f64, b: f64| (a - b).abs() <= EPS;
    let pts = if pts.len() == 5 && same(pts[0][0], pts[4][0]) && same(pts[0][1], pts[4][1]) { &pts[..4] } else { pts };
    let [a, b, c, d] = *<&[[f64; 2]; 4]>::try_from(pts).ok()?;
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
    /// Graphics states saved and not yet restored, in every stack being walked.
    saved: usize,
    /// The layers that fill a shape other than a box (a cloudy border drawn with curves), or paint
    /// under a clip that is no box: the audit reads no cover from it.
    shaped: HashSet<usize>,
    /// Something was painted on the page whose colours the audit doesn't read as what lies under
    /// text: a stroke not read as a band, or an image with a mask of its own.
    unread: bool,
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
        let mut line = Line::Empty;
        // The box holding every point of the path, in user space (a curve lies within its control
        // points), how many subpaths it has, and, once `W` makes the path the clip when it is
        // painted, whether that path is the box.
        let mut path_box: Option<[f64; 4]> = None;
        let mut subpaths = 0;
        let mut clipping: Option<bool> = None;
        // Text shown since `BT` in a clipping render mode: at `ET` its glyphs become part of the clip.
        let mut glyph_clip = false;
        for (i, op) in ops.enumerate() {
            self.bump()?;
            let op = &op;
            line = line.then(op, &gs.ctm);
            if matches!(op.op.as_slice(), b"m" | b"re") {
                subpaths += 1;
            }
            match op.op.as_slice() {
                b"m" | b"l" | b"c" | b"v" | b"y" => {
                    for p in op.operands.as_chunks::<2>().0 {
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
                b"q" => {
                    if self.saved >= MAX_SAVED {
                        return Err(AuditError::TooManyStates);
                    }
                    self.saved += 1;
                    stack.push(gs.clone());
                }
                b"Q" => {
                    // The current path is not graphics state (ISO 32000-2 §8.4.1): `re Q f` still
                    // fills the rectangle, whose corners are already in user space.
                    if let Some(g) = stack.pop() {
                        self.saved = self.saved.saturating_sub(1);
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
                    if gs.ctm != before && !matches!(line, Line::Empty) {
                        line = Line::Other;
                    }
                }
                b"g" | b"rg" | b"k" | b"G" | b"RG" | b"K" | b"cs" | b"CS" | b"sc" | b"scn" | b"SC" | b"SCN" | b"w" | b"J" | b"d" | b"gs" => {
                    self.set_state(op, gs, res, calls, i)?;
                }
                b"re" => {
                    let rect = op.nums::<4>().and_then(|[x, y, w, h]| {
                        if [x, y, w, h].iter().all(|v| v.is_finite()) && w != 0.0 && h != 0.0 {
                            let user = [x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)];
                            let r = gs.ctm.bbox(user);
                            // `re` runs anticlockwise when w and h share a sign; a CTM that mirrors
                            // turns it the other way.
                            let [a, b, c, d, ..] = gs.ctm.0;
                            let anticlockwise = (w * h > 0.0) == (a * d - b * c > 0.0);
                            (r[2] > r[0] && r[3] > r[1]).then(|| Rect { area: r, boxed: box_on_page(&gs.ctm, user), anticlockwise })
                        } else {
                            None
                        }
                    });
                    path = match (path, rect) {
                        (Path::Empty, Some(rect)) => Path::Rects(vec![rect]),
                        (Path::Rects(mut rects), Some(rect)) => {
                            rects.push(rect);
                            Path::Rects(rects)
                        }
                        _ => Path::Other,
                    };
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
                        rects @ Path::Rects(_) => rects,
                        _ => Path::Other,
                    };
                }
                // `b`/`b*` close the path before filling it, which the fill does anyway.
                b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                    self.fill(std::mem::replace(&mut path, Path::Empty), op.op.ends_with(b"*"), gs, calls, i);
                    // `B` and `b` stroke the path over the fill.
                    if matches!(op.op.as_slice(), b"B" | b"B*" | b"b" | b"b*") {
                        self.stroke(std::mem::replace(&mut line, Line::Empty), gs, calls, i);
                    }
                }
                // A shading paints everywhere the clip lets it, within its own `/BBox` if it has one.
                b"sh" => {
                    let shadings = resources.get(b"Shading").map(|o| self.doc.resolve(o));
                    let shading = op.name(0).and_then(|name| shadings.as_deref().and_then(Object::as_dict)?.get(name)).map(|o| self.doc.resolve(o));
                    let bbox = shading.as_deref().and_then(Object::as_dict).and_then(|d| form_bbox(self.doc, d));
                    let area = bbox.map_or(gs.clip, |b| meet(gs.clip, gs.ctm.bbox(b)));
                    if self.covers_box(area, bbox.is_none_or(|b| box_on_page(&gs.ctm, b)), gs) {
                        self.paint(area, None, gs, false, calls, i);
                    }
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if let Some(unplaced) = self.inside {
                        return Err(AuditError::UncheckedText(self.page, unplaced.what()));
                    }
                    glyph_clip |= gs.text_clip && gs.font.clips(op);
                }
                b"Tr" => gs.text_clip = op.nums::<1>().map_or(gs.text_clip, |[mode]| (4.0..=7.0).contains(&mode)),
                b"BT" => glyph_clip = false,
                // The glyphs' outlines are no box, so what is painted after may not show where its box
                // says. With none shown nothing is clipped.
                b"ET" => gs.clip_exact &= !glyph_clip,
                b"Tf" => {
                    let fonts = resources.get(b"Font").map(|o| self.doc.resolve(o));
                    gs.font = match op.name(0).and_then(|name| fonts.as_deref().and_then(Object::as_dict)?.get(name)) {
                        Some(entry) => self.walk_type3(entry, i, res, calls)?,
                        None => FontKind::Drawn,
                    };
                }
                b"S" | b"s" => {
                    path = Path::Empty;
                    self.stroke(std::mem::replace(&mut line, Line::Empty), gs, calls, i);
                }
                b"n" => path = Path::Empty,
                b"W" | b"W*" => {
                    // The clip is the box exactly when the path is one subpath that is a box on the
                    // page: a second could cut a hole in it.
                    let boxed = match &path {
                        Path::Rects(rects) => rects.iter().all(|r| r.boxed),
                        Path::Lines(pts, _) => lines_rect(pts).is_some(),
                        Path::Empty | Path::Other => false,
                    };
                    clipping = Some(boxed && subpaths == 1);
                    path = Path::Other;
                }
                b"c" | b"v" | b"y" => path = Path::Other,
                b"Do" => {
                    if let Some(name) = op.name(0) {
                        self.walk_xobject(name, gs, res, calls, i)?;
                    }
                    path = Path::Empty;
                    line = Line::Empty;
                }
                b"BI" => {
                    if let Some((image, _)) = &op.inline {
                        self.paint_image(image, gs, calls, i);
                    }
                }
                _ => {}
            }
            // A path ends where it is painted, and after `W` it is the clip from then on.
            if matches!(op.op.as_slice(), b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" | b"n") {
                if let Some(exact) = clipping.take()
                    && let Some(b) = path_box
                {
                    gs.clip = meet(gs.clip, b);
                    gs.clip_exact &= exact;
                }
                path_box = None;
                subpaths = 0;
                line = Line::Empty;
            }
        }
        Ok(())
    }

    /// Apply the graphics state operator `op` (index `i`): colours, colour spaces, line width,
    /// dash and `gs`, walking any pattern or `gs` font it names.
    fn set_state(&mut self, op: &Op, gs: &mut Gs, res: Res<'_>, calls: &mut Vec<usize>, i: usize) -> Result<(), AuditError> {
        let resources = res.dict;
        match op.op.as_slice() {
            name @ (b"g" | b"rg" | b"k" | b"G" | b"RG" | b"K") => {
                let n = match name {
                    b"g" | b"G" => 1,
                    b"rg" | b"RG" => 3,
                    _ => 4,
                };
                if let Some(color) = op.operands.len().checked_sub(n).and_then(|k| numbers(&op.operands[k..])).and_then(|v| rgb(&v)) {
                    if name[0].is_ascii_lowercase() {
                        (gs.fill, gs.space) = (Some(color), n);
                    } else {
                        (gs.stroke, gs.stroke_space) = (Some(color), n);
                    }
                }
            }
            b"cs" => (gs.space, gs.fill) = color_space(self.doc, resources, op.name(0).unwrap_or_default()),
            b"CS" => (gs.stroke_space, gs.stroke) = color_space(self.doc, resources, op.name(0).unwrap_or_default()),
            name @ (b"sc" | b"scn" | b"SC" | b"SCN") => {
                if name[0] == b's' {
                    gs.fill = numbers(&op.operands).filter(|v| v.len() == gs.space).and_then(|v| rgb(&v));
                } else {
                    gs.stroke = numbers(&op.operands).filter(|v| v.len() == gs.stroke_space).and_then(|v| rgb(&v));
                }
                if let Some(name) = op.operands.last().and_then(Object::as_name) {
                    self.walk_pattern(name, i, res, calls)?;
                }
            }
            b"w" => {
                if let Some([w]) = op.nums::<1>().filter(|[w]| w.is_finite() && *w >= 0.0) {
                    gs.line_width = w;
                }
            }
            b"J" => {
                if let Some([cap]) = op.nums::<1>().filter(|[cap]| [0.0, 1.0, 2.0].contains(cap)) {
                    gs.capped = cap != 0.0;
                }
            }
            b"d" => {
                if let Some(dashes) = op.operands.first().and_then(Object::as_array) {
                    gs.dashed = !dashes.is_empty();
                }
            }
            b"gs" => {
                if let Some(name) = op.name(0) {
                    // A state leaves what it doesn't set as it was (ISO 32000-2 §8.4.5).
                    let set = ext_state(self.doc, resources, name);
                    gs.alpha = set.alpha.unwrap_or(gs.alpha);
                    gs.stroke_alpha = set.stroke_alpha.unwrap_or(gs.stroke_alpha);
                    gs.line_width = set.line_width.unwrap_or(gs.line_width);
                    gs.capped = set.capped.unwrap_or(gs.capped);
                    gs.dashed = set.dashed.unwrap_or(gs.dashed);
                    gs.blended = set.blended.unwrap_or(gs.blended);
                    gs.masked = set.masked.unwrap_or(gs.masked);
                    gs.font = self.walk_state_font(name, i, res, calls)?.unwrap_or(gs.font);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Record the covers the fill at index `at` paints with `path` (by the even-odd rule or the
    /// nonzero one), within the clip, as [`Walker::paint_box`] does, or, for a shape the audit reads
    /// no box from, that this layer fills one.
    fn fill(&mut self, path: Path, even_odd: bool, gs: &Gs, calls: &[usize], at: usize) {
        if matches!(path, Path::Empty) {
            return;
        }
        match path.boxes(even_odd) {
            Some(boxes) => {
                for r in boxes {
                    let area = meet(gs.clip, r);
                    if self.covers_box(area, true, gs) {
                        self.paint(area, gs.fill, gs, false, calls, at);
                    }
                }
            }
            None if gs.alpha > 0.0 && self.inside.is_none() => {
                self.shaped.insert(self.layer);
            }
            None => {}
        }
    }

    /// Whether what is painted over `area`, within the box holding the clip, is a cover of all of
    /// it: what it paints lands on a box (`boxed`) and so does the clip ([`Gs::clip_exact`]). Under a
    /// triangle, a box with a hole or a sheared box, or for a sheared shading or image, it may paint
    /// only part of `area`, so it is no cover and this layer fills a shape the audit reads no box
    /// from.
    fn covers_box(&mut self, area: [f64; 4], boxed: bool, gs: &Gs) -> bool {
        if gs.alpha <= 0.0 || self.inside.is_some() || area[2] <= area[0] || area[3] <= area[1] {
            return false;
        }
        if !(boxed && gs.clip_exact) {
            self.shaped.insert(self.layer);
        }
        boxed && gs.clip_exact
    }

    /// Record the cover the stroke at index `at` paints along `line`: the band [`Line::band`] gives,
    /// within the clip, as a fill in the stroking colour and alpha would paint it. Under a clip that
    /// is not a box ([`Gs::clip_exact`]) the band may be clipped away from what it seems to cover, so
    /// none is read. A band read is recorded as a fill would be, so it leaves the lines' backdrop
    /// read; any other stroke paints where the audit doesn't read, and unreads it.
    fn stroke(&mut self, line: Line, gs: &Gs, calls: &[usize], at: usize) {
        let band = line.band(gs).filter(|_| gs.clip_exact);
        self.unread |= self.inside.is_none() && band.is_none();
        let Some(band) = band else { return };
        let area = meet(gs.clip, band);
        if gs.stroke_alpha > 0.0 && self.inside.is_none() && area[2] > area[0] && area[3] > area[1] {
            let pen = Gs { alpha: gs.stroke_alpha, ..gs.clone() };
            self.paint(area, gs.stroke, &pen, false, calls, at);
        }
    }

    /// Record a cover painted over `rect` in `rgb` by the operator at index `at`, with the graphics
    /// state `gs`.
    fn paint(&mut self, rect: [f64; 4], rgb: Option<[f64; 3]>, gs: &Gs, image: bool, calls: &[usize], at: usize) {
        let pos = std::iter::once(self.layer).chain(calls.iter().copied()).chain(std::iter::once(at)).collect();
        let opaque = gs.alpha >= 1.0 && !gs.blended && !gs.masked && !gs.grouped;
        let alpha = (!gs.blended && !gs.masked && !gs.grouped).then_some(gs.alpha);
        self.out.push(FilledRect { rect, rgb, pos, opaque, alpha, image });
    }

    /// Record the cover the image `image` (its dictionary) makes, painted by the operator at index
    /// `at`: the unit square the CTM places it on, within the clip (ISO 32000-2 §8.9.4). An image
    /// mask paints in the fill colour, another image in colours the audit doesn't read. An image with
    /// a mask of its own (`/SMask`, `/Mask`, or a JPEG 2000 image's `/SMaskInData`) is no cover: it
    /// lets what lies under it show where its mask says, which the audit doesn't read.
    fn paint_image(&mut self, image: &Dict, gs: &Gs, calls: &[usize], at: usize) {
        // An inline image may name its entries by their abbreviations (ISO 32000-2 §8.9.7).
        let get = |key: &[u8], short: &[u8]| image.get(key).or_else(|| image.get(short)).map(|o| self.doc.resolve(o));
        let stencil = get(b"ImageMask", b"IM").is_some_and(|o| matches!(*o, Object::Bool(true)));
        let alpha_in_data = image.get(b"SMaskInData").and_then(|o| self.doc.resolve(o).as_int()).is_some_and(|n| n != 0);
        // An entry that is null is no mask (ISO 32000-2 §7.3.9).
        let mask = |key: &[u8]| image.get(key).is_some_and(|o| !matches!(*self.doc.resolve(o), Object::Null));
        let own_mask = mask(b"SMask") || mask(b"Mask") || alpha_in_data;
        const UNIT: [f64; 4] = [0.0, 0.0, 1.0, 1.0];
        self.unread |= own_mask && self.inside.is_none();
        if !own_mask {
            let area = meet(gs.clip, gs.ctm.bbox(UNIT));
            if self.covers_box(area, box_on_page(&gs.ctm, UNIT), gs) {
                self.paint(area, if stencil { gs.fill } else { None }, gs, true, calls, at);
            }
        }
    }

    /// Follow the XObject `name` that the `Do` at index `at` draws: an image is a cover
    /// ([`Walker::paint_image`]); a form is followed read-only, composing its matrix onto the
    /// current CTM, and state changes inside never leak out.
    fn walk_xobject(&mut self, name: &[u8], gs: &Gs, res: Res<'_>, calls: &mut Vec<usize>, at: usize) -> Result<(), AuditError> {
        // Borrowed, not copied: a `Do` must not cost the size of the whole XObject table.
        let xobjects = res.dict.get(b"XObject").map(|o| self.doc.resolve(o));
        let Some(entry) = xobjects.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)) else { return Ok(()) };
        let resolved = self.doc.resolve(entry);
        let Object::Stream(stream) = &*resolved else { return Ok(()) };
        let dict = &stream.dict;
        match crate::subtype(self.doc, dict)?.as_deref() {
            Some(b"Form") => {}
            Some(b"Image") => {
                self.paint_image(dict, gs, calls, at);
                return Ok(());
            }
            _ => return Ok(()),
        }
        if calls.len() >= MAX_DEPTH {
            return Err(AuditError::TooDeep);
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
        let data = self.budget.decode_strict(self.doc, stream)?;
        // The XObject matrix maps form space into the user space active at `Do`.
        let mut sub_gs = gs.clone();
        sub_gs.ctm = matrix.then(&gs.ctm);
        // A `/Group` makes the form a transparency group (ISO 32000-2 §11.6.6), as the renderer reads it.
        let group = dict.get(b"Group").is_some_and(|o| self.doc.resolve(o).as_dict().is_some());
        sub_gs.grouped |= group && (gs.alpha < 1.0 || gs.blended || gs.masked);
        if let Some(bbox) = form_bbox(self.doc, dict) {
            sub_gs.clip = meet(sub_gs.clip, sub_gs.ctm.bbox(bbox));
            sub_gs.clip_exact &= box_on_page(&sub_gs.ctm, bbox);
        }
        calls.push(at);
        self.walk_nested(ops(&data), &mut sub_gs, sub, calls)?;
        calls.pop();
        Ok(())
    }

    /// Walk a stream of its own (a form, a glyph procedure, a pattern cell) with a stack of its
    /// own: the states it leaves saved are dropped with it.
    fn walk_nested(&mut self, ops: impl Iterator<Item = Op>, gs: &mut Gs, res: Res<'_>, calls: &mut Vec<usize>) -> Result<(), AuditError> {
        let mut stack = Vec::new();
        let walked = self.walk_ops(ops, gs, &mut stack, res, calls);
        self.saved = self.saved.saturating_sub(stack.len());
        walked
    }

    /// Walk the font an ExtGState's `/Font` sets (ISO 32000-2 §8.4.5) as [`Walker::walk_type3`]
    /// walks one `Tf` selects: the `gs` at index `at` names the ExtGState. Returns the font's kind, if
    /// the state sets one.
    fn walk_state_font(&mut self, name: &[u8], at: usize, res: Res<'_>, calls: &mut Vec<usize>) -> Result<Option<FontKind>, AuditError> {
        // Borrowed, not copied: a `gs` must not cost the size of the whole ExtGState table.
        let states = res.dict.get(b"ExtGState").map(|o| self.doc.resolve(o));
        let Some(state) = states.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)).map(|o| self.doc.resolve(o)) else { return Ok(None) };
        let font = state.as_dict().and_then(|d| d.get(b"Font")).map(|o| self.doc.resolve(o));
        match font.as_deref().and_then(Object::as_array).and_then(|a| a.first()) {
            Some(entry) => self.walk_type3(entry, at, res, calls).map(Some),
            None => Ok(None),
        }
    }

    /// Walk the glyph procedures of the Type 3 font `entry`, selected at index `at`, as unplaced
    /// content, once a page for each set of resources they read. Returns the font's kind.
    fn walk_type3(&mut self, entry: &Object, at: usize, res: Res<'_>, calls: &mut Vec<usize>) -> Result<FontKind, AuditError> {
        let font = self.doc.resolve(entry);
        let Some(font) = font.as_dict() else { return Ok(FontKind::Drawn) };
        let kind = match crate::subtype(self.doc, font)?.as_deref() {
            Some(b"Type3") => FontKind::Type3,
            Some(b"Type1" | b"MMType1" | b"TrueType") if !space_remapped(self.doc, font) => FontKind::Spaced,
            _ => FontKind::Drawn,
        };
        // A font without `/Resources` reads those where it is selected (PDF 1.1), as its glyphs
        // are drawn there: a glyph drawing a form that only some of them name shows its text there
        // alone.
        let own = font.get(b"Resources").map(|o| (o, self.doc.resolve(o)));
        let glyph = own_resources(own.as_ref(), entry.as_ref()).unwrap_or(res);
        if !matches!(kind, FontKind::Type3) || !self.first_walk(entry, glyph.owner) {
            return Ok(kind);
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
        Ok(kind)
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
        let data = self.budget.decode_strict(self.doc, stream)?;
        let outer = self.inside.replace(unplaced);
        calls.push(at);
        // Nothing painted here is placed, so the clip is never read.
        self.walk_nested(ops(&data), &mut Gs::new([0.0; 4]), res, calls)?;
        calls.pop();
        self.inside = outer;
        Ok(())
    }
}

/// Whether a simple font's `/Encoding` `/Differences` gives code 20h a glyph other than `/space`:
/// every standard base encoding leaves it the space.
fn space_remapped(doc: &Document, font: &Dict) -> bool {
    let encoding = font.get(b"Encoding").map(|o| doc.resolve(o));
    let differences = encoding.as_deref().and_then(Object::as_dict).and_then(|d| d.get(b"Differences")).map(|o| doc.resolve(o));
    let (mut code, mut remapped) = (None, false);
    for item in differences.as_deref().and_then(Object::as_array).into_iter().flatten() {
        if let Some(name) = item.as_name() {
            remapped = if code == Some(32.0) { name != b"space" } else { remapped };
            code = code.map(|c| c + 1.0);
        } else {
            code = item.as_f64();
        }
    }
    remapped
}

/// The resources a stream has of its own, `own` being its `/Resources` entry and what that
/// resolves to, `stream` its object number: `None` when it has none that are a dictionary, and it
/// reads those where it is used.
fn own_resources<'r>(own: Option<&'r (&Object, Arc<Object>)>, stream: Option<ObjRef>) -> Option<Res<'r>> {
    let (entry, resolved) = own?;
    Some(Res { dict: resolved.as_dict()?, owner: Owner::of(entry, stream) })
}

/// A `/BBox` (a form's, a shading's), normalized, or `None` when it is not 4 finite numbers. Any
/// other array is refused before its items are read, so a long one costs nothing at each use.
pub(crate) fn form_bbox(doc: &Document, dict: &Dict) -> Option<[f64; 4]> {
    let bbox = dict.get(b"BBox").map(|o| doc.resolve(o));
    let items = bbox.as_deref().and_then(Object::as_array).filter(|a| a.len() == 4)?;
    let mut b = [0.0; 4];
    for (x, o) in b.iter_mut().zip(items) {
        *x = doc.resolve(o).as_f64().filter(|x| x.is_finite())?;
    }
    let [x0, y0, x1, y1] = b;
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

/// The color space `cs` names, as the audit reads it: the components of its colors when it reads
/// them as gray (1), RGB (3) or CMYK (4), else 0 (a pattern, a spot color, an indexed or Lab space),
/// and its initial color (ISO 32000-2 §8.6.8). The spaces read are the device spaces, by name or
/// through `resources`, and calibrated or ICC-based ones in `resources`. DeviceCMYK starts black,
/// and the others at 0 in each component, which is white for an ICC-based CMYK space.
fn color_space(doc: &Document, resources: &Dict, name: &[u8]) -> (usize, Option<[f64; 3]>) {
    let device = |name: &[u8]| match name {
        b"DeviceGray" => Some(1),
        b"DeviceRGB" => Some(3),
        b"DeviceCMYK" => Some(4),
        _ => None,
    };
    let spaces = resources.get(b"ColorSpace").map(|o| doc.resolve(o));
    let space = spaces.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)).map(|o| doc.resolve(o));
    // A device space by name, or one of `resources` that names one.
    let named = device(name).or_else(|| space.as_deref().and_then(Object::as_name).and_then(device));
    let family = space.as_deref().and_then(Object::as_array);
    let part = |i: usize| family.and_then(|f| f.get(i)).map(|o| doc.resolve(o));
    let n = named.or_else(|| match part(0).as_deref().and_then(Object::as_name) {
        Some(b"CalGray") => Some(1),
        Some(b"CalRGB") => Some(3),
        Some(b"ICCBased") => {
            let n = part(1).as_deref().and_then(|s| s.as_dict()?.get(b"N").map(|o| doc.resolve(o))).and_then(|n| n.as_int());
            n.filter(|n| matches!(n, 1 | 3 | 4)).map(|n| n as usize)
        }
        _ => None,
    });
    let Some(n) = n else { return (0, None) };
    let cmyk = named == Some(4);
    (n, rgb(if cmyk { &[0.0, 0.0, 0.0, 1.0] } else { &[0.0; 4][..n] }))
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

/// What a named ExtGState sets of what decides whether a fill or a stroke hides what lies under it.
/// `None` for each it doesn't set.
#[derive(Default)]
struct StateSets {
    /// The non-stroking and stroking alpha (`/ca`, `/CA`).
    alpha: Option<f64>,
    stroke_alpha: Option<f64>,
    /// The line width (`/LW`).
    line_width: Option<f64>,
    /// Whether its line cap (`/LC`) is round or projecting square.
    capped: Option<bool>,
    /// Whether its dash pattern (`/D`) has dashes.
    dashed: Option<bool>,
    /// Whether its blend mode lets what lies under show through ([`shows_through`]).
    blended: Option<bool>,
    /// Whether it sets a soft mask (`/SMask` a dictionary, not `/None`).
    masked: Option<bool>,
}

/// What the ExtGState `name` sets ([`StateSets`]).
fn ext_state(doc: &Document, resources: &Dict, name: &[u8]) -> StateSets {
    // Borrowed, not copied: a `gs` must not cost the size of the whole ExtGState table.
    let states = resources.get(b"ExtGState").map(|o| doc.resolve(o));
    let state = states.as_deref().and_then(Object::as_dict).and_then(|d| d.get(name)).map(|o| doc.resolve(o));
    let Some(state) = state.as_deref().and_then(Object::as_dict) else { return StateSets::default() };
    let get = |key: &[u8]| state.get(key).map(|o| doc.resolve(o));
    let alpha = |key: &[u8]| get(key).and_then(|o| o.as_f64()).filter(|a| a.is_finite()).map(|a| a.clamp(0.0, 1.0));
    // `/D` is `[dash array, phase]`; an empty dash array is a solid line.
    let dashes = get(b"D").and_then(|d| d.as_array().and_then(|d| d.first()).map(|a| doc.resolve(a)));
    StateSets {
        alpha: alpha(b"ca"),
        stroke_alpha: alpha(b"CA"),
        line_width: get(b"LW").and_then(|o| o.as_f64()).filter(|w| w.is_finite() && *w >= 0.0),
        capped: get(b"LC").and_then(|o| o.as_f64()).filter(|c| [0.0, 1.0, 2.0].contains(c)).map(|c| c != 0.0),
        dashed: dashes.as_deref().and_then(Object::as_array).map(|a| !a.is_empty()),
        blended: get(b"BM").map(|bm| shows_through(doc, &bm)),
        masked: get(b"SMask").map(|m| m.as_dict().is_some()),
    }
}

/// The blend modes ISO 32000-2 defines (§11.3.5).
const BLEND_MODES: [&[u8]; 17] = [
    b"Normal",
    b"Compatible",
    b"Multiply",
    b"Screen",
    b"Overlay",
    b"Darken",
    b"Lighten",
    b"ColorDodge",
    b"ColorBurn",
    b"HardLight",
    b"SoftLight",
    b"Difference",
    b"Exclusion",
    b"Hue",
    b"Saturation",
    b"Color",
    b"Luminosity",
];

/// Whether the blend mode a `/BM` entry names lets what lies under a fill show through: any but
/// Normal and Compatible (ISO 32000-2 §11.3.5). Of an array, which lists modes for a viewer to try in
/// turn, the first entry counts, as PDFium reads it (pdf.js would try the next where it doesn't know
/// the first); a mode it doesn't know is Normal, as viewers paint it.
fn shows_through(doc: &Document, bm: &Object) -> bool {
    let first = bm.as_array().and_then(|a| a.first()).map(|o| doc.resolve(o));
    first.as_deref().unwrap_or(bm).as_name().is_some_and(|name| BLEND_MODES.contains(&name) && !matches!(name, b"Normal" | b"Compatible"))
}

/// Every filled rectangle drawn by the content of page `n` (and its form XObjects), then by
/// its annotations' `appearances`, with paint positions. A page's content streams are walked once,
/// joined, with one graphics state, so state set in an earlier stream is still in force in the
/// later ones. Any content stream that cannot be decoded, content, forms and appearances that
/// decode or enter past one page's [`DecodeBudget`] in all, or text in a glyph procedure or
/// pattern cell is an audit failure, never an empty result. `crop` is the page's crop box, the clip
/// its content and appearances start with. Also the layers that fill a shape other than a box, and
/// whether a stroke or an image with a mask of its own was painted.
pub fn filled_rects(
    doc: &Document,
    page: &Dict,
    crop: [f64; 4],
    n: usize,
    appearances: &[&Appearance],
) -> Result<(Vec<FilledRect>, HashSet<usize>, bool), AuditError> {
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
            streams.push(budget.decode_strict(doc, s)?);
        }
    }
    let pieces = Pieces::join(&streams);
    let mut w = Walker {
        doc,
        out: Vec::new(),
        ops_seen: 0,
        budget,
        page: n,
        layer: 0,
        inside: None,
        walked: HashSet::new(),
        saved: 0,
        shaped: HashSet::new(),
        unread: false,
    };
    let mut gs = Gs::new(crop);
    // Read one operator at a time: the joined stream can hold millions of them within the
    // decode budget, and the walk only ever needs the current one. The states the page leaves
    // saved go with it: a viewer paints each appearance from a fresh graphics state.
    let page = Res { dict: &resources, owner: Owner::Page };
    w.walk_nested(ops(pieces.data()), &mut gs, page, &mut Vec::new())?;
    for a in appearances {
        let Object::Stream(form) = &*a.form else { return Err(AuditError::BadAppearance(n)) };
        if !w.budget.enter_form() {
            return Err(AuditError::TooManyForms);
        }
        let data = w.budget.decode_strict(doc, form)?;
        // An appearance without resources of its own reads the page's, as a form does.
        let own = form.dict.get(b"Resources").map(|o| (o, doc.resolve(o)));
        w.layer = a.layer;
        // A viewer clips the appearance to its box, as a form's.
        let bbox = form_bbox(doc, &form.dict);
        let clip = bbox.map_or(crop, |bbox| meet(crop, a.ctm.bbox(bbox)));
        let mut gs = Gs { ctm: a.ctm, clip_exact: bbox.is_none_or(|bbox| box_on_page(&a.ctm, bbox)), ..Gs::new(clip) };
        w.walk_nested(ops(&data), &mut gs, own_resources(own.as_ref(), a.contents.as_ref()).unwrap_or(page), &mut Vec::new())?;
    }
    Ok((w.out, w.shaped, w.unread))
}
