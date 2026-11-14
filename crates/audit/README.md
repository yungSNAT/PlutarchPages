# pdfcraft-audit

Layer L4 (may use `edit`): check PDFs for potential redaction leaks.

```rust
let report = audit_document(&doc)?; // page_findings and document_findings
```

The report checks for:

- **faux redaction**: an opaque filled rectangle painted *after* text it overlaps, dark enough
  against the text's colour (WCAG contrast below 3) to hide it, while the text is still
  extractable underneath; opaque filled Square annotations count as covers too;
- **unapplied mark**: a Redact annotation that was marked but never applied;
- **retained advance**: a large forward `TJ` adjustment entirely under a later opaque
  rectangle. An adjustment can preserve the removed text's exact width even after the
  text has gone. Ordinary text positioning can produce the same signal;
- **earlier revisions**: counts incremental revisions that a reader can inspect;
- **document metadata**: counts entries in the document information dictionary and
  reports an XMP metadata stream attached to the catalogue. The audit returns counts
  only.

A properly applied redaction can still have a retained advance, earlier revision or
metadata finding. Check those signals before sharing the file. Page backgrounds, table
shading and highlights are excluded from the covered-text check.

A rectangle covers a text line when it hides at least half of one of the line's words, at
least 40% of the area of the words the line draws, or at least 40% of the line's box together
with a positive-area intersection with one of its words. The gap an applied redaction leaves (a `TJ` adjustment where the text was) is in the line's box but
holds no word. A box over the gap alone can produce a retained-advance finding.

Covers and text drawn by form XObjects, nested ones included, are checked like the page's own,
in the order they are painted: a cover a form paints after its text hides it, as does one the
page paints after drawing the form.

Limits, by design and documented rather than hidden:

- only filled rectangles count as covers, drawn with `re` or as a single `m`/`l` subpath whose
  corners form an axis-aligned rectangle (not other paths, shadings or patterns), and a
  translucent one (`/ca` below 1) is not flagged;
- the retained-advance check considers forward `TJ` adjustments larger than a quarter em;
  it cannot establish which text originally occupied a gap or whether all redaction tools
  leave one. The audit counts earlier revisions without reading their content;
- annotation rectangles are compared in unrotated user space, so `/Rotate` pages may misalign;
- a cover that hides less than half of every word it touches, less than 40% of a line's box and
  less than 40% of its words (a sliver of a long value) is not flagged.

It fails closed: an undecodable content stream, malformed `/Annots`, form XObjects nested deeper
than 32 or in a cycle, a form whose text can't be read (it draws itself, doesn't decode, or comes
past the text reader's form limits), a walk past the operator bound, more than 50 million cover-against-word
comparisons on a page, or more than 64 MiB of covered text in the report is an error, never "no
findings".

A document whose permissions withhold copying (Table 22, bit 5) still gets every finding, with
an empty `covered_text`: the audit hands out no text that copying would refuse.

`doc_audit_redactions` and `pdfcraft-cli audit` return `findings` and `count` for
page findings, plus `document_findings` and `document_count`. The CLI exits with
an error for covered text, unapplied marks or an audit failure. Retained advances
and document-level findings are advisory. Rectangles are
in points from the top-left of the displayed page (y down, after `/Rotate`).
`AuditFinding::rect` stays in PDF user space. The library's `audit_redactions`
entry point checks only covered text and unapplied marks.
