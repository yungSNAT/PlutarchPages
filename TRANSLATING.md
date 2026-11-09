# Translating PlutarchPages

PlutarchPages uses the [craft-i18n catalogues](docs/localization.md) at the revision in
`craft-i18n.rev`. Its own rows live in `crates/ui-egui/i18n-fork/<locale>.tsv`.
Use the fork overlay for a PlutarchPages string. Translate a shared PdfCraft string in
craft-i18n.

1. Set `CRAFT_I18N_DIR` to a checkout at the pinned revision and run
   `cargo xtask i18n-report --fork`. The report lists every missing fork key for each
   shipped language. A row deliberately kept in English has its own count.
2. Copy the context and English source from another locale's overlay. Add the row to
   your locale's UTF-8 TSV with 3 tab-separated columns: context, English source and
   translation. For a plain string, leave the first column empty. Keep placeholders
   such as `{version}`, literal escape sequences and trailing ellipses. For a new
   fork string, add the first translated row to any overlay so the report sees it.
3. Update that file's SHA-256 and record the new rows' source and credit in
   `ATTRIBUTION.toml`, then run
   `cargo xtask assets --write` and `cargo xtask assets`. Run the report again
   and verify the key is gone from that locale's missing list. Run
   `cargo test -p pdfcraft-ui-egui --lib the_fork_overlay_is_embedded_and_served`
   with the pinned catalogues to check that the app serves the row.

Missing translations show in English. Label machine-assisted translations in the
pull request so a speaker can review them. If craft-i18n gains a row already in an
overlay, remove the duplicate and update the attribution hash.
