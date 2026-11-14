# CSV formula protection: spreadsheet import trials

`crates/xfdf/README.md` says the CSV formula protection was tested in Excel 365 and LibreOffice
Calc across 29 import settings. These are the settings and the results, recorded for #101 on
13 November 2026.

## What was imported

The CSV files were written by the crate's own `merge_csv`, which shares `csv_cell` with Export Data
as CSV, through a probe example kept outside the repository. Every value is made up.

- `probe.csv`: a header and 378 records, each on one line, in these groups: `cross-is_amount-start` 60, `cross-is_amount-after-semicolon` 60, `cross-f64-start` 30, `cross-f64-after-semicolon` 30, `cross-whitespace-start` 20, `cross-whitespace-after-semicolon` 20, `semicolon-cell-first` 10, `semicolon-cell-last` 10, `semicolon-cell-then-5+A1` 10, `semicolon-closed-first` 10, `semicolon-closed-last` 10, `tab-cell-first` 10, `tab-cell-last` 10, `tab-cell-then-5+A1` 10, `tab-closed-first` 10, `tab-closed-last` 10, `amount-first` 29, `amount-second` 29.
  The 29 amount shapes from #98 make up `amount-first` (in the first cell) and `amount-second`
  (in the second). sha256 `4f338dc38f2e2054e490b357025469638d7d363075b2adb6673312c2308bc934`.
- `probe-linebreak.csv`: 123 records whose cells hold a line break (LF, CR or CRLF), most of them
  before a formula-like payload. sha256 `b0a14eb8ac428e3a0125a2ca6986dbad3c772d1a8979e66fce76ea426e66e129`.
- `probe-bom.csv`: one record whose first field name, `-1`, is led by a byte order mark.
  sha256 `07e0171c02877ad8d1fd26e50426f709e24e2791c24e8d7563e10448d2e1415a`.

Before is `main` at 49bf04b7, before #101. After is #101 (`crates/xfdf/src/lib.rs` sha256
`6ef32602fd3b849e658f55456395f096980473b8f83d18bbc82a91993474540e`, as merged in ce634056).

## Applications and settings

Excel: Microsoft 365, version 16.0 build 20625, through COM. Windows locale English (Australia):
decimal `.`, thousands `,`, list separator `,`.

- `open-default`: `Workbooks.Open` with COM defaults (`Local` false).
- `open-local`: `Workbooks.Open` with `Local` true.
- `opentext-utf8-en` and `opentext-utf8-de`: `Workbooks.OpenText`, UTF-8, comma delimited, double
  quote qualifier, every column General; decimal `.` and thousands `,` (`en`) or decimal `,` and
  thousands `.` (`de`).
- `opentext-utf8-en-semicolon` and `opentext-utf8-de-semicolon`: the same split on `;` only.
- `opentext-utf8-en-tab` and `opentext-utf8-de-tab`: the same split on tabs only.
- `appsep-de-open-default` and `appsep-de-open-local`: system separators off, decimal `,` and
  thousands `.` as the Application separators, then `Workbooks.Open` without and with `Local`.
  The separators were restored afterwards.

Calc: LibreOffice 26.8.2.2, headless on Windows, each run with a fresh user profile. The CSV
filter converted each file to flat ODS, read from the XML. Unless named, the filter splits on `,`
with a double quote qualifier, UTF-8, 'Quoted field as text' off.

- `en-US-defaults`, `en-AU-defaults` and `de-DE-defaults`: 'Detect special numbers' and 'Evaluate
  formulas' off, with that language.
- `en-US-evaluate-special`, `en-AU-evaluate-special` and `de-DE-evaluate-special`: both on.
- `bare-en-AU` and `bare-de-DE`: no filter options; the profile's locale English (Australia) or
  German.
- `semicolon-en-US-evaluate-special`, `semicolon-de-DE-evaluate-special`,
  `tab-en-US-evaluate-special` and `tab-de-DE-evaluate-special`: both on, split on `;` or tabs
  only.
- `comma-semicolon-de-DE-evaluate-special`: both on, split on `,` and `;`, German.
- `trim-comma-*`, `trim-semicolon-*` and `trim-tab-*`, each `en-US` and `de-DE`: both on, split on
  `,`, `;` or tabs, with 'Trim spaces' on.

## Results

Each formula count is the cells the application held as a formula after opening the file; `by
group` splits those for `probe.csv` by record group. The last 2 columns count how many of the 29
amount shapes the application held as numbers. A setting that splits on `;` or tabs alone holds
each amount record in one cell, so it holds no amount as a number, before or after.

### Before (49bf04b7)

| Setting | Formulae, probe.csv | by group | Formulae, probe-linebreak.csv | Formulae, probe-bom.csv | amount-first as numbers | amount-second as numbers |
| --- | --- | --- | --- | --- | --- | --- |
| Excel appsep-de-open-default | 27 | cross-f64-after-semicolon 6, cross-f64-start 6, semicolon-cell-last 4, semicolon-cell-then-5+A1 2, semicolon-closed-first 4, semicolon-closed-last 5 | 42 | 1 | 0 | 0 |
| Excel appsep-de-open-local | 27 | cross-f64-after-semicolon 6, cross-f64-start 6, semicolon-cell-last 4, semicolon-cell-then-5+A1 2, semicolon-closed-first 4, semicolon-closed-last 5 | 42 | 1 | 0 | 0 |
| Excel open-default | 0 | - | 0 | 0 | 25 | 25 |
| Excel open-local | 0 | - | 0 | 0 | 25 | 25 |
| Excel opentext-utf8-de-semicolon | 38 | cross-f64-after-semicolon 6, cross-f64-start 6, semicolon-cell-first 3, semicolon-cell-last 6, semicolon-cell-then-5+A1 4, semicolon-closed-first 6, semicolon-closed-last 7 | 42 | 1 | 0 | 0 |
| Excel opentext-utf8-de-tab | 13 | cross-f64-start 6, tab-closed-last 7 | 42 | 1 | 0 | 0 |
| Excel opentext-utf8-de | 0 | - | 42 | 0 | 13 | 13 |
| Excel opentext-utf8-en-semicolon | 22 | semicolon-cell-first 3, semicolon-cell-last 6, semicolon-closed-first 6, semicolon-closed-last 7 | 42 | 0 | 0 | 0 |
| Excel opentext-utf8-en-tab | 7 | tab-closed-last 7 | 42 | 0 | 0 | 0 |
| Excel opentext-utf8-en | 0 | - | 42 | 0 | 25 | 25 |
| Calc bare-de-DE | 0 | - | 0 | 0 | 13 | 13 |
| Calc bare-en-AU | 0 | - | 0 | 0 | 22 | 22 |
| Calc comma-semicolon-de-DE-evaluate-special | 10 | semicolon-cell-first 2, semicolon-cell-last 2, semicolon-cell-then-5+A1 2, semicolon-closed-first 2, semicolon-closed-last 2 | 0 | 0 | 13 | 13 |
| Calc de-DE-defaults | 0 | - | 0 | 0 | 13 | 13 |
| Calc de-DE-evaluate-special | 0 | - | 0 | 0 | 13 | 13 |
| Calc en-AU-defaults | 0 | - | 0 | 0 | 22 | 22 |
| Calc en-AU-evaluate-special | 0 | - | 0 | 0 | 22 | 22 |
| Calc en-US-defaults | 0 | - | 0 | 0 | 22 | 22 |
| Calc en-US-evaluate-special | 0 | - | 0 | 0 | 22 | 22 |
| Calc semicolon-de-DE-evaluate-special | 10 | semicolon-cell-first 2, semicolon-cell-last 2, semicolon-cell-then-5+A1 2, semicolon-closed-first 2, semicolon-closed-last 2 | 24 | 0 | 0 | 0 |
| Calc semicolon-en-US-evaluate-special | 10 | semicolon-cell-first 2, semicolon-cell-last 2, semicolon-cell-then-5+A1 2, semicolon-closed-first 2, semicolon-closed-last 2 | 24 | 0 | 0 | 0 |
| Calc tab-de-DE-evaluate-special | 10 | tab-cell-first 2, tab-cell-last 2, tab-cell-then-5+A1 2, tab-closed-first 2, tab-closed-last 2 | 24 | 0 | 0 | 0 |
| Calc tab-en-US-evaluate-special | 10 | tab-cell-first 2, tab-cell-last 2, tab-cell-then-5+A1 2, tab-closed-first 2, tab-closed-last 2 | 24 | 0 | 0 | 0 |
| Calc trim-comma-de-DE-evaluate-special | 0 | - | 0 | 0 | 13 | 13 |
| Calc trim-comma-en-US-evaluate-special | 0 | - | 0 | 0 | 22 | 22 |
| Calc trim-semicolon-de-DE-evaluate-special | 15 | semicolon-cell-first 3, semicolon-cell-last 3, semicolon-cell-then-5+A1 3, semicolon-closed-first 3, semicolon-closed-last 3 | 36 | 0 | 0 | 0 |
| Calc trim-semicolon-en-US-evaluate-special | 15 | semicolon-cell-first 3, semicolon-cell-last 3, semicolon-cell-then-5+A1 3, semicolon-closed-first 3, semicolon-closed-last 3 | 36 | 0 | 0 | 0 |
| Calc trim-tab-de-DE-evaluate-special | 15 | tab-cell-first 3, tab-cell-last 3, tab-cell-then-5+A1 3, tab-closed-first 3, tab-closed-last 3 | 36 | 0 | 0 | 0 |
| Calc trim-tab-en-US-evaluate-special | 15 | tab-cell-first 3, tab-cell-last 3, tab-cell-then-5+A1 3, tab-closed-first 3, tab-closed-last 3 | 36 | 0 | 0 | 0 |

### After (#101)

| Setting | Formulae, probe.csv | by group | Formulae, probe-linebreak.csv | Formulae, probe-bom.csv | amount-first as numbers | amount-second as numbers |
| --- | --- | --- | --- | --- | --- | --- |
| Excel appsep-de-open-default | 0 | - | 0 | 0 | 0 | 0 |
| Excel appsep-de-open-local | 0 | - | 0 | 0 | 0 | 0 |
| Excel open-default | 0 | - | 0 | 0 | 25 | 25 |
| Excel open-local | 0 | - | 0 | 0 | 25 | 25 |
| Excel opentext-utf8-de-semicolon | 0 | - | 0 | 0 | 0 | 0 |
| Excel opentext-utf8-de-tab | 0 | - | 0 | 0 | 0 | 0 |
| Excel opentext-utf8-de | 0 | - | 0 | 0 | 13 | 13 |
| Excel opentext-utf8-en-semicolon | 0 | - | 0 | 0 | 0 | 0 |
| Excel opentext-utf8-en-tab | 0 | - | 0 | 0 | 0 | 0 |
| Excel opentext-utf8-en | 0 | - | 0 | 0 | 25 | 25 |
| Calc bare-de-DE | 0 | - | 0 | 0 | 13 | 13 |
| Calc bare-en-AU | 0 | - | 0 | 0 | 22 | 22 |
| Calc comma-semicolon-de-DE-evaluate-special | 0 | - | 0 | 0 | 13 | 13 |
| Calc de-DE-defaults | 0 | - | 0 | 0 | 13 | 13 |
| Calc de-DE-evaluate-special | 0 | - | 0 | 0 | 13 | 13 |
| Calc en-AU-defaults | 0 | - | 0 | 0 | 22 | 22 |
| Calc en-AU-evaluate-special | 0 | - | 0 | 0 | 22 | 22 |
| Calc en-US-defaults | 0 | - | 0 | 0 | 22 | 22 |
| Calc en-US-evaluate-special | 0 | - | 0 | 0 | 22 | 22 |
| Calc semicolon-de-DE-evaluate-special | 0 | - | 0 | 0 | 0 | 0 |
| Calc semicolon-en-US-evaluate-special | 0 | - | 0 | 0 | 0 | 0 |
| Calc tab-de-DE-evaluate-special | 0 | - | 0 | 0 | 0 | 0 |
| Calc tab-en-US-evaluate-special | 0 | - | 0 | 0 | 0 | 0 |
| Calc trim-comma-de-DE-evaluate-special | 0 | - | 0 | 0 | 13 | 13 |
| Calc trim-comma-en-US-evaluate-special | 0 | - | 0 | 0 | 22 | 22 |
| Calc trim-semicolon-de-DE-evaluate-special | 0 | - | 0 | 0 | 0 | 0 |
| Calc trim-semicolon-en-US-evaluate-special | 0 | - | 0 | 0 | 0 | 0 |
| Calc trim-tab-de-DE-evaluate-special | 0 | - | 0 | 0 | 0 | 0 |
| Calc trim-tab-en-US-evaluate-special | 0 | - | 0 | 0 | 0 | 0 |

Every setting holds no formula after #101, and every amount held as a number before still is.

## Not tested

Excel's Power Query import (Data > From Text/CSV), a German Windows or Excel installation
(simulated above with Text Import and the Application separators), Excel for Mac or the web,
Google Sheets, Numbers, Calc's 'Quoted field as text', and separators other than `,`, `;` and tab.
