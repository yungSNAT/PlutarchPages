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
  writes it. Quoted cells may hold commas, doubled quotes and line breaks. Blank
  lines are skipped. A check box cell is empty or `Off` (cleared) or one of the
  box's on states; a multi-select list cell joins its options with `, `.
- **JSON**: an array of objects, each mapping field names to values as
  `form_fill` takes them: strings, numbers, `true` or `false` for check boxes,
  arrays of strings for multi-select lists.

Rows are numbered from 1 after the names row. XML, XFDF and FDF data hold one
record; import them with `doc_import_data` instead.

## A row is all or nothing

Each row fills a fresh copy of the form, so calculations, formats and validation
run as they do when typing. A row fails, and nothing is written for it, when it
names a field the form lacks, gives a value the form refuses (a format or
validation failure, an option the field does not list) or has the wrong number of
cells. The other rows go on. The command exits with status 1 when any row failed.

## Output names

With `--name-column`, that column's value names the file (`.pdf` is added) and
fills no field. Without it, files are named after the form: `form-1.pdf`,
`form-2.pdf` and so on. Every name is checked before the first file is written:
a name with a path separator, `..`, a control character, one of `: * ? " < > |`
or a Windows device name (`CON`, `NUL`, `COM1` ...) fails its row, and a name two
rows share (compared ignoring case) fails both. Each file is written atomically
and never replaces an existing file: a taken name gets a number, `kept (2).pdf`,
as in `batch`.

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
