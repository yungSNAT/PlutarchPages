# pdfcraft-audit

Layer L4 (may use `edit`): check PDFs for potential redaction leaks.

```rust
let report = audit_document(&doc)?; // page_findings and document_findings
```

The report checks for:

- **faux redaction**: a filled rectangle, a thick straight stroke (a pen line across the text),
  an image, or a word of block characters (`█`, `■` and the like), painted *after* text it
  overlaps, while the text is still extractable underneath; filled Square annotations count as
  covers too. A cover painted fully opaque in the Normal blend
  mode hides the text whatever its colour: it replaces what lies under it, so a black box over red
  text or a white box over black text both hide it. A translucent cover (`/ca` above 0 and below 1,
  or a Square's interior colour at `/CA` below 1), a blended one (`/BM` other than Normal, as a
  highlight multiplied onto the page is; of a list, its first entry, as PDFium reads it, and Normal
  for a mode it doesn't know) or one painted through a soft mask hides the text when it leaves it
  unreadable (WCAG contrast below 3). A translucent fill in the Normal blend mode is mixed with the
  text and with what lies around it: the white page, or rectangles painted before the text over its
  whole line, in colours the audit reads. So black at half opacity over black text on white is
  legible, and on mid grey is not. Where the audit can't read what lies under the text, and for a
  blended or masked cover, a Square annotation or a word of blocks, the cover is judged by its
  colour against the text's;
- **unapplied mark**: a Redact annotation that was marked but never applied;
- **retained advance**: a large forward `TJ` adjustment entirely under a later
  cover. An adjustment can preserve the removed text's exact width even after the
  text has gone. Ordinary text positioning can produce the same signal;
- **earlier revisions**: counts incremental revisions that a reader can inspect;
- **unreferenced text**: counts streams nothing in the file refers to that still show text, as a
  redaction applied and saved without garbage collection leaves the page's old content. No viewer
  draws them, but any reader of the file's objects recovers the text. `pdfcraft-cli audit` fails
  on them, as on covered text. A document an edit has marked for a full rewrite, as applying a
  redaction does, has none: its next save keeps only what is referred to;
- **document metadata**: counts entries in the document information dictionary and
  reports an XMP metadata stream attached to the catalogue. The audit returns counts
  only.

A properly applied redaction can still have a retained advance, earlier revision or
metadata finding. Check those signals before sharing the file. Page backgrounds and table
shading painted before the text, and highlights multiplied onto it or translucent enough to
leave it readable, are excluded from the covered-text check.

[The redaction head-to-head](../../docs/redaction-head-to-head.md) runs this audit, the released
pdfcraft and PlutarchPages audits, and reads 5 other open source PDF tools' redaction code, on 10
unsafe and 4 safe files (15 November 2026).

A rectangle covers a text line when it hides at least half of one of the line's words, at
least 40% of the area of the words the line draws, or at least 40% of the line's box together
with a positive-area intersection with one of its words. The gap an applied redaction leaves (a `TJ` adjustment where the text was) is in the line's box but
holds no word. A box over the gap alone can produce a retained-advance finding.

Covers and text drawn by form XObjects, nested ones included, are checked like the page's own,
in the order they are painted: a cover a form paints after its text hides it, as does one the
page paints after drawing the form. So are those of the appearance a viewer paints for each
annotation that isn't Hidden or NoView (its normal appearance, or the state `/AS` names), mapped
onto the annotation's rectangle and painted after the page in `/Annots` order. An appearance
without resources of its own uses the page's. PDFium and pdf.js paint an appearance as it is,
whatever the annotation's `/CA` and `/BM`, so those don't change it. A Square is checked by the
box its appearance paints. Its interior colour (`/IC`) counts where the audit has no appearance
for it, or the appearance fills a shape it reads no box from (a cloudy border), for text no box
lies over: pdf.js, PDFium, Poppler and MuPDF paint the appearance where there is one, so `/IC`
behind a border-only appearance hides nothing. A Redact mark's own appearance is not text it
leaves under it.

The audit doesn't place text a tiling pattern's cell or a Type 3 font's glyph procedure shows,
which a viewer paints once for each tile or glyph. Such text fails the audit. A font counts
whether `Tf` selects it or a graphics state's `/Font` sets it (`gs`), and its text is read either
way. A Type 3 font or tiling pattern without resources of its own reads those where it is used,
so it is checked with each set it is used with.

Limits, by design and documented rather than hidden:

- only filled rectangles and straight strokes count as covers: rectangles drawn with `re` or as a single `m`/`l` subpath whose
  corners form an axis-aligned rectangle (not other paths; several `re` filled together only by
  the nonzero rule and all drawn the same way round), shadings painted with `sh`, over the
  box that holds their clip and within their `/BBox`, images (an image XObject, an inline image or
  an image mask) over the box the CTM places them in, within the box that holds their clip, and
  words of block characters drawn filled (render mode 0, 2, 4 or 6), over the box of each word. A
  stroke counts when it is one subpath along one horizontal or vertical line, solid (no dash
  pattern), under a CTM that keeps the axes, stroked alone (`S`, `s`) or over a fill (`B`, `B*`,
  `b`, `b*`): it covers the band its line width paints between its ends, its caps not counted.
  Each of these counts only under a clip that is a box on the page (a path of one `re` or one
  four-cornered subpath, or a form's or appearance's `/BBox`, whose corners land on a box, and
  no glyph shown in a render mode that clips, 4 to 7, other than a space in a simple font or a
  Type 3 glyph, which add nothing to the clip; another glyph without an outline is still taken to
  clip), and only where what it paints lands on a box there (not a rectangle, `/BBox` or image the CTM
  shears), since otherwise the clip or the shape may leave the text showing. Other strokes, such as a stroked rectangle's border or a curved pen
  line, are not covers. Other glyphs,
  such as a Type 3 box that maps to a letter, are not covers, and a run whose text doesn't split
  into as many words as it draws counts only when it is all blocks. An image's pixels are not
  read: one with a mask of its own (`/SMask`, `/Mask` or a JPEG 2000 image's `/SMaskInData`) is
  not a cover, and one without hides its whole box, even where its pixels are white or match the
  page. An image doesn't hide invisible text (render mode 3 or 7): OCR tools such as OCRmyPDF
  draw the scan over the text they read from it, so a black image pasted over a scan's OCR layer
  is not flagged, though a filled rectangle there is. A translucent, blended or soft-masked
  cover is judged by its colour as set with `g`, `rg`, `k` or in a gray, RGB or CMYK colour space
  with `cs` and `sc`/`scn` (a stroke's with `G`, `RG`, `K`, `CS` and `SC`/`SCN`), a block by its
  text's colour. On a page with a stroke, or an image with
  a mask of its own, a translucent cover is judged by its colour too, as is one in a transparency
  group (a form with `/Group`) drawn translucent, blended or through a soft mask, whatever its own
  opacity. One in a colour the audit doesn't read (a
  pattern's, a spot colour's, a shading's, an image's) counts as dark as the text. A soft mask is
  not read, and text a soft mask hides is outside the covered-text check;
- the retained-advance check considers forward `TJ` adjustments larger than a quarter em;
  it cannot establish which text originally occupied a gap or whether all redaction tools
  leave one. The audit counts earlier revisions without reading their content;
- the unreferenced-text check reads every stream but the binary ones that are never content:
  not those typed as an image, a `/FontFile3` font program, a cross-reference stream, an embedded
  file, a sound, 3D artwork or a halftone, nor one without `/Type` or `/Subtype` that a font
  descriptor names as its `/FontFile` or `/FontFile2`. XMP metadata, CMaps and object streams
  are read, as is an embedded file without the `/Type` the standard leaves optional. It counts each that shows text with
  `Tj`, `TJ`, `'` or `"`, whatever the text says. An object only an earlier revision holds is not
  counted there: the earlier-revisions count covers it. So a file built to hide text can still
  get past it: an unreferenced stream typed as one of those is not read, nor is one with neither
  key, as a font program has none, that a dictionary typed `/FontDescriptor` names as its font
  program, even when that dictionary is no real descriptor;
- annotation rectangles are compared in unrotated user space, so `/Rotate` pages may misalign;
- a cover that hides less than half of every word it touches, less than 40% of a line's box and
  less than 40% of its words (a sliver of a long value) is not flagged.

It fails closed: an undecodable content stream, malformed `/Annots`, form XObjects nested deeper
than 32 or in a cycle, a form whose text can't be read (it draws itself, doesn't decode, or comes
past the text reader's form limits), an annotation appearance without a usable `/BBox`, text in a
tiling pattern or a Type 3 glyph, a walk past the operator bound (2 million operators a page, its
forms and appearances included, counted before any text is read), more than 4,096 graphics states
saved at once (the renderer's own limit), more than 16 MiB of text on a
page and its appearances, more than 50 million
cover-against-word comparisons on a page, or more than 64 MiB of covered text in the report is an
error, never "no findings". Errors name pages from 1.

A document whose permissions withhold copying (Table 22, bit 5) still gets every finding, with
an empty `covered_text`: the audit hands out no text that copying would refuse.

`doc_audit_redactions` and `pdfcraft-cli audit` return `findings` and `count` for
page findings, plus `document_findings` and `document_count`. The CLI exits with
an error for covered text, unapplied marks or an audit failure. Retained advances
and document-level findings are advisory. Rectangles are
in points from the top-left of the displayed page (y down, after `/Rotate`).
`AuditFinding::rect` stays in PDF user space. The library's `audit_redactions`
entry point checks only covered text and unapplied marks.

`fuzz/` is a detached cargo-fuzz crate (`cargo +nightly fuzz run content`, run from
`crates/audit/`). Its target reads each input as a page's content, and as the content of the form,
Type 3 glyph, tiling pattern and annotation appearance the page offers, or as a whole file when it
starts with `%PDF`. The documents the integration tests build make its seed corpus: with
`PDFCRAFT_AUDIT_SEEDS` naming an existing folder, `cargo test -p pdfcraft-audit --test it` saves
each one there.

```sh
mkdir -p fuzz/corpus/content
PDFCRAFT_AUDIT_SEEDS="$PWD/fuzz/corpus/content" cargo test -p pdfcraft-audit --test it
cargo +nightly fuzz run content fuzz/corpus/content
```

The weekly coverage-guided fuzz workflow (`.github/workflows/cargo-fuzz.yml`) runs it this way.
