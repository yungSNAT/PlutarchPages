# Redaction safety head-to-head

Measured 15 November 2026 (UTC). Which tools find a faux redaction, text a viewer shows hidden whilst
it can still be copied out of the file, in a set of one-page test files.

## Fixtures

Each file draws `SECRET 4417` and `PUBLIC LINE` in Helvetica. The unsafe files (U) hide
`SECRET 4417` on screen and leave it extractable; the safe ones (S) hide nothing a reader can't
see, or nothing at all.
[`head_to_head.rs`](../crates/audit/tests/it/head_to_head.rs) builds them object for object. The
'PDFium' column is a render with PDFium 159.0.8149.0 (pypdfium2 5.15.2): whether the pixels inside
the ink box of `SECRET 4417` vary (text shows) or are one value (text hidden). Every file still
extracts `SECRET` (in S01, the part left on show).

| File | What it draws | PDFium |
| --- | --- | --- |
| U01 | a black box over the text | hidden |
| U02 | a black box over red text | hidden |
| U03 | a white box over black text (whiteout) | hidden |
| U04 | a black image XObject over the text | hidden |
| U05 | a black inline image over the text | hidden |
| U06 | a filled Square annotation over the text | hidden |
| U07 | a Redact annotation marked and never applied | shows |
| U08 | a word of block characters typed over the text | hidden |
| U09 | a black shading clipped to the text's box | hidden |
| U10 | a Stamp annotation whose appearance draws a black image | hidden |
| S01 | an applied redaction: `SECRET` stays, and the removed text leaves a `TJ` gap with a box over it | shows |
| S02 | a yellow box multiplied onto the text (a highlight) | shows |
| S03 | a grey background painted before the text | shows |
| S04 | invisible OCR text under a white scan image, as OCRmyPDF writes it | hidden (by design) |

## Results

'Flags' means the tool reports the file as unsafe: `covered-text` for U01 to U06 and U08 to U10,
`unapplied-mark` for U07. A safe file passes when the tool reports nothing.

| Tool | Flags (of 10 unsafe) | Passes (of 4 safe) | Missed |
| --- | --- | --- | --- |
| PlutarchPages main: the audit `pdfcraft-cli audit` calls, in `head_to_head.rs` | 10 | 4 | none |
| PlutarchPages 0.18.2 release, `pdfcraft-cli audit` | 3 | 4 | U02 to U05, U08 to U10 |
| [pdfcraft](https://github.com/storytold/pdfcraft) 0.21.1 (upstream) release, `pdfcraft-cli audit` | 3 | 4 | U02 to U05, U08 to U10 |

Both released CLIs ran on Linux x86_64 from their release archives, checked against the release's
`SHA256SUMS.txt`, on the same files written to disk (the image samples Flate-compressed). Neither
flagged a safe file. Both missed the black box over red text, the whiteout, both images, the block
characters, the shading and the Stamp.

## Tools without a check for faux redactions

These tools apply redactions. None of the redaction code named below, read at each release's tag
on 15 November 2026, reports a cover over extractable text in a file it is given, and a GitHub code
search of each repository that day for 'faux', 'fake redaction' and 'improper redaction' found no
such check (one match, a translated OCR help page). KillerPDF and Open PDF Studio released again
by 16 November and were read again at their new tags that day, with the same searches (one match,
a page about a test server for Open PDF Studio's Python library). Having no such check, they were
not run on the fixtures.

- **Stirling-PDF** v3.3.0: its `/redact` and `/auto-redact` endpoints remove covered or matched
  page content ([`RedactController.java`](https://github.com/Stirling-Tools/Stirling-PDF/blob/v3.3.0/app/core/src/main/java/stirling/software/SPDF/controller/api/security/RedactController.java)). No detection.
- **BentoPDF** v2.8.11: the PDF Editor's redaction tools remove marked content once applied
  ([`edit-pdf.md`](https://github.com/alam00000/bentopdf/blob/v2.8.11/docs/tools/edit-pdf.md)); the workflow redact node calls PyMuPDF's `applyRedactions`
  ([`redact-node.ts`](https://github.com/alam00000/bentopdf/blob/v2.8.11/src/js/workflow/nodes/redact-node.ts)). No detection.
- **PDF4QT** v1.7.0.0: `PdfTool redact` creates a redacted document from the marks in the original
  ([`pdftoolredact.cpp`](https://github.com/JakubMelka/PDF4QT/blob/v1.7.0.0/PdfTool/pdftoolredact.cpp)). No detection.
- **Open PDF Studio** v2026.47.0: [`hidden-information-scan.js`](https://github.com/OpenAEC-Foundation/open-pdf-studio/blob/v2026.47.0/open-pdf-studio/js/pdf/hidden-information-scan.js) (unchanged since v2026.46.3) checks a cleaned copy for metadata,
  actions, attachments and comments the cleaner should have removed. It does not look for covers
  over text.
- **KillerPDF** v1.9.4 (Windows): `PdfContentRedaction.Verify` reads a redacted output back and
  reports content that still intersects regions the caller names ([`redaction.md`](https://github.com/SteveTheKiller/KillerPDF/blob/v1.9.4/engine/docs/redaction.md), unchanged since v1.9.3). It
  checks its own redaction of given regions; it does not find a cover another tool drew. v1.9.3
  and v1.9.4 made its own redaction remove more (every copy of a redacted form field; text under a
  rasterised region), not detect covers.

KillerPDF, Stirling-PDF, BentoPDF and PDF4QT remove content when told where; that is redaction,
which PlutarchPages also does. Of these tools, only pdfcraft and PlutarchPages check a file someone else
redacted.
