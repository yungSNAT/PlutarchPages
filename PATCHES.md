# What this fork carries

PlutarchPages is upstream [PdfCraft](https://github.com/storytold/pdfcraft) plus the changes below.
[Fork changes](README.md#fork-changes) describes them for users; this file is for whoever merges
upstream next.

Last upstream merge: `5bb8bbf2` (8 November 2026).

## Stricter than upstream on purpose

Upstream's tests for these were changed to assert the fork's behaviour. Keep them that way.

- **Redaction** refuses when content under the marks is also used where the marks don't reach.
  Upstream goes ahead. Tests in `crates/redact` and `crates/automation`. This check runs before
  upstream's pattern guard, so 2 upstream tests in `crates/redact/tests/pattern_redaction.rs`
  expect its `StillUsed` refusal where both would refuse.
- **Copy-restricted documents** refuse text extraction through the tools, including `text_find`.
- **An empty password** never skips the Document Open prompt or lifts restrictions. `pdfcraft-cli
  check-one` checks before it renders.
- **CSV exports** write formula-like cells as text, and import doesn't strip the apostrophe, so a
  neutralised cell doesn't round-trip back into a formula.
- **Crypt filters** named by a reference that can't be read are never `/Identity`. Upstream reads
  the reference as absent, which saves the stream in the clear. Here a stream's `/DecodeParms` or
  `/Name`, or a `/Name` that `/CF` doesn't hold, takes the document's filter, and a save drops its
  `/Crypt`; `/StmF`, `/StrF`, `/EFF`, `/CF`
  or `/CFM` from `/V 4` on refuses the file, as does a selector naming a filter `/CF` doesn't hold
  or an `/Encrypt` and `/ID` referring to more than
  4,096 objects. `crypt_name` and `encrypt_dict` in `crates/cos/src/security.rs` and
  `encryption_members` in `crates/cos/src/document/open.rs` hold this, with tests in
  `crates/cos/tests/it/encryption.rs`. The cos
  part of upstream's `9203e4a6b` (#5548) came in ahead of the next merge; its hayro-syntax patch 30
  and renderer tests didn't.

## Only here

- The PDF-XChange-style converters, speech, bookmark, optimiser, colour and Excel features.
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
- Shared CI: workflows from `storytold/craft-common-actions` are pinned to a commit, and SonarCloud
  receives only `SONAR_TOKEN`. Upstream merges restore `@main` and `secrets: inherit`, so reapply both.
  `.github/workflows/house-rules.yml` isn't carried: it runs craft-common-actions at `@main` and
  checks `AGENTS.md` against craft-contrib's, which this fork's departs from on purpose. Delete it
  when a merge brings it back.
- Releases: `.github/workflows/release.yml` is this fork's own and uses nothing from
  `craft-common-actions`. It builds unsigned packages, tests an installed MSI and an upgrade from the
  latest published release with `packaging/windows/test-install.ps1`, and only drafts the release. Keep it over
  upstream's. Its FreeBSD, riscv64, web and Flatpak jobs are ported from that repository's
  `release.yml` at `2b3fb9dc`, without the FreeBSD Cargo cache action, and
  `packaging/linux/arch-package.sh` replaces its `arch-package` action. All are first included in the
  release after v0.15.0, and the `assets` job fails unless all 30 packages are there.
- Linux identity: the Flatpak's id is `io.github.EquityBot.PlutarchPages` and the Arch package is `plutarchpages`, so
  neither updates or replaces upstream's. If a merge brings back
  `packaging/linux/flatpak/ai.storyteller.pdfcraft*.yml`, move its changes across and delete it. The
  other packages still use `ai.storyteller.pdfcraft` and the `pdfcraft` package name, which v0.15.0
  shipped, so they can't be installed alongside upstream's either.
- `packaging/linux/nfpm.yaml` takes its glibc floors from `package.sh`, because riscv64 builds on Ubuntu
  24.04 and needs 2.39. Upstream hard-codes 2.35, which understates it there.
- Windows registration: `PlutarchPages.*` names and its own context-menu component GUIDs, so it doesn't
  share registry state with an upstream install. `pdfcraft.exe` stays, so `App Paths\pdfcraft.exe` and
  `Applications\pdfcraft.exe` are still shared: installing it alongside upstream isn't supported.
- CodeQL: `.github/codeql/codeql-config.yml` excludes tests and `vendor/`.
- SonarCloud: `.sonarcloud.properties` excludes `vendor/` for the automatic analysis, and 7 hash lines
  carry `NOSONAR` for rust:S4790: 6 MD5 lines in `crates/crypt/src/lib.rs` and the RIPEMD-160 line in
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
  text. The editor's `TextLine` keeps each run's rendering mode and `/OCR` marker (`run_modes`) and
  gives the line's opacity (`opacity`). A merge that changes `TextDevice`, `extract_parallel`,
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

## Ported ahead of the next merge

Security and robustness commits cherry-picked from upstream's `main` after `f137a289`. Each commit
names its source in a `cherry picked from` line. The next merge brings them in again: take
upstream's side except where this section says otherwise, then delete the entry.
`vendor/README.md` keeps upstream's patch numbers so that merge lines up, which leaves gaps:
hayro-jbig2 11, hayro-syntax 30 and 31, and hayro-interpret 73 aren't here.

- As upstream: `21a48ac735` (font discovery worker panic), `2289b5755d` (negative trailer `/Size`),
  `fdaa502f27` (`try_oid` arcs), `8faebd0d96` (`Matrix::invert`), `84a2005f` (an Indexed lookup
  table in an indirect string: hayro-interpret 71), `fa7acd3156` (a tuple keeps its references:
  hayro-syntax 32, hayro-interpret 72), `ae94b54505` (export reads `/Decode` through
  references, so it agrees with the renderer), `33d7a51301` (reference cycles), `036811368b` (both
  `/Decode` ends must be finite numbers to invert an export) and, apart from their rows in
  `vendor/README.md`, `85c1005148` (hayro-jbig2 patches 5 to 9, hayro-syntax 29), `10107d943a`
  (hayro-jbig2 12), `8243952569` (hayro-jbig2 10), `72e763ad` and `b7ec3669` (what patch 10 charges)
  and `e3bc4e85` (an empty height class). `vendor/hayro-jbig2/src` matches upstream at `b57ddb0a`.
- **Crop drags** (`aef42635d0`): `clamp_crop` in `crates/ui-egui/src/tools/edit_text_ui/image.rs`
  holds the fork's older clamps, which this crop code had before upstream's `apply_crop`: the far
  edge stays at least 6 px past the clamped near edge, where upstream clamps each edge against the
  image alone. It uses `min` and `max`, as rounding can invert the interval and `clamp` panics on
  that. Keep or drop that difference when `apply_crop` arrives, but keep the guards.
- Left out: `4696dd2f42` (`discover` already spends the budget before it opens a file; the fork
  keeps its limit of 4,096 entries rather than upstream's 500 files), `0ed339901e` (the allowance
  JBIG2 images of a page or an operation share; needs `ea6c3297`, `5480dbe1` and `39eb03a0`) and
  `801fd3aa67` (signature preview images: it calls `ImageBudget` and `image_file_within`, which
  `0ed339901e` introduces). Until those arrive, each JBIG2 stream has its own bound and a signature
  preview image has no smaller allowance.

## Merging upstream

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
