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

## Launch to first page on screen

Measured 19 November 2026 (UTC). How long each viewer takes from being started with a file to
showing that file's first page in its window: the PlutarchPages app at `main` (`546192981`,
version 0.21.1) against MuPDF's viewer `mupdf-gl` 1.28.6. Same machine and files as above.

### Method on screen

- Both viewers ran in the same `archlinux:base-devel` image (20261025.0.616225) on the Ryzen 7
  4800H above, on an Xvfb 21.1.26 screen of 1920 by 1080 pixels at 24 bits, under openbox
  3.6.1. Nothing set a display scale; PlutarchPages chose 1.083 pixels a point. Other jobs shared
  the machine: the load average was 6.2 just before timing and 9.2 just after.
- There is no graphics processor: both drew with Mesa 26.3.1's software renderers (llvmpipe,
  and `vulkan-swrast` 26.3.1 installed for Vulkan). Times on a machine with one will differ.
- PlutarchPages: built on the host with `cargo build --release -p pdfcraft -p pdfcraft-cli` and
  rustc 1.100.0, started as `pdfcraft --control CTL FILE`. Its default renderer, wgpu, found
  only a processor device and said so in a notice. A third column starts it with
  `PDFCRAFT_RENDERER=gl`, its OpenGL renderer.
  `mupdf-gl`: Arch's `mupdf-gl` 1.28.6-1 with freeglut 3.8.0-1, started as `mupdf FILE`. Without
  a window manager it exits at once (`BadAtom` from `X_DeleteProperty`), so both ran under
  openbox.
- Each viewer opened each file with its defaults: PlutarchPages in a 1560 by 997 window at its
  fit-width zoom (120%, or 82% for ISO 32000-1); `mupdf-gl` in a window it sizes to the page,
  804 by 1040 (735 by 1040 for ISO 32000-1).
- Every run started from a new, empty home folder. Each viewer had 1 warm-up run and then 10
  timed runs for each file, the viewers taking turns. The files were in the page cache.
- Timing: the script started the viewer and read Xvfb's screen, which Xvfb kept in a
  memory-mapped file (`-fbdir`), every 2 to 6 ms. The first page counted as shown at the first
  read in which 99.9% of the page's pixels (every 16th one read) and 99% of its ink (pixels that
  are not paper white; every 4th one read) matched that viewer's settled screen within 8 levels
  on each colour channel. The settled screen was taken 10 s after a separate launch.
  - The page's pixels are the first page's rectangle inside the viewport: from `ui.state`
    (`pages_on_screen` and `viewport`) for PlutarchPages, and the whole window for `mupdf-gl`.
  - Without a graphics processor PlutarchPages shows a notice over the bottom of the page for
    about 4 s after launch. Its box, found as a dark area over the page at 3 s that the settled
    screen does not show, is left out of the comparison. The other 2 columns show no notice.
  - The script is not in the repository. Its rule is the one above; a 95% rule over the whole
    screen passed before a sparse page was drawn, and the notice made a whole-screen 99.9% rule
    wait for it to go.
- The PlutarchPages columns also give the time its control method `ui.first_page_paint` reported
  (`unix_ms` less the start time): when it painted the page, before the frame reached the screen.

### Results on screen

Milliseconds from start to the first page on screen: median of 10 runs, then the interquartile
range and the range.

| File | PlutarchPages (wgpu) | PlutarchPages (OpenGL) | `mupdf-gl` | Faster |
| --- | --- | --- | --- | --- |
| One-page file | 976 (970 to 994; 882 to 1,027) | 521 (486 to 544; 476 to 561) | 258 (251 to 266; 223 to 269) | `mupdf-gl`, 3.8 times |
| tracemonkey | 965 (935 to 1,010; 914 to 1,036) | 544 (518 to 562; 510 to 633) | 277 (257 to 303; 250 to 341) | `mupdf-gl`, 3.5 times |
| ISO 32000-1 | 1,656 (1,602 to 1,779; 1,589 to 1,850) | 1,136 (1,085 to 1,275; 1,080 to 1,300) | 282 (266 to 319; 256 to 328) | `mupdf-gl`, 5.9 times |

'Faster' compares PlutarchPages with its default renderer. `mupdf-gl` shows the first page
sooner on every file: 3.5 to 5.9 times sooner by default, and 2.0 to 4.0 times sooner than
PlutarchPages with OpenGL. OpenGL cuts PlutarchPages' time by 0.4 to 0.5 s on each file, so
much of the default's time goes on starting wgpu without a graphics processor. On ISO 32000-1,
PlutarchPages takes 0.6 to 0.7 s longer than on the other files, in line with the cold-start
results above, where opening and parsing that file took most of the time.

`ui.first_page_paint`, median of the same runs: 923, 907 and 1,609 ms with wgpu and 497, 522 and
1,116 ms with OpenGL, for the 3 files in the order above. The screen medians are 20 to 58 ms
later than these.

Raw times in milliseconds, in run order:

| File | Viewer | Screen | `ui.first_page_paint` |
| --- | --- | --- | --- |
| One-page file | PlutarchPages (wgpu) | 969.9, 882.3, 969.5, 987.0, 1027.2, 975.7, 971.1, 978.7, 1015.2, 976.7 | 920.2, 831.8, 919.7, 939.8, 984.1, 932.5, 918.2, 925.3, 961.8, 916.2 |
| One-page file | PlutarchPages (OpenGL) | 476.4, 486.6, 483.8, 506.7, 548.7, 535.9, 542.5, 538.6, 486.3, 561.3 | 456.5, 470.0, 467.2, 484.8, 527.9, 509.7, 519.9, 513.4, 462.6, 535.9 |
| One-page file | `mupdf-gl` | 223.0, 248.8, 251.9, 257.8, 268.1, 269.0, 255.4, 263.4, 265.1, 257.9 | |
| tracemonkey | PlutarchPages (wgpu) | 914.3, 1029.4, 961.6, 973.6, 917.8, 940.3, 1035.5, 1003.6, 955.1, 967.9 | 858.4, 977.7, 896.0, 918.6, 864.0, 882.3, 979.0, 950.9, 896.2, 916.9 |
| tracemonkey | PlutarchPages (OpenGL) | 545.1, 516.6, 541.0, 518.1, 510.0, 542.9, 632.6, 547.0, 559.3, 568.7 | 524.9, 492.1, 520.6, 503.6, 481.9, 517.5, 608.1, 523.9, 538.0, 539.1 |
| tracemonkey | `mupdf-gl` | 258.7, 265.6, 249.7, 251.8, 269.2, 299.8, 340.9, 312.2, 293.2, 285.2 | |
| ISO 32000-1 | PlutarchPages (wgpu) | 1849.8, 1814.5, 1695.1, 1767.7, 1751.3, 1602.3, 1601.3, 1588.6, 1615.9, 1605.7 | 1798.0, 1759.9, 1645.4, 1715.3, 1709.6, 1552.7, 1561.3, 1550.6, 1573.3, 1565.0 |
| ISO 32000-1 | PlutarchPages (OpenGL) | 1263.8, 1273.3, 1162.5, 1278.4, 1300.4, 1080.6, 1101.5, 1087.0, 1109.7, 1079.6 | 1251.4, 1250.9, 1141.6, 1258.4, 1281.8, 1062.6, 1082.9, 1067.1, 1089.9, 1063.5 |
| ISO 32000-1 | `mupdf-gl` | 328.4, 316.2, 326.6, 290.6, 308.5, 266.2, 256.0, 267.9, 274.4, 265.2 | |

## Not measured

- A machine with a graphics processor: both timings above drew in software.
- SumatraPDF, which runs only on Windows: not timed in either run.
- A read from disk with an empty page cache.
