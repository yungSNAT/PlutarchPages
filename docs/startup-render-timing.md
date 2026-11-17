# Cold start and first-page render against MuPDF

Measured 17 November 2026 (UTC). How long a fresh process takes to open a file, render its first
page at 96 dpi and write it as a PNG: `pdfcraft-cli render` at `main` (`e6693c4fd`) against MuPDF's
`mutool draw` 1.28.6, the renderer that MuPDF's viewer `mupdf-gl` draws with.

## Method

- Both ran in the same `archlinux:base-devel` image (20261025.0.616225) on Linux x86_64, an AMD
  Ryzen 7 4800H with 16 threads, all visible in the container. Other jobs shared the machine: the
  load average was 7.6 about 15 minutes before timing and 6.6 just after.
- `pdfcraft-cli`: built on the host with `cargo build --release -p pdfcraft-cli` and rustc 1.100.0.
  `mutool`: Arch's `mupdf-tools` 1.28.6-1.
- Commands, timed with hyperfine 2.0.0 (`-N`, 3 warm-up runs, then 30):
  - `pdfcraft-cli render FILE --page 1 --dpi 96 --out out.png`
  - `mutool draw -r 96 -o out.png FILE 1`
- 'Cold' means a new process for each run: start-up, opening and parsing the file, rendering
  page 1 and writing the PNG. The file itself is in the page cache after the warm-up runs.
- Both wrote 8-bit RGB PNGs of the same dimensions for each file (816 by 1056 pixels, or 794 by
  1123 for ISO 32000-1). PlutarchPages' PNGs are larger files.

| File | SHA-256 (first 12) | Size |
| --- | --- | --- |
| A one-page file from PyMuPDF 2.1: 2 lines of Helvetica 24 | `47797230b089` | 949 bytes |
| pdf.js's `web/compressed.tracemonkey-pldi-09.pdf`, from `master` on 17 November 2026 | `3662ff519e48` | 1.0 MB |
| ISO 32000-1:2008 (`PDF32000_2008.pdf`, as Adobe publishes it) | `9de0ca9e8570` | 22.5 MB |

## Results

Wall time, mean ± standard deviation of 30 runs, and peak memory (mean).

| File | PlutarchPages | `mutool draw` | Faster |
| --- | --- | --- | --- |
| One-page file | 21.7 ± 2.0 ms, 20.2 MiB | 47.1 ± 6.7 ms, 24.4 MiB | PlutarchPages, 2.2 times |
| tracemonkey | 63.4 ± 3.5 ms, 22.9 MiB | 74.5 ± 4.3 ms, 25.4 MiB | PlutarchPages, 1.2 times |
| ISO 32000-1 | 384.0 ± 10.0 ms, 77.2 MiB | 73.8 ± 3.3 ms, 31.9 MiB | `mutool`, 5.2 times |

On the 2 small files PlutarchPages starts and renders sooner. On the large one `mutool` is 5.2
times faster, and PlutarchPages uses 2.4 times as much memory (77.2 against 31.9 MiB).
`pdfcraft-cli` reported its render of that page as 2 ms in one run, and its whole run takes 362 ms
longer than on the one-page file, so most of the time goes on opening and parsing the file before
the page is drawn. That split was not timed separately.

## Not measured

- The viewers' windows: `mupdf-gl` and the PlutarchPages app from launch to the first page on
  screen. Both need a display and a signal for the first frame; this run measured the renderers
  without windows.
- SumatraPDF, which runs only on Windows: not timed in this run.
- A read from disk with an empty page cache.
