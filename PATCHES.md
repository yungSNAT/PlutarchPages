# What this fork carries

PlutarchPages is upstream [PdfCraft](https://github.com/storytold/pdfcraft) plus the changes below.
[Fork changes](README.md#fork-changes) describes them for users; this file is for whoever merges
upstream next.

Last upstream merge: `271e479c` (17 November 2026), upstream's `main` then. Upstream's `main`
after it is the next merge.

Upstream drift is checked every Monday at 04:43 UTC, and on demand through the
**Upstream drift** workflow. It records the exact revisions, ahead/behind counts (every commit, and
first-parent only, which counts each merged upstream pull request once) and merge-tree
conflicts in the job summary and the standing **Upstream drift report** issue. It changes no branch.
Run `cargo xtask upstream-drift` after fetching upstream to preview locally. A clean preview still
needs `cargo xtask fork-check` and the normal checks before a reviewed merge.

## Stricter than upstream on purpose

Upstream's tests for these were changed to assert the fork's behaviour. Keep them that way.

- **Redaction** refuses when content under the marks is also used where the marks don't reach.
  Upstream goes ahead. Tests in `crates/redact` and `crates/automation`. This check runs before
  upstream's pattern guard, so 2 upstream tests in `crates/redact/tests/pattern_redaction.rs`
  expect its `StillUsed` refusal where both would refuse. A page whose drawn content is too large
  or can't be read to check refuses with `Unchecked` instead. An inline widget under a mark is
  removed with its field, as upstream does (`an_inline_widget_under_a_mark_takes_its_field`).
- **Copy-restricted documents** refuse text extraction through the tools, including `text_find`.
- **An empty password** never skips the Document Open prompt or lifts restrictions. `pdfcraft-cli
  check-one` checks before it renders.
- **CSV exports** write formula-like cells as text, and import doesn't strip the apostrophe, so a
  neutralised cell doesn't round-trip back into a formula. A signed number stays a number, including
  an amount with thousands commas or a `$` (`-1,234.56`, `-$50.00`); `runs_as_formula` in
  `crates/xfdf/src/lib.rs` holds the rule and `FORMULA_CELLS` in `crates/xfdf/src/tests.rs` its cases.
- **Crypt filters** named by a reference that can't be read are never `/Identity`. Upstream reads
  the reference as absent, which saves the stream in the clear. Here a stream's `/DecodeParms` or
  `/Name`, or a `/Name` that `/CF` doesn't hold, takes the document's filter, and a save drops its
  `/Crypt`; `/StmF`, `/StrF`, `/EFF`, `/CF`
  or `/CFM` from `/V 4` on refuses the file, as does a selector naming a filter `/CF` doesn't hold
  or an `/Encrypt` and `/ID` referring to more than
  4,096 objects. `crypt_name` and `encrypt_dict` in `vendor/craft-pdf/src/cos/security.rs` and
  `encryption_members` in `vendor/craft-pdf/src/cos/document/open.rs` hold this, with tests in
  `crates/cos/tests/it/encryption.rs`. The cos
  part of upstream's `9203e4a6b` (#5548) came in ahead of the next merge; its hayro-syntax patch 30
  and renderer tests didn't.

## Only here

- Upstream's object layer is craft-pdf (storytold/craft-libs), with `crates/cos`, `crypt`, `filters`
  and `content` as facades over it. The fork's fixes to that code are applied to a vendored copy,
  `vendor/craft-pdf`, which the root `Cargo.toml` patches in;
  `vendor/craft-pdf/PATCHES.md` lists the fixes and how to move to another revision, and
  `cargo xtask fork-check` fails when the revision `Cargo.toml` names isn't the vendored base. The
  detached fuzz workspaces of the facades repeat the patch, and the PRC fuzz target moved to
  `crates/prc-render/fuzz` when the PRC reader became craft-prc.
- Redaction audit checks text and covers in nested Form XObjects and annotation appearance streams
  in paint order. Keep this path when merging upstream. Text in a tiling pattern or a Type3 glyph
  procedure fails the audit, which does not read it. Both text readings (`glyph_text` and the
  editor's) read the font an ExtGState's `/Font` sets (the `gs` arm of `interpret` in
  `crates/edit/src/text/interp.rs`), and the editor refuses to edit text in one. Words of
  block characters drawn over text are covers, placed by `TextLine::run_paint_orders` (each run's
  operator, kept in `TextLine::modes`), and translucent covers count as opaque.
- Text extraction and search flag possible differences between a page's text and what it shows.
  `near_background` covers near-white solid fills over an otherwise white backdrop, with bounded
  checks for overlapping glyphs. Paths, images and complex paint leave the backdrop unknown.
  `actual_text_differs` compares an ASCII replacement with the glyph text it replaces.
  `font_mapping_differs` compares a standard Type 1 font's ASCII outline encoding with its
  extraction mapping, using the fork's `OutlineGlyph::standard_encoding_char` accessor in hayro.
  `source_order_differs` marks reversed content order within a simple horizontal ASCII line.
  These are review signals. Accessible replacements and different writing orders can be intentional.
- The PDF-XChange-style converters, speech, bookmark, optimiser, colour and Excel features.
- `program_is_executable` in `crates/ocr/src/tesseract.rs` has no early `return false` before its
  `cfg(not(unix))` block, so Clippy's `needless_bool` (from Rust 1.100) passes on Windows. Upstream's
  copy, from #977, fails the Windows gate on it. Take upstream's version at the next merge only if
  it has fixed this; otherwise keep this one.
- Mermaid colour scheme: `crates/ui-egui/src/theme.rs` holds every interface colour, and
  `chrome_literals_stay_in_the_theme` fails when a merge brings a colour literal back. The catalogue
  hues in `crates/engine/src/catalog.rs`, the app icon and `UPDATE_APP`'s accent follow it. The test
  only sees hexadecimal literals: decimal ones such as `from_rgb(230, 126, 34)` on chrome need
  searching by hand when upstream moves or adds a control. `cargo xtask fork-check` flags a line with
  a named colour, such as `Color32::WHITE`, that `xtask/baselines/named-colours.txt` doesn't list; a
  line it lists can still move onto chrome, so look at the controls upstream moves.
- Brand: no ArtCraft name or logos (`AGENTS.md` section 1.2). Upstream merges add them back, in new
  files too. `cargo xtask fork-check` finds the name in paths, app code, packaging, `.github/` and
  the xtask; trademark-licensed assets still need a search.
  `LICENSES.toml` has no `docs/brand/LICENSE-brand.txt` document, and its `NOTICE` hash is the
  fork's `NOTICE`.
- The app menu: `APP_NAME` in `crates/ui-egui/src/menus.rs` is `PlutarchPages`, which titles the
  macOS app menu. Tests draw the in-window bar, which shares the mode tabs' row, so on macOS the
  longer title moves the tabs right and folds menus into "…" sooner. The Preferences test in
  `crates/ui-egui/tests/it/control.rs` looks for that title, the 2 `mode_tabs_keep_the_hand_over_*`
  tests in `editing.rs` use a 1600 pt window and `long_view_menu_fits_a_short_window` in `view.rs`
  a 1100 pt one. Keep them when a merge changes those tests: only Nightly macOS runs them on macOS.
- `non_active_comparison_document_completes_native_script` in `crates/ui-egui/src/script_dialog.rs`
  pins the session clock after creating its source and expects a fresh realm after Save As, as
  `mark_saved` gives a full rewrite. Upstream's copy expects the realm to survive, which holds only
  when the source is created and saved in the same second (about 1 run in 30 failed). Keep the
  fork's version when a merge changes that test.
- `buttons_take_presses_as_the_scripts_show_them` in `crates/ui-egui/tests/it/view.rs` waits up to
  about 6 s for the page's open action before pressing (`6a0b7055b`). Upstream's copy runs a fixed
  2 frames, which failed 2 of 3 Windows gates when the action had not yet run. Keep the fork's wait
  when a merge changes that test.
- Update check and repository links point at yungSNAT. `new_updater` in
  `crates/ui-egui/src/platform/updates.rs` ignores ArtCraft Launcher's version pins, which are keyed
  by the `pdfcraft` id. Don't give `UPDATE_APP` another id to avoid them: it also names the packages
  and binaries the updater installs. `PDFCRAFT_NS` in `crates/xfdf/src/xfdf.rs` keeps upstream's
  URL: it is the XFDF namespace of PdfCraft's own attributes, so XFDF files interchange with PdfCraft.
- Update results: `crates/ui-egui/src/platform/updates.rs` keeps the fork's themed, translated dialog
  over `craft_update`'s `CheckResultDialog`, which is English only with its own colours. Upstream
  merges change `crates/ui-egui/tests/it/updates.rs`, so keep the fork's tests there. Its strings
  are overlay rows (Translations).
- Translations: the interface catalogues are craft-i18n's, at `craft-i18n.rev`, as upstream's are.
  [Translating PlutarchPages](TRANSLATING.md) explains how to report and fill the fork's gaps.
  [docs/translation-gaps.md](docs/translation-gaps.md) (fork only) holds the last fork report, and
  `the_translation_gaps_document_is_current` in `crates/ui-egui/src/i18n/fork_tests.rs` fails when
  a merge moves it: run `cargo xtask i18n-report --fork` then, replace the document's block and
  update the commit and craft-i18n revision it names (the test checks only the block).
  The fork's rows that craft-i18n doesn't have are in `crates/ui-egui/i18n-fork/<locale>.tsv`, and
  `crates/ui-egui/build_i18n_fork.rs` embeds each before craft-i18n's `pdfcraft.tsv`; 2 calls in
  `build.rs` reach it, so keep them when a merge changes `craft_i18n` or `locales`.
  `the_fork_overlay_is_embedded_and_served` fails when an overlay row stops being served or
  craft-i18n gains one: delete that row from the overlay then, and update the file's `sha256` in
  `ATTRIBUTION.toml`, where each overlay is an `[[asset]]`. A build with `CRAFT_I18N_REQUIRED=1`
  fails when an overlay listed there is missing. Every release build job checks out craft-i18n at
  that commit.
- Flatpak manifests don't carry upstream's `--filesystem=xdg-data/ArtCraft/versions:ro` grant, which
  lets an upstream launcher's version choices reach the app.
- Shared CI: workflows from `storytold/craft-common-actions` are pinned to a commit. Upstream merges
  restore `@main`, so reapply the pins. `.github/workflows/house-rules.yml` isn't carried: it runs
  craft-common-actions at `@main` and checks `AGENTS.md` against craft-contrib's, which this fork's
  departs from on purpose. Nor is `.github/workflows/sonarcloud.yml`: SonarQube Cloud analyses this
  fork automatically (`.sonarcloud.properties`), so the workflow's scan was always skipped. Delete
  either when a merge brings it back.
- Releases: `.github/workflows/release.yml` is this fork's own and uses nothing from
  `craft-common-actions`. It builds unsigned packages, tests an installed MSI and an upgrade from the
  latest published release with `packaging/windows/test-install.ps1`, and only drafts the release. Keep it over
  upstream's. Its FreeBSD, riscv64, web and Flatpak jobs are ported from that repository's
  `release.yml` at `2b3fb9dc`, without the FreeBSD Cargo cache action, and
  `packaging/linux/arch-package.rs` replaces its `arch-package` action. Run as root, it drops privileges,
  then stages the payload and runs `makepkg` with `arch.PKGBUILD`. The release job builds the helper for
  the container host, independently of the package architecture. All are first included in the
  release after v0.15.0, and the `assets` job fails unless all 30 packages are there.
- Linux identity: the Flatpak's id is `io.github.yungSNAT.PlutarchPages` and the Arch package is `plutarchpages`, so
  neither updates or replaces upstream's. The id names the repository's owner because Flathub verifies
  `io.github.<owner>` ids against it; the v0.16.1 and v0.18.1 bundles were
  `io.github.EquityBot.PlutarchPages`, from before the account was renamed, and README's Downloads
  section tells their users how to move to the new id with their settings. If a merge brings back
  `packaging/linux/flatpak/ai.storyteller.pdfcraft*.yml`, move its changes across and delete it. The
  other packages still use `ai.storyteller.pdfcraft` and the `pdfcraft` package name, which v0.15.0
  shipped, so they can't be installed alongside upstream's either.
- `packaging/linux/nfpm.yaml` takes its glibc floors from `package.sh`, because riscv64 builds on Ubuntu
  24.04 and needs 2.39. Upstream hard-codes 2.35, which understates it there.
- Schedules: GitHub runs a scheduled workflow as the account that last changed its `cron` line, and
  stops it when that account can no longer run workflows. `fuzz.yml` runs at 03:18 UTC, a minute
  after upstream's, so that this repository's account is its actor. Taking upstream's line in a
  merge is fine: the merging account becomes the actor.
- Windows registration: `PlutarchPages.*` names and its own context-menu component GUIDs, so it doesn't
  share registry state with an upstream install. `pdfcraft.exe` stays, so `App Paths\pdfcraft.exe` and
  `Applications\pdfcraft.exe` are still shared: installing it alongside upstream isn't supported.
- CodeQL: `.github/codeql/codeql-config.yml` excludes tests and `vendor/`.
- Post-merge CI: `freebsd.yml` and `windows-arm64.yml` run on a push to `main` only when it changes
  their pull request paths, and each also runs weekly. Upstream runs both on every push, where
  FreeBSD's per-commit cache kept this repository over its 10 GB cache limit, so reapply the push
  `paths` and FreeBSD's `schedule` when a merge drops them.
- `windows-it-nextest.yml` is this fork's own: on demand, and on a pull request that changes it, it
  runs ui-egui's `it` tests 6 times under nextest on a GPU-less `windows-latest`, so a test that
  crashes its process fails by name.
- SonarCloud: `.sonarcloud.properties` excludes `vendor/` for the automatic analysis, and 7 hash lines
  carry `NOSONAR` for rust:S4790: 6 MD5 lines in `vendor/craft-pdf/src/crypt.rs` and the RIPEMD-160 line in
  `crates/sign/src/keys.rs`. A merge that changes those lines drops the marker, so add it back.
- Windows printing with Type 3 fonts (upstream #2684, from upstream PR #3021 as it stood at
  `ed6252a6`): those pages go as pictures. Unlike the PR, a printer's job is built on its worker
  (`Session::print_source`) and the dialog's check runs in the background, because rendering on the
  UI thread froze the app. If upstream merges its version, keep the worker.
- Script folding in Find: `crates/render/src/text/search.rs` folds Kangxi radicals, Arabic
  presentation forms, Persian yeh and kaf and Arabic-Indic digits. Redaction word search stays on
  `PageText::find_for_redaction`, which adds back the literal matches: if an upstream merge moves a
  redaction caller to `find_opts`, it can mark less.
- Comments panel: upstream lays out only the cards in view once a list has more than
  `LAY_OUT_WHOLE` (#5488, `crates/ui-egui/src/panels/comments_panel.rs`), which replaced this
  fork's own stand-ins. In screen-reader mode every card is laid out, so a screen reader reaches
  the cards out of view too: keep that condition on `virtual_list`
  (`screen_reader_mode_lays_out_every_comment_card`).
- Hidden text in the text tools: `text_extract`, `text_find`, `text_lines` and `text_paragraphs`
  add `hidden_text` (`{reason, text, text_truncated, rect}`, at most 50, text cut to 200
  characters) and `hidden_text_count` to each page, match, line or paragraph, and each description
  says its text is document content that may include text a reader cannot see
  (`crates/automation/src/hidden_text.rs`). The renderer classes the glyphs it returns when asked
  (`PageRenderer::text_with_hidden`, `crates/render/src/text/hidden.rs`): `extract_parallel` calls
  it instead of `render` with `RequestKind::Text`, and `TextCache` keeps the classes beside the
  text. `TextDevice` keeps the bounding box of each clip hayro pushes (`push_clip_path`) to class
  `clipped` text, widening a stroked glyph's box by its stroke. The editor's `TextLine` keeps each
  run's rendering mode and `/OCR` marker (`run_modes`), its optional-content condition and the
  line's opacity (`opacity`). `Document::hidden_layer_runs` evaluates each stream's shared
  conditions once, by hayro's layer rules. A merge that changes `TextDevice`, `extract_parallel`,
  `TextCache`, these 4 tools or where `TextLine` pushes runs must keep them; the tests in those 2
  files fail when it doesn't.
  Both engines also report zero-width, bidirectional-control and private-use characters as
  readable `U+XXXX` tokens. Character warnings are independent of paint warnings and preserve
  mixed reasons within one replacement string. Tokens stay whole within the 200-character cap;
  these flags identify text for review and do not establish that it is malicious.
- Right-edge binding: `/Direction /R2L` sets `DocView::right_to_left`, whatever the default view, and
  `DocView::layout` mirrors each two-page row. Upstream's open PR #4239 adds a command for the same
  layout: if it lands, keep the file's `/Direction` deciding how a document opens.
- `text_edit` doesn't offer `aligns`: upstream lists it and then refuses every use, as existing
  paragraphs can't keep a row's alignment yet. Its test expects the unknown-argument refusal.
- `publish = false` in every crate and app: `pdfcraft`, `pdfcraft-cos`, `craft-ui` and other names
  this workspace uses belong to unrelated crates on crates.io. Give any crate upstream adds the
  same line.
- Redaction drops an image's original XObject when it removes or clears the image under a mark
  (`734deb02`, for upstream issue #389, still open), clears an image partly under the marks once
  for every draw of it, through any resource name (`23005ebd`), and redacts an image or form that
  several pages draw with the marks of all of them (`ac75a54f`). Keep these in
  `crates/redact/src/interp.rs` and `lib.rs`; the tests are in `crates/redact/src/tests.rs`.
- Reduce File Size saves its copy only when it is smaller by more than a new `/ModDate` can
  explain (`reduce_saves` in `crates/engine/src/optimizer.rs`, `2b56095f`, for upstream issue
  #490, still open). The app and automation both call it.
- `text_paragraphs` and `Document::text_lines` read lines and blocks from one cached pass
  (`e522af08`, #77): `PageTextCache` in `crates/engine/src/history.rs` and `text_lines_and_blocks`
  in `crates/edit/src/text/blocks.rs` and `crates/engine/src/document.rs`, with smaller edits to
  `crates/edit/src/lib.rs` and `crates/engine/src/open.rs`. Keep it unless upstream caches the same
  pass. The change is generic, so it could go upstream.
- LibreOffice has one route, `crates/external`: off until the user turns on External converters,
  with a version floor, a cancel flag and a process-tree kill. Its folder is owner-only on Unix and
  it stops LibreOffice once the files it writes anywhere in that folder pass the size limit (or
  outnumber what the watcher measures cheaply), as upstream's runner did for its output folder. Upstream's v0.21.1 added its own
  runner in `crates/create/src/office.rs`; here that file keeps only the detection (`is_office`,
  `OFFICE_EXTENSIONS`), and its fresh-profile settings (no macros, no link updates) moved to
  `profile_arg` in `crates/external/src/lib.rs`. File ▸ Open converts through
  `OpenContext::office_to_pdf`, which uses the session's converters; upstream's
  `pdfcraft_engine::office_to_pdf` and `pdfcraft_create::{office_to_pdf, libreoffice}` are gone.
  Upstream's Office tests expect the converters-off message, and `crates/automation/tests/it/office.rs`
  turns them on. `source_kind` takes an Office file only when its bytes look like one and its
  extension is in `OFFICE_FILES` (no macro-enabled or template formats).
- Flatpak: no `--talk-name=org.freedesktop.Flatpak`, which upstream added so its update helper can
  run `flatpak-spawn --host`; that is a way out of the sandbox. `crates/ui-egui/tests/flatpak_updates.rs`
  checks the fork's 2 manifests for it. Flatpak users install a new bundle by hand.
- The redaction audit's glyph reading keeps text a clip hides: upstream's #3503 skips it in
  `FormWalk::walk` (`crates/edit/src/text/lines.rs`), which here applies only when
  `logical_text` is set, as the text is still in the file.
- Optimizer: Discard form actions and Discard external cross-references are upstream's
  (`form_actions.rs`, `external_refs.rs`). External cross-references then also runs this fork's
  sweep (`crates/optimize/src/actions.rs`), which removes `GoToR` and `Launch` actions upstream
  keeps: form fields, pages and the catalog's open action and triggers (`report.external_refs`).
  Remove PDF/A and PDF/X identification (`standards.rs`) is this fork's.
- Convert to gray (`doc_convert_gray`, `crates/optimize/src/colors.rs`) predates upstream's Convert
  colours (`doc_convert_colours`), which also converts to gray. Both are kept; retiring the fork's
  tool changes the automation interface, so it needs its own review.
- XFA's XML tree (`crates/xfa/src/xml.rs`, upstream's since it left roxmltree) keeps one copy of
  each element name and namespace (`intern`). With a copy for each element, merging a prototype
  with 20,000 children took 3.4 times the CPU time and failed
  `prototype_children_that_write_nothing_still_count_against_a_budget` under coverage.
- A 16-bit PNG becomes 8-bit samples in one buffer asked for fallibly (`png_image` in
  `crates/create/src/lib.rs`), so too little memory is an error, not an abort. Upstream copies the
  samples twice with allocations that can't fail: about 3 GiB more for a 2 MB file of 16384 × 16384
  pixels.
- Font metrics: a page's walk reads each font once by `FontKey` (`select_font` in
  `crates/edit/src/text/interp.rs`). Upstream's v0.21.1 shares metrics between aliases of one font
  object its own way (`by_font`, `METRICS_BUILT`); its test counts `FONT_READS` here.
- Editing existing Chinese, Japanese, Korean, Arabic and Hebrew text, checked by
  `crates/edit/src/script_edit_tests.rs`. The Type 3 fallback (`type3_faces` in
  `crates/edit/src/text/fallback.rs`) also holds the craft-fonts Simplified Chinese and Korean
  faces, the Chinese one first for text only it draws (`is_chinese`). A matching bundled CJK face
  completes a subset with outlines of the new characters, not embedded whole (`encodable_font` and
  `FallbackBuilder::prepare` in `crates/edit/src/text_regions/styled.rs`). An Arabic span keeps
  the first program of its face on the page, a subset's too, that shapes it and draws every glyph
  the shaping chose (`arabic_span` in `crates/edit/src/text_regions/styled/scripts.rs`,
  `draws_every_glyph` in `crates/fonts/src/source_shaping.rs`, which `shape_with_program` asks),
  shown in that font's own codes where they are glyph ids (`codes_are_glyph_ids`); other Arabic
  spans use the bundled Arabic outlines (`outline_glyph_resources` in
  `crates/edit/src/added/draw.rs`). Right-to-left styled paragraphs
  are right-aligned and a single row grows to the left (`paragraph_geometry`). `replace_line`
  refuses Arabic letters and keeps a right-to-left line's right edge (`right_edge_gap` in
  `crates/edit/src/text/rewrite.rs`). A line edit that needs Type 3 outlines keeps the line's font for the
  characters it draws, and the next edit reads the line back in that font (`adopt_source_font`). `japanese_replacement_missing_a_glyph_after_240_changes_nothing`
  picks a character no bundled face draws. Keep these at the next merge unless upstream covers the
  same pages.
- The control channel's transport tests live in
  `crates/ui-egui/src/control/transport/transport_tests.rs`, not inline in `transport.rs`, so
  CodeQL's test exclusion (`.github/codeql/codeql-config.yml`) covers them: their clients parse the
  server's replies, which `rust/unsafe-deserialization` flagged. Upstream's changes to those tests
  go into that file at the next merge.
- Windows first starts wgpu with Direct3D 12 alone, on a GPU that drives a display, when nothing
  chose the renderer or adapter (`dx12_first`, `hardware_dx12_only` and `dx12_alone_may_use` in
  `apps/pdfcraft/src/main.rs`): listing OpenGL adapters too starts an OpenGL context on every
  launch. If that run fails before the app starts, upstream's run (Direct3D 12 and OpenGL) and then
  eframe's OpenGL follow, and the `dx12-alone-failed` file in the settings folder skips the first
  run on later launches with the same display GPUs. Keep these runs at the next merge.

## Fixed differently upstream

The fork fixed these before upstream did. Upstream's `main` now has its own fix: at the next merge,
take upstream's side, keep only the fork's tests that upstream's don't cover, then delete the entry.

- Variation selectors stay with their character when text is edited (`fa90fd89`, #51). Upstream's
  `98b47619` (#4899, in v0.21.1) fixes the same in `crates/edit/src/text/fallback.rs`; both
  `is_variation_selector`s are still here, and `linebreak::between` keeps the fork's check.
- OCR loads the ONNX models, and an existing `.rten` pair, since ocrs withdrew the `.rten` files
  (`797d1356`, #17). Upstream's `crates/ocr/src/ocrs.rs` does the same.
- Import Data reads a CSV whose header holds a quoted tab as CSV (`472a0d99`, #23). Upstream's
  `crates/xfdf/src/lib.rs` has its own quoted-tab check. The same commit's Indexed colour space
  part is upstream's `4aad36cf1`, already merged.

## Merging upstream

No schedule sets when to merge. Upstream merges have landed 1 to 9 days apart since 30 October
2026 (#25, #32, #69, #136, #155), and `upstream-drift.yml` updates an 'Upstream drift report'
issue each Monday (cron 04:43 UTC) with how far `main` trails upstream.

1. `git merge --no-ff` upstream's `main` on a `ryan/` branch. Never rewrite history.
2. Run `cargo xtask fork-check`. It fails when the merge brings back an ArtCraft name, a
   `storytold/pdfcraft` reference in app code or packaging (other than `links::UPSTREAM` and
   `PDFCRAFT_NS`), an `UPDATE_APP` without the fork's
   `repo`, a named colour the baseline doesn't list, `@main` or `secrets: inherit` in a workflow, a
   crate without `publish = false`, a hash line without its `NOSONAR`, the Flatpak grant or an
   `ai.storyteller` Flatpak manifest. Once a build script reads
   `CRAFT_I18N_DIR`, it also fails when a `release.yml` build sets `CRAFT_FONTS_DIR` without
   `CRAFT_I18N_DIR` and `CRAFT_I18N_REQUIRED=1` beside it. When a flagged named colour colours a page
   rather than chrome, `cargo xtask fork-check --write` records it. Search by hand for
   trademark-licensed assets.
3. Run `cargo xtask ci`, which runs `fork-check` again. `cargo xtask assets --write` regenerates
   `ATTRIBUTION.md`.
4. A failure that also occurs on plain upstream isn't the merge's fault: check before patching it.
5. Update the commit above.
