# hayro-interpret, vendored

Base: `9eb02be84f21898e247daeb2af5827c9fe85a3a0` (storytold/craft-libs `vendor/hayro-interpret`, Apache-2.0 OR MIT).

Upstream PdfCraft builds hayro from craft-libs at that revision. This copy is craft-libs' own with
one fork patch, and the root `Cargo.toml` patches it in with `[patch.crates-io]`.
`cargo xtask pdf-deps` fails when the base above is not its `SHARED_REV`.

## What differs from the base

- `src/font/mod.rs` and `src/font/type1.rs`: `OutlineGlyph::standard_encoding_char` gives a
  standard Type 1 outline's character from the encoding that selected it, ignoring `/ToUnicode`,
  for the fork's `font_mapping_differs` signal (#74). Covered by
  `text::tests::fork_tests::integrity_signals_distinguish_each_split_view_family` in
  `pdfcraft-render`.
- `SOURCE.json`: those 2 files' `sha256` values, recomputed.
- This file.

## Moving to another revision

1. Copy craft-libs `vendor/hayro-interpret` at the new revision over this folder, keeping this file.
2. Add the accessor back to the 2 files and recompute their `SOURCE.json` hashes.
3. Set the base above, and run `cargo xtask pdf-deps` and the `pdfcraft-render` tests.
