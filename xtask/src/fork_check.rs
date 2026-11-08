//! `cargo xtask fork-check [--write]`: what PATCHES.md says an upstream merge must not undo.
//! Each merge brings upstream's side of these back, so this runs after every merge and in
//! `cargo xtask ci`: Red Queen work, running to keep the fork in the same place. It fails when:
//! - a path names ArtCraft, or app code, packaging, `.github/` or the xtask names it other than as
//!   'the ArtCraft team' or 'an ArtCraft product' (comments and test modules in Rust are left alone);
//! - app code or packaging names `storytold/pdfcraft` anywhere but `links::UPSTREAM`;
//! - `UPDATE_APP` loses `repo: "yungSNAT/PlutarchPages"`, so updates would come from upstream's releases;
//! - interface code uses a named colour (`Color32::WHITE` and the like) on a line that
//!   `xtask/baselines/named-colours.txt` doesn't list (`--write` re-records it);
//! - a workflow uses `@main`, a `craft-common-actions` reference that isn't a commit, or
//!   `secrets: inherit`;
//! - a workspace crate lacks `publish = false`;
//! - a weak-hash line in `crates/crypt/src/lib.rs` or `crates/sign/src/keys.rs` lacks `NOSONAR`;
//! - packaging grants `xdg-data/ArtCraft`, or a Flatpak manifest named `ai.storyteller.*` is back;
//! - once a build script reads `CRAFT_I18N_DIR`, a place in `release.yml` that sets
//!   `CRAFT_FONTS_DIR` doesn't also set `CRAFT_I18N_DIR` and `CRAFT_I18N_REQUIRED=1`.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};

const COLOUR_BASELINE: &str = "xtask/baselines/named-colours.txt";
const RELEASE: &str = ".github/workflows/release.yml";
const WEAK_HASH_FILES: [&str; 2] = ["crates/crypt/src/lib.rs", "crates/sign/src/keys.rs"];
const UPDATE_APP_FILE: &str = "crates/ui-egui/src/lib.rs";
/// Its own rules name ArtCraft and upstream to look for them.
const THIS_FILE: &str = "xtask/src/fork_check.rs";

type Rule = fn(&Tree) -> Vec<String>;

const RULES: [(&str, Rule); 10] = [
    ("ArtCraft name", artcraft_names),
    ("upstream link", upstream_links),
    ("update source", update_source),
    ("named colour", named_colours),
    ("workflow", workflow_refs),
    ("publish", unpublished_crates),
    ("NOSONAR", weak_hash_markers),
    ("Flatpak grant", flatpak_grant),
    ("Flatpak manifest", upstream_flatpak_manifests),
    ("release i18n", release_i18n),
];

/// Every path in the repository, and the text of the files a rule reads.
pub struct Tree {
    paths: Vec<String>,
    text: BTreeMap<String, String>,
}

impl Tree {
    fn read(root: &Path, paths: Vec<String>) -> Result<Self> {
        let mut text = BTreeMap::new();
        for path in paths.iter().filter(|p| read_by_a_rule(p)) {
            let bytes = std::fs::read(root.join(path)).with_context(|| format!("reading {path}"))?;
            // Icons and test PDFs aren't text, and no rule reads them.
            if let Ok(s) = String::from_utf8(bytes) {
                text.insert(path.clone(), s);
            }
        }
        Ok(Self { paths, text })
    }

    fn get(&self, path: &str) -> Option<&str> {
        self.text.get(path).map(String::as_str)
    }

    fn under<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = (&'a str, &'a str)> {
        self.text.range(prefix.to_string()..).take_while(move |(p, _)| p.starts_with(prefix)).map(|(p, t)| (p.as_str(), t.as_str()))
    }
}

fn read_by_a_rule(path: &str) -> bool {
    !path.starts_with("vendor/")
        && (["crates/", "apps/", "packaging/", ".github/", "xtask/src/"].iter().any(|d| path.starts_with(d))
            || path.ends_with("Cargo.toml")
            || path == COLOUR_BASELINE)
}

pub fn run(args: &[String]) -> Result<()> {
    let root = crate::gates::root();
    let mut tree = Tree::read(&root, crate::assets::repo_files(&root)?)?;
    if args.iter().any(|a| a == "--write") {
        let baseline = colour_baseline(&tree);
        std::fs::write(root.join(COLOUR_BASELINE), &baseline)?;
        println!("fork-check: wrote {COLOUR_BASELINE}");
        tree.text.insert(COLOUR_BASELINE.into(), baseline);
    }
    let problems = check(&tree);
    if problems.is_empty() {
        let i18n = if i18n_is_a_build_input(&tree) { "" } else { "; release i18n waits for a build script that reads CRAFT_I18N_DIR" };
        println!("fork-check: ok ({} rules{i18n})", RULES.len());
        return Ok(());
    }
    for p in &problems {
        eprintln!("  ✗ {p}");
    }
    bail!("{} fork rule(s) broken; see PATCHES.md", problems.len())
}

fn check(tree: &Tree) -> Vec<String> {
    RULES.iter().flat_map(|(name, rule)| rule(tree).into_iter().map(move |p| format!("{name}: {p}"))).collect()
}

fn is_test(path: &str) -> bool {
    path.split('/').any(|c| c == "tests" || c == "tests.rs" || c.ends_with("_tests.rs"))
}

fn is_rust_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// A line for a message: trimmed, and cut short when it is long.
fn excerpt(line: &str) -> String {
    let line = line.trim();
    match line.char_indices().nth(120) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

/// Non-test Rust source under `crates/*/src` and `apps/*/src`.
fn app_code(tree: &Tree) -> impl Iterator<Item = (&str, &str)> {
    tree.text
        .iter()
        .filter(|(p, _)| (p.starts_with("crates/") || p.starts_with("apps/")) && p.contains("/src/") && p.ends_with(".rs") && !is_test(p))
        .map(|(p, t)| (p.as_str(), t.as_str()))
}

fn artcraft_names(tree: &Tree) -> Vec<String> {
    let mut out: Vec<String> =
        tree.paths.iter().filter(|p| p.to_ascii_lowercase().contains("artcraft")).map(|p| format!("{p}: the path names ArtCraft")).collect();
    let scanned = tree.text.iter().filter(|(p, _)| {
        let code = ["crates/", "apps/", "xtask/src/"].iter().any(|d| p.starts_with(d)) && !is_test(p) && *p != THIS_FILE;
        code || p.starts_with("packaging/") || p.starts_with(".github/")
    });
    for (path, text) in scanned {
        let rust = path.ends_with(".rs");
        let lines = if rust { lines_outside_test_mods(text) } else { text.lines().enumerate().map(|(i, l)| (i + 1, l)).collect() };
        for (n, line) in lines {
            let line = if path.ends_with(".tsv") {
                catalogue_source(line)
            } else if rust && is_rust_comment(line) {
                continue;
            } else {
                line
            };
            if names_artcraft(line) {
                out.push(format!("{path}:{n}: {}", excerpt(line)));
            }
        }
    }
    out
}

/// Each line of Rust source with its number from 1, leaving out `#[cfg(test)]` modules.
fn lines_outside_test_mods(text: &str) -> Vec<(usize, &str)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match test_mod_end(&lines, i) {
            Some(end) => i = end + 1,
            None => {
                out.push((i + 1, lines[i]));
                i += 1;
            }
        }
    }
    out
}

/// The last line of the `#[cfg(test)]` module whose attribute is line `i`, if it is one.
// shortcut: braces are counted without reading strings, characters or comments. A stray brace in
// a test module ends it early or makes it unbalanced, and then its lines are read like any other,
// which shows up as a false failure rather than a miss. Parse with `syn` if that happens.
fn test_mod_end(lines: &[&str], i: usize) -> Option<usize> {
    if lines[i].trim() != "#[cfg(test)]" {
        return None;
    }
    let item = i + 1 + lines[i + 1..].iter().take_while(|l| l.trim_start().starts_with("#[")).count();
    let head = lines.get(item)?.trim_start();
    if !["mod ", "pub mod ", "pub(crate) mod ", "pub(super) mod "].iter().any(|m| head.starts_with(m)) {
        return None;
    }
    let (mut depth, mut opened) = (0i32, false);
    for (k, line) in lines.iter().enumerate().skip(item) {
        if !opened && line.trim_end().ends_with(';') {
            return Some(k);
        }
        for c in line.chars() {
            match c {
                '{' => (depth, opened) = (depth + 1, true),
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return Some(k);
        }
    }
    None
}

/// A catalogue row's context and English source: a translation of an allowed source is allowed.
fn catalogue_source(line: &str) -> &str {
    line.match_indices('\t').nth(1).map_or(line, |(i, _)| &line[..i])
}

/// The fork names ArtCraft only to credit upstream's authors and say it isn't their product.
fn names_artcraft(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.match_indices("artcraft").any(|(i, _)| !["artcraft team", "artcraft product"].iter().any(|a| lower[i..].starts_with(a)))
}

fn upstream_links(tree: &Tree) -> Vec<String> {
    let mut out = Vec::new();
    for (path, text) in app_code(tree).chain(tree.under("packaging/")) {
        for (n, line) in text.lines().enumerate() {
            let credit = line.trim_start().starts_with("pub const UPSTREAM");
            if !credit && !is_rust_comment(line) && line.to_ascii_lowercase().contains("storytold/pdfcraft") {
                out.push(format!("{path}:{}: {} (point it at this fork)", n + 1, excerpt(line)));
            }
        }
    }
    out
}

/// Upstream's `UPDATE_APP` takes its repository from `craft_update::suite::PDFCRAFT`, so a merge
/// that drops the fork's `repo` line reads and downloads upstream's releases.
fn update_source(tree: &Tree) -> Vec<String> {
    match tree.get(UPDATE_APP_FILE) {
        Some(text) if text.lines().any(|l| l.trim() == "repo: \"yungSNAT/PlutarchPages\",") => Vec::new(),
        Some(_) => vec![format!("{UPDATE_APP_FILE}: `UPDATE_APP` lacks `repo: \"yungSNAT/PlutarchPages\"`, so updates would come from upstream")],
        None => vec![format!("{UPDATE_APP_FILE} is missing: find where `UPDATE_APP` went and keep its `repo`")],
    }
}

/// Lines in interface code, outside `theme.rs` and tests, that use a named `Color32` constant.
fn colour_lines(tree: &Tree) -> Vec<(&str, usize, &str)> {
    let mut out = Vec::new();
    for (path, text) in tree.under("crates/ui-egui/src/") {
        // The same files `chrome_literals_stay_in_the_theme` reads.
        if !path.ends_with(".rs") || path == "crates/ui-egui/src/theme.rs" || path.contains("tests") {
            continue;
        }
        for (n, line) in lines_outside_test_mods(text) {
            let named = line.match_indices("Color32::").any(|(i, m)| line[i + m.len()..].starts_with(|c: char| c.is_ascii_uppercase()));
            if named {
                out.push((path, n, line.trim()));
            }
        }
    }
    out
}

fn colour_baseline(tree: &Tree) -> String {
    let mut rows: Vec<String> = colour_lines(tree).into_iter().map(|(path, _, line)| format!("{path}\t{line}\n")).collect();
    rows.sort();
    let header = "\
# Named colours (Color32::WHITE and the like) in crates/ui-egui/src outside theme.rs and tests:
# one line per use, its path, a tab and the trimmed line. `cargo xtask fork-check` fails on a use
# not listed here, so an upstream merge can't put one on the chrome unnoticed (PATCHES.md).
# Check that a new one colours a page or document rather than the chrome, then run
# `cargo xtask fork-check --write`.
";
    header.to_string() + &rows.concat()
}

fn named_colours(tree: &Tree) -> Vec<String> {
    let Some(baseline) = tree.get(COLOUR_BASELINE) else {
        return vec![format!("{COLOUR_BASELINE} is missing: run `cargo xtask fork-check --write`")];
    };
    let mut listed: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for row in baseline.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        *listed.entry(row.split_once('\t').unwrap_or((row, ""))).or_default() += 1;
    }
    let mut out = Vec::new();
    for (path, n, line) in colour_lines(tree) {
        match listed.get_mut(&(path, line)) {
            Some(left) if *left > 0 => *left -= 1,
            _ => out.push(format!("{path}:{n}: {} isn't in {COLOUR_BASELINE}", excerpt(line))),
        }
    }
    out
}

fn workflow_refs(tree: &Tree) -> Vec<String> {
    let mut out = Vec::new();
    for (path, text) in tree.under(".github/workflows/") {
        for (n, line) in text.lines().enumerate() {
            let code = line.split(" #").next().unwrap_or_default().trim();
            let code = code.strip_prefix("- ").unwrap_or(code).trim();
            let Some((key, value)) = code.split_once(':') else { continue };
            let value = value.trim().trim_matches(['"', '\'']);
            let problem = match key.trim() {
                "uses" => {
                    let reference = value.rsplit_once('@').map(|(_, r)| r);
                    if reference == Some("main") {
                        "uses `@main`: pin it to a commit"
                    } else if value.starts_with("storytold/craft-common-actions") && !reference.is_some_and(is_commit) {
                        "craft-common-actions isn't pinned to a commit"
                    } else {
                        continue;
                    }
                }
                "secrets" if value == "inherit" => "`secrets: inherit`: pass only the secrets the workflow needs",
                _ => continue,
            };
            out.push(format!("{path}:{}: {problem}", n + 1));
        }
    }
    out
}

fn is_commit(reference: &str) -> bool {
    reference.len() == 40 && reference.bytes().all(|b| b.is_ascii_hexdigit())
}

fn unpublished_crates(tree: &Tree) -> Vec<String> {
    let members = match tree.get("Cargo.toml").map(str::parse::<toml::Table>) {
        None => return vec!["Cargo.toml is missing".into()],
        Some(Err(e)) => return vec![format!("Cargo.toml: {e}")],
        Some(Ok(t)) => t.get("workspace").and_then(|w| w.get("members")).and_then(toml::Value::as_array).cloned().unwrap_or_default(),
    };
    let mut manifests = Vec::new();
    let mut out = Vec::new();
    for member in members.iter().filter_map(toml::Value::as_str) {
        if let Some(dir) = member.strip_suffix("/*") {
            let in_dir = |p: &&String| {
                p.strip_prefix(dir).and_then(|r| r.strip_prefix('/')).and_then(|r| r.strip_suffix("/Cargo.toml")).is_some_and(|n| !n.contains('/'))
            };
            manifests.extend(tree.paths.iter().filter(in_dir).cloned());
        } else if member.contains('*') {
            out.push(format!("workspace member `{member}`: only `dir/*` patterns are understood"));
        } else {
            manifests.push(format!("{member}/Cargo.toml"));
        }
    }
    for manifest in manifests {
        let publish = tree.get(&manifest).and_then(|t| t.parse::<toml::Table>().ok()).and_then(|t| t.get("package")?.get("publish").cloned());
        if publish != Some(toml::Value::Boolean(false)) {
            out.push(format!("{manifest}: needs `publish = false` (crates.io names these crates for unrelated projects)"));
        }
    }
    out
}

fn weak_hash_markers(tree: &Tree) -> Vec<String> {
    let mut out = Vec::new();
    for path in WEAK_HASH_FILES {
        let Some(text) = tree.get(path) else {
            out.push(format!("{path} is missing: find where its MD5 or RIPEMD-160 lines went and mark them"));
            continue;
        };
        let hashes: Vec<(usize, &str)> = text.lines().enumerate().filter(|(_, l)| l.contains("Md5::") || l.contains("Ripemd160::")).collect();
        if hashes.is_empty() {
            out.push(format!("{path}: no MD5 or RIPEMD-160 line: find where they went and mark them"));
        }
        for (n, line) in hashes.into_iter().filter(|(_, l)| !l.contains("NOSONAR")) {
            out.push(format!("{path}:{}: {} has lost its NOSONAR marker (rust:S4790)", n + 1, excerpt(line)));
        }
    }
    out
}

fn flatpak_grant(tree: &Tree) -> Vec<String> {
    let mut out = Vec::new();
    for (path, text) in tree.under("packaging/") {
        for (n, line) in text.lines().enumerate().filter(|(_, l)| l.to_ascii_lowercase().contains("xdg-data/artcraft")) {
            out.push(format!("{path}:{}: {} (upstream's launcher grant)", n + 1, excerpt(line)));
        }
    }
    out
}

fn upstream_flatpak_manifests(tree: &Tree) -> Vec<String> {
    tree.paths
        .iter()
        .filter(|p| p.strip_prefix("packaging/linux/flatpak/").is_some_and(|name| name.starts_with("ai.storyteller.")))
        .map(|p| format!("{p}: upstream's manifest is back: move its changes into the fork's and delete it"))
        .collect()
}

fn i18n_is_a_build_input(tree: &Tree) -> bool {
    tree.text.iter().any(|(p, t)| (p == "build.rs" || p.ends_with("/build.rs")) && t.contains("CRAFT_I18N_DIR"))
}

/// Upstream's catalogues come from craft-i18n through `CRAFT_I18N_DIR`, and a build without it
/// is English only unless `CRAFT_I18N_REQUIRED=1`. Each place a release build gets its fonts
/// must get its translations too.
fn release_i18n(tree: &Tree) -> Vec<String> {
    if !i18n_is_a_build_input(tree) {
        return Vec::new();
    }
    let Some(text) = tree.get(RELEASE) else { return vec![format!("{RELEASE} is missing")] };
    let lines: Vec<&str> = text.lines().collect();
    let fonts: Vec<usize> = (0..lines.len()).filter(|&i| set_to(lines[i], "CRAFT_FONTS_DIR").is_some()).collect();
    if fonts.is_empty() {
        return vec![format!("{RELEASE}: nothing sets CRAFT_FONTS_DIR, so no build can be checked for CRAFT_I18N_DIR")];
    }
    let mut out = Vec::new();
    for i in fonts {
        let block = block_around(&lines, i);
        let dir = block.iter().any(|l| set_to(l, "CRAFT_I18N_DIR").is_some_and(|v| !v.is_empty()));
        let required = block.iter().any(|l| set_to(l, "CRAFT_I18N_REQUIRED") == Some("1"));
        if !(dir && required) {
            out.push(format!("{RELEASE}:{}: CRAFT_FONTS_DIR is set without CRAFT_I18N_DIR and CRAFT_I18N_REQUIRED=1 beside it", i + 1));
        }
    }
    out
}

/// The value a line gives `var` (`VAR: "x"` in YAML, `VAR=x` in a script), unquoted, up to a space.
fn set_to<'a>(line: &'a str, var: &str) -> Option<&'a str> {
    line.match_indices(var).find_map(|(i, _)| {
        let value = line[i + var.len()..].strip_prefix([':', '='])?.trim_start().trim_start_matches(['"', '\'']);
        value.split(|c: char| c == '"' || c == '\'' || c.is_whitespace()).next()
    })
}

/// The lines around `i` indented at least as far: its `env:` map, or the rest of its script.
fn block_around<'a>(lines: &[&'a str], i: usize) -> Vec<&'a str> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let inside = |l: &str| l.trim().is_empty() || indent(l) >= indent(lines[i]);
    let start = i - lines[..i].iter().rev().take_while(|l| inside(l)).count();
    let end = i + lines[i + 1..].iter().take_while(|l| inside(l)).count();
    lines[start..=end].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> Tree {
        Tree { paths: files.iter().map(|(p, _)| p.to_string()).collect(), text: files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect() }
    }

    fn passes(rule: Rule, files: &[(&str, &str)]) {
        let problems = rule(&tree(files));
        assert!(problems.is_empty(), "{problems:#?}");
    }

    fn fails(rule: Rule, files: &[(&str, &str)], expected: &str) {
        let problems = rule(&tree(files));
        assert!(problems.iter().any(|p| p.contains(expected)), "expected a problem naming {expected:?}, got {problems:#?}");
    }

    #[test]
    fn artcraft_is_named_only_to_credit_upstream() {
        let credit = [
            ("crates/ui-egui/src/home.rs", "label(tl!(\"Based on PdfCraft by the ArtCraft team.\"));"),
            ("crates/ui-egui/src/dialogs.rs", "tl!(\"PlutarchPages is not an ArtCraft product.\")"),
            ("crates/ui-egui/src/platform/updates.rs", "/// An updater that ignores ArtCraft Launcher's version choices."),
            ("crates/ui-egui/src/i18n/de.tsv", "\tBased on PdfCraft by the ArtCraft team.\tBasiert auf PdfCraft des ArtCraft-Teams."),
            ("crates/ui-egui/tests/it/links.rs", "assert_eq!(h.query_all_by_label(\"ArtCraft\").count(), 0);"),
            ("xtask/src/assets.rs", "fn f() {}\n#[cfg(test)]\nmod tests {\n    const MARK: &str = \"docs/brand/artcraft-mark.svg\";\n}\n"),
            (THIS_FILE, "if lower.contains(\"xdg-data/artcraft\") {}"),
        ];
        passes(artcraft_names, &credit);
        for (path, text) in [
            ("crates/ui-egui/src/home.rs", "widgets::artcraft_mark(ui, 28.0);"),
            ("crates/ui-egui/src/home.rs", "#[cfg(test)]\nmod tests {}\nfn f() { widgets::artcraft_mark(ui, 28.0); }"),
            ("crates/ui-egui/src/home.rs", "#[cfg(test)]\nuse crate::Harness;\nfn f() { widgets::artcraft_mark(ui, 28.0); }"),
            ("xtask/src/brand.rs", "const MARK: &str = \"docs/brand/artcraft-mark.svg\";"),
            (".github/workflows/release.yml", "      - run: echo \"Building ArtCraft PdfCraft\""),
            ("crates/engine/src/links.rs", "pub const DISCORD: &str = \"https://discord.gg/artcraft\";"),
            ("crates/engine/src/links.rs", "pub const WEBSITE: &str = \"https://getartcraft.com\";"),
            ("crates/ui-egui/src/i18n/de.tsv", "\tJoin the ArtCraft community\tTritt der ArtCraft-Community bei"),
            ("packaging/windows/install-policy.wxi", r#"<SetProperty Value="[%ProgramFiles]\ArtCraft\PdfCraft\" />"#),
            ("docs/brand/artcraft-mark.svg", ""),
        ] {
            let mut files = credit.to_vec();
            files.push((path, text));
            fails(artcraft_names, &files, path);
        }
    }

    #[test]
    fn app_code_links_to_upstream_only_as_its_credit() {
        let ok = [
            ("crates/engine/src/links.rs", "pub const UPSTREAM: &str = \"https://github.com/storytold/pdfcraft\";"),
            ("crates/engine/src/links/tests.rs", "assert_eq!(super::UPSTREAM, \"https://github.com/storytold/pdfcraft\");"),
            ("crates/render/src/raster.rs", "/// See https://github.com/storytold/pdfcraft/issues/623."),
        ];
        passes(upstream_links, &ok);
        let mut merged = ok.to_vec();
        merged
            .push(("crates/ui-egui/src/platform/updates.rs", "pub const RELEASES_PAGE: &str = \"https://github.com/storytold/pdfcraft/releases\";"));
        fails(upstream_links, &merged, "updates.rs:1");
        for (path, text) in [
            ("crates/ui-egui/src/lib.rs", "    repo: \"storytold/pdfcraft\","),
            ("packaging/linux/flatpak/io.github.Example.PlutarchPages.yml", "image: ghcr.io/storytold/pdfcraft:latest"),
        ] {
            let mut files = ok.to_vec();
            files.push((path, text));
            fails(upstream_links, &files, path);
        }
    }

    #[test]
    fn updates_come_from_this_fork() {
        let app =
            "pub const UPDATE_APP: App = App {\n    name: \"PlutarchPages\",\n    repo: \"yungSNAT/PlutarchPages\",\n    ..suite::PDFCRAFT\n};\n";
        passes(update_source, &[(UPDATE_APP_FILE, app)]);
        let merged = app.replace("    repo: \"yungSNAT/PlutarchPages\",\n", "");
        fails(update_source, &[(UPDATE_APP_FILE, merged.as_str())], "lacks `repo");
        fails(update_source, &[], "is missing");
    }

    #[test]
    fn a_named_colour_must_be_in_the_baseline() {
        // A test module's colours aren't listed; a colour after a test-only `use` is.
        let canvas = "#[cfg(test)]\nuse crate::Harness;\nfn page() {\n    painter.rect_filled(r, 0.0, Color32::WHITE);\n}\n#[cfg(test)]\nmod tests {\n    fn f() { let c = Color32::RED; }\n}\n";
        let others =
            [("crates/ui-egui/src/theme.rs", "pub const PAGE: Color32 = Color32::WHITE;"), ("crates/ui-egui/src/theme/tests.rs", "Color32::BLACK")];
        let mut files = vec![("crates/ui-egui/src/canvas.rs", canvas)];
        files.extend(others);
        let baseline = colour_baseline(&tree(&files));
        assert!(baseline.ends_with("crates/ui-egui/src/canvas.rs\tpainter.rect_filled(r, 0.0, Color32::WHITE);\n"), "{baseline}");
        assert!(!baseline.contains("Color32::RED"), "{baseline}");
        files.push((COLOUR_BASELINE, baseline.as_str()));
        passes(named_colours, &files);

        let moved = format!("\n\n{canvas}");
        let mut f = files.clone();
        f[0].1 = moved.as_str();
        passes(named_colours, &f);

        let again = format!("fn border() {{\n    painter.rect_filled(r, 0.0, Color32::WHITE);\n}}\n{canvas}");
        let mut f = files.clone();
        f[0].1 = again.as_str();
        fails(named_colours, &f, "canvas.rs:7");

        // Code after a test module is still read.
        let after = format!("{canvas}fn border() {{\n    painter.rect_filled(r, 0.0, Color32::BLACK);\n}}\n");
        let mut f = files.clone();
        f[0].1 = after.as_str();
        fails(named_colours, &f, "canvas.rs:11");

        let mut f = files.clone();
        f.push(("crates/ui-egui/src/chrome.rs", "ui.label(RichText::new(name).color(egui::Color32::GRAY));"));
        fails(named_colours, &f, "chrome.rs:1");

        fails(named_colours, &files[..1], "is missing");
    }

    #[test]
    fn shared_workflows_are_pinned_and_get_only_named_secrets() {
        let pinned = "jobs:\n  sonar:\n    uses: storytold/craft-common-actions/.github/workflows/sonarcloud.yml@2b3fb9dc05036f73c77d18f753a3aa9da55fa386 # main\n    secrets:\n      SONAR_TOKEN: ${{ secrets.SONAR_TOKEN }}\n    steps:\n      - uses: ./local-action\n";
        passes(workflow_refs, &[(".github/workflows/sonarcloud.yml", pinned)]);
        for (broken, expected) in [
            (pinned.replace("@2b3fb9dc05036f73c77d18f753a3aa9da55fa386", "@main"), "`@main`"),
            (pinned.replace("@2b3fb9dc05036f73c77d18f753a3aa9da55fa386", "@v1"), "isn't pinned"),
            (pinned.replace("secrets:\n      SONAR_TOKEN: ${{ secrets.SONAR_TOKEN }}", "secrets: inherit"), "secrets: inherit"),
            (pinned.replace("./local-action", "actions/checkout@main"), "`@main`"),
        ] {
            fails(workflow_refs, &[(".github/workflows/sonarcloud.yml", broken.as_str())], expected);
        }
    }

    #[test]
    fn every_workspace_crate_says_publish_false() {
        let workspace = "[workspace]\nmembers = [\"crates/*\", \"xtask\"]\nexclude = [\"vendor\"]\n";
        let unpublished = "[package]\nname = \"x\"\npublish = false\n";
        let ok = [
            ("Cargo.toml", workspace),
            ("crates/cos/Cargo.toml", unpublished),
            ("crates/cos/fuzz/Cargo.toml", "[package]\nname = \"not-a-member\"\n"),
            ("xtask/Cargo.toml", unpublished),
            ("vendor/lopdf/Cargo.toml", "[package]\nname = \"lopdf\"\n"),
        ];
        passes(unpublished_crates, &ok);
        let mut added = ok.to_vec();
        added.push(("crates/prc/Cargo.toml", "[package]\nname = \"pdfcraft-prc\"\n"));
        fails(unpublished_crates, &added, "crates/prc/Cargo.toml");
        let mut published = ok.to_vec();
        published[3].1 = "[package]\nname = \"xtask\"\npublish = true\n";
        fails(unpublished_crates, &published, "xtask/Cargo.toml");
    }

    #[test]
    fn weak_hash_lines_keep_nosonar() {
        let crypt = "use md5::{Digest as _, Md5};\nlet mut h = Md5::new(); // NOSONAR: ISO 32000 mandates MD5\n";
        let keys = "DigestAlg::Ripemd160 => {\n    let mut h = ripemd::Ripemd160::new(); // NOSONAR: existing signatures\n";
        passes(weak_hash_markers, &[(WEAK_HASH_FILES[0], crypt), (WEAK_HASH_FILES[1], keys)]);
        let merged = crypt.replace(" // NOSONAR: ISO 32000 mandates MD5", "");
        fails(weak_hash_markers, &[(WEAK_HASH_FILES[0], merged.as_str()), (WEAK_HASH_FILES[1], keys)], "lib.rs:2");
        let added = format!("{crypt}digest = Md5::digest(&digest).to_vec();\n");
        fails(weak_hash_markers, &[(WEAK_HASH_FILES[0], added.as_str()), (WEAK_HASH_FILES[1], keys)], "lib.rs:3");
        fails(weak_hash_markers, &[(WEAK_HASH_FILES[0], crypt)], "keys.rs is missing");
    }

    #[test]
    fn the_flatpak_has_no_artcraft_launcher_grant() {
        let manifest = "finish-args:\n  - --share=ipc\n  - --socket=wayland\n";
        let path = "packaging/linux/flatpak/io.github.Example.PlutarchPages.yml";
        passes(flatpak_grant, &[(path, manifest)]);
        let merged = format!("{manifest}  - --filesystem=xdg-data/ArtCraft/versions:ro\n");
        fails(flatpak_grant, &[(path, merged.as_str())], "PlutarchPages.yml:4");
    }

    #[test]
    fn upstream_flatpak_manifests_stay_deleted() {
        let fork = ("packaging/linux/flatpak/io.github.Example.PlutarchPages.yml", "");
        let desktop = ("packaging/linux/ai.storyteller.pdfcraft.desktop", "");
        passes(upstream_flatpak_manifests, &[fork, desktop]);
        for back in ["packaging/linux/flatpak/ai.storyteller.pdfcraft.yml", "packaging/linux/flatpak/ai.storyteller.pdfcraft.bundle.yml"] {
            fails(upstream_flatpak_manifests, &[fork, desktop, (back, "")], back);
        }
    }

    #[test]
    fn release_builds_require_their_translations_once_they_are_a_build_input() {
        let job = |extra: &str| {
            format!(
                "jobs:\n  linux:\n    env:\n      VERSION: x\n      CRAFT_FONTS_DIR: ${{{{ github.workspace }}}}/craft-fonts\n      CRAFT_FONTS_REQUIRED: \"1\"\n{extra}    steps:\n      - run: cargo build\n"
            )
        };
        let without = job("");
        let with = job("      CRAFT_I18N_DIR: ${{ github.workspace }}/craft-i18n\n      CRAFT_I18N_REQUIRED: \"1\"\n");
        let build = ("crates/ui-egui/build.rs", "let dir = std::env::var_os(\"CRAFT_I18N_DIR\");");
        passes(release_i18n, &[(RELEASE, without.as_str())]);
        passes(release_i18n, &[build, (RELEASE, with.as_str())]);
        fails(release_i18n, &[build, (RELEASE, without.as_str())], "release.yml:5");
        let optional = with.replace("CRAFT_I18N_REQUIRED: \"1\"", "CRAFT_I18N_REQUIRED: \"0\"");
        fails(release_i18n, &[build, (RELEASE, optional.as_str())], "release.yml:5");

        let script = "      - run: |\n          export CRAFT_FONTS_DIR=\"$PWD/craft-fonts\" CRAFT_FONTS_REQUIRED=1\n          bash package.sh\n";
        let both = script.replace("REQUIRED=1\n", "REQUIRED=1 CRAFT_I18N_DIR=\"$PWD/craft-i18n\" CRAFT_I18N_REQUIRED=1\n");
        passes(release_i18n, &[build, (RELEASE, format!("{with}  freebsd:\n    steps:\n{both}").as_str())]);
        fails(release_i18n, &[build, (RELEASE, format!("{with}  freebsd:\n    steps:\n{script}").as_str())], "release.yml:14");
        fails(release_i18n, &[build, (RELEASE, "jobs: {}\n")], "nothing sets CRAFT_FONTS_DIR");
    }

    #[test]
    fn check_names_the_rule_that_failed() {
        let problems = check(&tree(&[
            ("Cargo.toml", "[workspace]\nmembers = []\n"),
            (COLOUR_BASELINE, ""),
            ("packaging/linux/flatpak/ai.storyteller.pdfcraft.yml", ""),
        ]));
        assert!(problems.iter().any(|p| p.starts_with("Flatpak manifest: ")), "{problems:#?}");
        assert!(problems.iter().any(|p| p.starts_with("NOSONAR: ")), "{problems:#?}");
    }
}
