# Redactions other tools wrote

Made on 17 November 2026 (UTC) in the `archlinux:base-devel` image with PyMuPDF 2.1 (MuPDF
1.28.6), `mutool` 1.28.6 and pdfjs-dist 6.5.438 on Node 26.13.1. `tests/it/tool_made.rs` checks
what the audit reports for each; `docs/redaction-head-to-head.md` gives the results.

`base.pdf` is a 612 by 792 page from PyMuPDF: `insert_text` writes `SECRET 4417` at (100, 92) and
`PUBLIC LINE` at (100, 292), Helvetica 24, top-left coordinates. `search_for("SECRET 4417")` gives
the text's box, PDF user space [100 692.824 257.392 725.8]; every cover below is drawn over that box.

| File | Made by |
| --- | --- |
| `pymupdf-applied-no-garbage.pdf` | PyMuPDF: `add_redact_annot(box, fill=(0, 0, 0))`, then `apply_redactions()`, saved with `save()` |
| `pymupdf-applied.pdf` | PyMuPDF: `pymupdf-applied-no-garbage.pdf` saved again with `save(garbage=4)` |
| `pymupdf-mark-unapplied.pdf` | PyMuPDF: `add_redact_annot(box, fill=(0, 0, 0))`, saved without applying it |
| `pymupdf-drawn-box.pdf` | PyMuPDF: `draw_rect(box, color=None, fill=(0, 0, 0))`, into the page content |
| `pymupdf-square-annot.pdf` | PyMuPDF: `add_rect_annot(box)`, stroke and fill black, `update()` |
| `mutool-applied-no-garbage.pdf` | `mutool run`: a `Redact` annotation on the box, `update()`, `page.applyRedactions(true)`, saved with `save(path, "")` |
| `mutool-applied.pdf` | `mutool clean -gggg` on `mutool-applied-no-garbage.pdf` |
| `mutool-mark-unapplied.pdf` | `mutool run`: the same `Redact` annotation, saved without applying it |
| `mutool-square-annot.pdf` | `mutool run`: a `Square` annotation on the box, colour and interior colour black |
| `pdfjs-highlight.pdf` | pdf.js `saveDocument()` after its HIGHLIGHT editor: black, opacity 1, over the box |
| `pdfjs-ink.pdf` | pdf.js `saveDocument()` after its INK editor: one black stroke along the box's middle, as wide as the box is tall |

The 2 pdf.js files are `base.pdf` with an incremental update appended. The `mutool` files start
from `base.pdf` too.

Both `-no-garbage` files were saved without garbage collection. Their page no longer shows or
extracts `SECRET 4417`, but object 6, the page's original content stream, is still in the file with
nothing referring to it. The 2 `-applied` files were saved with garbage collection and hold no object
with the secret.
