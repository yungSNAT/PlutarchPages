# Filling a form once per data row

`pdfcraft-cli form-fill-batch` fills a copy of a PDF form for each row of a CSV,
tab-delimited or JSON file and writes each copy as its own PDF. It prints a JSON
report to stdout and leaves the form and the data file unchanged.

```sh
# One PDF per CSV row, named form-1.pdf, form-2.pdf, ...
pdfcraft-cli form-fill-batch form.pdf --data rows.csv --out-dir filled --root .

# Name each file from a column, and bake the values into the pages.
pdfcraft-cli form-fill-batch form.pdf --data rows.json --out-dir filled --name-column File --flatten
```

## Data

- **CSV or tab-delimited text**: the first row names the fields, as Export Data
  writes it. In CSV, quoted cells may hold commas, doubled quotes and line breaks.
  In tab-delimited text, each line is one row and each tab starts a new cell. Blank
  lines are skipped. A check box cell is empty or `Off` (cleared) or one of the
  box's on states; anything else, `yes` or `1` included, fails the row. A
  multi-select list cell joins its options with `, `.
- **JSON**: an array of objects, each mapping field names to values as
  `form_fill` takes them: strings, numbers, `true` or `false` for check boxes,
  arrays of strings for multi-select lists. A check box also takes the strings
  `form_fill` takes, such as `"yes"`, `"on"` and `"1"`. `null` clears a radio
  group and fails the row for a text, combo or list field. When a row repeats a
  key, the last value is used.

Rows are numbered from 1 after the names row. The data file must be UTF-8 (a
spreadsheet's 'Unicode text' or ANSI CSV export is refused as a whole; save as
CSV UTF-8), at most 32 MiB and at most 10000 rows. XML, XFDF and FDF data hold
one record; import them with `doc_import_data` instead.

## A row is all or nothing

Each row fills a fresh copy of the form, so calculations, formats and validation
run as they do when typing. A row fails, and nothing is written for it, when it
names a field the form lacks, gives a value the form refuses (a format or
validation failure, an option the field does not list, a read-only field), gives
a calculated field a value the form then calculates differently, or has the wrong
number of cells. A calculated field's value must match the form's result as the
form stores it, character for character (`6` and `6.00` differ); an empty cell
leaves the calculation to the form. The other rows go on. The command exits
with status 1 when any row failed.

A field the row does not name keeps the form's value, unsaved edits included.
Required fields are not enforced. A names row with an empty name (a trailing
comma, as some spreadsheets export) names a field the form lacks, so every row
fails; remove the empty column.

Each value is set as if typed, and the form recalculates after each one, so a
form with many calculated fields fills slowly, row by row.

## Errors and personal data

Rows often hold personal data, so a row's error names the row and the field and
never quotes a cell or a file name from the data. When the form refuses a value,
the reason itself may quote it, so the error says only that the form refused it,
even for reasons such as a read-only field or a length limit. Add `--verbose`
(`"verbose": true` through `run` or MCP) to see the value and the form's reason.

## Output names

With `--name-column`, that column's value names the file (`.pdf` is added) and
fills no field. Without it, files are named after the form: `form-1.pdf`,
`form-2.pdf` and so on. Every name is checked before the first file is written.
A name fails its row when it:

- is `.` or `..`, or longer than 200 bytes;
- holds a path separator, a control character or one of `: * ? " < > |`;
- holds an invisible or direction-changing character (such as a right-to-left
  override or a zero-width space), which can make a name display as another;
- starts with a space, or ends with a space or dot before `.pdf`;
- is a Windows device name with any extension (`CON`, `NUL`, `CONIN$`, `COM0` to
  `COM9`, `LPT0` to `LPT9` and their superscript forms).

A name two rows share fails both. Names are compared ignoring case and Unicode
normalisation, as Windows and macOS file systems compare them, so `Café` composed
and decomposed clash. Each file is written atomically and never replaces an
existing file: a taken name gets a number, `kept (2).pdf`, as in `batch`, and the output
folder is opened as `batch` opens it (no links on the way; on Windows a volume mounted at a folder is
refused too; on macOS
and other systems without `O_PATH` the output folder and the folders above it must be readable,
and a folder that has to be made needs a writable parent).

Each file is staged as a temporary file in the output folder and renamed into
place. If the process is killed or the power fails mid-row, that staging file
(named `.tmp` and random characters; on Windows 2 such files) can remain; delete them. The files already
published stay but the report is lost: rerun the rows that have no file in the
folder.

With `--root DIR`, the form, the data file and the output folder must be inside
DIR.

## Report

```json
{
  "completed": 2,
  "failed": 1,
  "rows": [
    { "row": 1, "status": "ok", "output": "filled/form-1.pdf", "bytes": 18234, "sha256": "9f2c…" },
    { "row": 2, "status": "error", "error": "Choice: ..." },
    { "row": 3, "status": "ok", "output": "filled/form-3.pdf", "bytes": 18240, "sha256": "41d0…" }
  ]
}
```

`sha256` is the SHA-256 of the bytes written, a receipt to check the file against
later.

## Through `run` and MCP

The command opens the form and calls the `form_fill_batch` tool, which takes an
open document, so the copy includes any unsaved edits to it:

```json
{ "tool": "form_fill_batch", "args": { "doc": 1, "data": "rows.csv", "out_dir": "filled", "name_column": "File", "flatten": false } }
```

Over MCP, a call with a failed row is marked as an error and keeps the report.
At most 10000 rows are filled in one call.
