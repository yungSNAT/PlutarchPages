# pdfcraft-audit

Layer L4 (may use `edit`): check PDFs for potential redaction leaks.

```rust
let report = audit_document(&doc)?; // page_findings and document_findings
```

The report checks for:

- **faux redaction**: a filled rectangle, an image, or a word of block characters (`█`, `■` and
  the like), painted *after* text it overlaps, while the text is still extractable underneath;
  filled Square annotations count as covers too. A cover painted fully opaque in the Normal blend
  mode hides the text whatever its colour: it replaces what lies under it, so a black box over red
  text or a white box over black text both hide it. A translucent cover (`/ca` above 0 and below 1,
  or a Square's interior colour at `/CA` below 1), a blended one (`/BM` other than Normal, as a
  highlight multiplied onto the page is; of a list, its first entry, as PDFium reads it, and Normal
  for a mode it doesn't know) or one painted through a soft mask hides the text when it is dark
  enough against the text's colour (WCAG contrast below 3): how much it hides depends on what lies
  under the text, which the audit doesn't read. A word of blocks is judged by its colour that way too;
- **unapplied mark**: a Redact annotation that was marked but never applied;
- **retained advance**: a large forward `TJ` adjustment entirely under a later
  cover. An adjustment can preserve the removed text's exact width even after the
  text has gone. Ordinary text positioning can produce the same signal;
- **earlier revisions**: counts incremental revisions that a reader can inspect;
- **document metadata**: counts entries in the document information dictionary and
  reports an XMP metadata stream attached to the catalogue. The audit returns counts
  only.

A properly applied redaction can still have a retained advance, earlier revision or
metadata finding. Check those signals before sharing the file. Page backgrounds and table
shading painted before the text, and highlights multiplied or painted translucent onto it, are
excluded from the covered-text check.

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
whatever the annotation's `/CA` and `/BM`, so those don't change it. A Square is checked by the box its appearance paints. Its interior colour (`/IC`)
counts where the audit has no appearance for it, or the appearance fills a shape it reads no box
from (a cloudy border), for text no box lies over: pdf.js, PDFium, Poppler and MuPDF paint the
appearance where there is one, so `/IC` behind a border-only appearance hides nothing. A Redact
mark's own appearance is not text it leaves under it.

The audit doesn't place text a tiling pattern's cell or a Type 3 font's glyph procedure shows,
which a viewer paints once for each tile or glyph. Such text fails the audit. A font counts
whether `Tf` selects it or a graphics state's `/Font` sets it (`gs`), and its text is read either
way. A Type 3 font or tiling pattern without resources of its own reads those where it is used,
so it is checked with each set it is used with.

Limits, by design and documented rather than hidden:

- only filled rectangles count as covers, drawn with `re` or as a single `m`/`l` subpath whose
  corners form an axis-aligned rectangle (not other paths), shadings painted with `sh`, over the
  box that holds their clip and within their `/BBox`, images (an image XObject, an inline image or
  an image mask) over the box the CTM places them in, within the box that holds their clip, and
  words of block characters drawn filled (render mode 0, 2, 4 or 6), over the box of each word. Other glyphs,
  such as a Type 3 box that maps to a letter, are not covers, and a run whose text doesn't split
  into as many words as it draws counts only when it is all blocks. An image's pixels are not
  read: one with a mask of its own (`/SMask`, `/Mask` or a JPEG 2000 image's `/SMaskInData`) is
  not a cover, and one without hides its whole box, even where its pixels are white or match the
  page. An image doesn't hide invisible text (render mode 3 or 7): OCR tools such as OCRmyPDF
  draw the scan over the text they read from it, so a black image pasted over a scan's OCR layer
  is not flagged, though a filled rectangle there is. A translucent, blended or soft-masked
  cover is judged by its colour as set with `g`, `rg`, `k` or in a gray, RGB or CMYK colour space
  with `cs` and `sc`/`scn`, a block by its text's colour. One in a colour the audit doesn't read (a
  pattern's, a spot colour's, a shading's, an image's) counts as dark as the text. A soft mask is
  not read, and text a soft mask hides is outside the covered-text check;
- the retained-advance check considers forward `TJ` adjustments larger than a quarter em;
  it cannot establish which text originally occupied a gap or whether all redaction tools
  leave one. The audit counts earlier revisions without reading their content;
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
starts with `%PDF`.
