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
use std::ops::Range;
use std::path::Path;

use anyhow::{Context, Result, bail};

const COLOUR_BASELINE: &str = "xtask/baselines/named-colours.txt";
const RELEASE: &str = ".github/workflows/release.yml";
const WEAK_HASH_FILES: [&str; 2] = ["crates/crypt/src/lib.rs", "crates/sign/src/keys.rs"];
const UPDATE_APP_FILE: &str = "crates/ui-egui/src/lib.rs";
/// The value `UPDATE_APP` gives its `repo` field.
const FORK_REPO: &str = "\"yungSNAT/PlutarchPages\"";
/// `links::UPSTREAM`, which credits upstream: the one line in app code that may link to it.
const UPSTREAM_CREDIT: (&str, &str) = ("crates/engine/src/links.rs", "pub const UPSTREAM: &str = \"https://github.com/storytold/pdfcraft\";");
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
            || is_build_script(path)
            || path == COLOUR_BASELINE)
}

fn is_build_script(path: &str) -> bool {
    path == "build.rs" || path.ends_with("/build.rs")
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
        let lines = if path.ends_with(".rs") { rust_lines(text) } else { text.lines().enumerate().map(|(i, l)| (i + 1, l.to_string())).collect() };
        for (n, line) in lines {
            let line = if path.ends_with(".tsv") { catalogue_source(&line) } else { &line };
            if names_artcraft(line) {
                out.push(format!("{path}:{n}: {}", excerpt(line)));
            }
        }
    }
    out
}

/// Each line of Rust source with its number from 1, its comments and `#[cfg(test)]` modules
/// blanked. Source that doesn't lex, or whose braces don't balance, is read whole and as it is,
/// comments and test modules too: a false failure rather than a miss.
fn rust_lines(source: &str) -> Vec<(usize, String)> {
    let read = lex(source).filter(|l| balanced(&l.code)).and_then(|l| without_test_modules(&l));
    read.as_deref().unwrap_or(source).lines().enumerate().map(|(i, l)| (i + 1, l.to_string())).collect()
}

/// `lexed.text` with each `#[cfg(test)]` module blanked from its attribute to its closing brace
/// or semicolon, so code before or after it on the same line is still read.
fn without_test_modules(lexed: &Lexed) -> Option<String> {
    const ATTRIBUTE: &str = "#[cfg(test)]";
    let mut text = lexed.text.clone().into_bytes();
    for (start, _) in lexed.code.match_indices(ATTRIBUTE) {
        if let Some(end) = test_mod_end(lexed.code.as_bytes(), start + ATTRIBUTE.len()) {
            text[start..end].iter_mut().filter(|b| **b != b'\n').for_each(|b| *b = b' ');
        }
    }
    String::from_utf8(text).ok()
}

/// Whether every `{` closes and no `}` closes nothing.
fn balanced(code: &str) -> bool {
    code.bytes().try_fold(0usize, |depth, b| match b {
        b'{' => Some(depth + 1),
        b'}' => depth.checked_sub(1),
        _ => Some(depth),
    }) == Some(0)
}

/// When the item after a `#[cfg(test)]` that ends at byte `j` of `code` ([`Lexed::code`]) is a
/// module, the byte after its closing brace or semicolon.
fn test_mod_end(code: &[u8], j: usize) -> Option<usize> {
    let space = |j: usize| j + code[j..].iter().take_while(|b| b.is_ascii_whitespace()).count();
    let mut j = space(j);
    while code[j..].starts_with(b"#[") {
        j = space(group_end(code, j + 1)?);
    }
    if word_at(code, j, "pub") {
        j = space(j + "pub".len());
        if code.get(j) == Some(&b'(') {
            j = space(group_end(code, j)?);
        }
    }
    if !word_at(code, j, "mod") {
        return None;
    }
    j = space(j + "mod".len());
    let name = code[j..].iter().take_while(|&&b| is_word_byte(b)).count();
    j = space(j + name);
    match code.get(j)? {
        b';' if name > 0 => Some(j + 1),
        b'{' if name > 0 => group_end(code, j),
        _ => None,
    }
}

/// The byte after the bracket that closes the `(`, `[` or `{` at `open` in `code`.
fn group_end(code: &[u8], open: usize) -> Option<usize> {
    let close = match code[open] {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };
    let mut depth = 0usize;
    for (k, &b) in code.iter().enumerate().skip(open) {
        if b == code[open] {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(k + 1);
            }
        }
    }
    None
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || !b.is_ascii()
}

/// Whether the identifier or keyword `word`, whole, is at byte `i` of `code`.
fn word_at(code: &[u8], i: usize, word: &str) -> bool {
    code[i..].starts_with(word.as_bytes()) && starts_word(code, i) && !code.get(i + word.len()).is_some_and(|&b| is_word_byte(b))
}

#[derive(Clone, Copy, PartialEq)]
enum Lexeme {
    Code,
    Comment,
    Literal,
}

/// Rust source with its comments blanked (`text`), and with the insides of its literals blanked
/// too (`code`). Each blanked byte becomes a space and line breaks stay, so a line or byte offset
/// in either is the same line or offset in the source.
struct Lexed {
    text: String,
    code: String,
}

/// `None` when a comment or literal doesn't close, or a `'` starts neither a character nor a
/// lifetime: callers then read the source as it is.
fn lex(source: &str) -> Option<Lexed> {
    let s = source.as_bytes();
    let (mut text, mut code) = (Vec::with_capacity(s.len()), Vec::with_capacity(s.len()));
    let mut i = 0;
    while i < s.len() {
        let (lexeme, end) = lexeme_at(source, i)?;
        for &b in &s[i..end] {
            let blank = if b == b'\n' { b'\n' } else { b' ' };
            text.push(if lexeme == Lexeme::Comment { blank } else { b });
            code.push(if lexeme == Lexeme::Code { b } else { blank });
        }
        i = end;
    }
    Some(Lexed { text: String::from_utf8(text).ok()?, code: String::from_utf8(code).ok()? })
}

/// What starts at byte `i`, and the byte after it.
fn lexeme_at(source: &str, i: usize) -> Option<(Lexeme, usize)> {
    let s = source.as_bytes();
    match (s[i], s.get(i + 1)) {
        (b'/', Some(b'/')) => Some((Lexeme::Comment, s[i..].iter().position(|&b| b == b'\n').map_or(s.len(), |n| i + n))),
        (b'/', Some(b'*')) => Some((Lexeme::Comment, block_comment_end(s, i + 2)?)),
        (b'"', _) => Some((Lexeme::Literal, string_end(s, i + 1)?)),
        (b'\'', _) => quote(source, i),
        (b'r', _) if starts_word(s, i) || (i > 0 && matches!(s[i - 1], b'b' | b'c') && starts_word(s, i - 1)) => raw_string(s, i),
        _ => Some((Lexeme::Code, i + 1)),
    }
}

/// Whether byte `i` starts an identifier or keyword rather than continuing one.
fn starts_word(s: &[u8], i: usize) -> bool {
    i == 0 || !is_word_byte(s[i - 1])
}

/// The end of a block comment, which nests, whose text starts at `j`.
fn block_comment_end(s: &[u8], mut j: usize) -> Option<usize> {
    let mut depth = 1;
    while depth > 0 {
        match (*s.get(j)?, s.get(j + 1)) {
            (b'/', Some(b'*')) => (depth, j) = (depth + 1, j + 2),
            (b'*', Some(b'/')) => (depth, j) = (depth - 1, j + 2),
            _ => j += 1,
        }
    }
    Some(j)
}

/// The end of a string or byte string whose contents start at `j`.
fn string_end(s: &[u8], mut j: usize) -> Option<usize> {
    loop {
        match *s.get(j)? {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
}

/// `r"…"` or `r#"…"#` with any number of `#`s; an `r` that starts anything else is code.
fn raw_string(s: &[u8], i: usize) -> Option<(Lexeme, usize)> {
    let hashes = s[i + 1..].iter().take_while(|&&b| b == b'#').count();
    if s.get(i + 1 + hashes) != Some(&b'"') {
        return Some((Lexeme::Code, i + 1));
    }
    let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
    let start = i + 2 + hashes;
    let n = s[start..].windows(close.len()).position(|w| w == close)?;
    Some((Lexeme::Literal, start + n + close.len()))
}

/// A character literal (`'x'`, `'\''`, `'\u{7f}'`) or the `'` of a lifetime or label (`'a`).
fn quote(source: &str, i: usize) -> Option<(Lexeme, usize)> {
    let s = source.as_bytes();
    let c = source[i + 1..].chars().next()?;
    if c == '\\' {
        let rest = s.get(i + 3..)?;
        let n = rest.iter().position(|&b| b == b'\'' || b == b'\n')?;
        return (rest[n] == b'\'').then_some((Lexeme::Literal, i + 4 + n));
    }
    let after = i + 1 + c.len_utf8();
    if s.get(after) == Some(&b'\'') {
        return Some((Lexeme::Literal, after + 1));
    }
    (c.is_alphabetic() || c == '_').then_some((Lexeme::Code, i + 1))
}

/// The bytes between the braces of each `const` called `name`; `None` for one whose initialiser
/// isn't in braces.
fn initialisers(lexed: &Lexed, name: &str) -> Vec<Option<Range<usize>>> {
    let code = lexed.code.as_bytes();
    let declared = |i: usize| {
        let before = lexed.code[..i].trim_end();
        word_at(code, i, name) && before.ends_with("const") && starts_word(code, before.len() - "const".len())
    };
    let body = |i: usize| {
        let rest = &lexed.code[i..];
        let open = i + rest.find(['{', ';']).filter(|&n| rest.as_bytes()[n] == b'{')?;
        Some(open + 1..group_end(code, open)? - 1)
    };
    lexed.code.match_indices(name).map(|(i, _)| i).filter(|&i| declared(i)).map(body).collect()
}

/// The value, trimmed and with comments blanked, that a struct literal gives field `name` at its
/// own level, not in a value nested inside it; `body` is the bytes between its braces.
fn field<'a>(lexed: &'a Lexed, body: Range<usize>, name: &str) -> Option<&'a str> {
    let mut fields = Vec::new();
    let (mut depth, mut start) = (0i32, body.start);
    for k in body.clone() {
        match lexed.code.as_bytes()[k] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            b',' if depth == 0 => fields.push(std::mem::replace(&mut start, k + 1)..k),
            _ => {}
        }
    }
    fields.push(start..body.end);
    fields.into_iter().find_map(|f| {
        let (key, value) = lexed.code[f.clone()].split_once(':')?;
        (key.trim() == name && !value.starts_with(':')).then(|| lexed.text[f.start + key.len() + 1..f.end].trim())
    })
}

/// A catalogue row's context and English source: a translation of an allowed source is allowed.
fn catalogue_source(line: &str) -> &str {
    line.match_indices('\t').nth(1).map_or(line, |(i, _)| &line[..i])
}

/// The fork names ArtCraft only to credit upstream's authors and say it isn't their product, in
/// the whole phrases 'the ArtCraft team' and 'an ArtCraft product'.
fn names_artcraft(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let s = lower.as_bytes();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let credited = |i: usize| {
        let name_end = i + "artcraft".len();
        [("the ", " team"), ("an ", " product")].iter().any(|&(before, after)| {
            let Some(start) = i.checked_sub(before.len()) else { return false };
            s[start..i] == *before.as_bytes()
                && s[name_end..].starts_with(after.as_bytes())
                && !word(lower[..start].chars().next_back())
                && !word(lower[name_end + after.len()..].chars().next())
        })
    };
    lower.match_indices("artcraft").any(|(i, _)| !credited(i))
}

fn upstream_links(tree: &Tree) -> Vec<String> {
    // The credit's code with its string blanked, so a string holding the same line isn't it.
    let credit_code = lex(UPSTREAM_CREDIT.1).map(|l| l.code);
    let credit_code = credit_code.as_deref().map(str::trim);
    let mut out = Vec::new();
    for (path, text) in app_code(tree).chain(tree.under("packaging/")) {
        let lexed = if path.ends_with(".rs") { lex(text) } else { None };
        let code: Vec<&str> = lexed.as_ref().map(|l| l.code.lines().collect()).unwrap_or_default();
        for (n, line) in lexed.as_ref().map_or(text, |l| l.text.as_str()).lines().enumerate() {
            let credit = (path, line.trim()) == UPSTREAM_CREDIT && code.get(n).map(|c| c.trim()) == credit_code;
            if !credit && line.to_ascii_lowercase().contains("storytold/pdfcraft") {
                out.push(format!("{path}:{}: {} (point it at this fork)", n + 1, excerpt(line)));
            }
        }
    }
    out
}

/// Upstream's `UPDATE_APP` takes its repository from `craft_update::suite::PDFCRAFT`, so a merge
/// that drops the fork's `repo` field reads and downloads upstream's releases. Each `UPDATE_APP`
/// initialiser needs the field itself.
fn update_source(tree: &Tree) -> Vec<String> {
    let Some(text) = tree.get(UPDATE_APP_FILE) else {
        return vec![format!("{UPDATE_APP_FILE} is missing: find where `UPDATE_APP` went and keep its `repo`")];
    };
    if let Some(lexed) = lex(text) {
        let bodies = initialisers(&lexed, "UPDATE_APP");
        if !bodies.is_empty() && bodies.into_iter().all(|b| b.is_some_and(|b| field(&lexed, b, "repo") == Some(FORK_REPO))) {
            return Vec::new();
        }
    }
    vec![format!("{UPDATE_APP_FILE}: `UPDATE_APP` lacks `repo: {FORK_REPO}`, so updates would come from upstream")]
}

/// Lines in interface code, outside `theme.rs` and tests, that use a named `Color32` constant.
fn colour_lines(tree: &Tree) -> Vec<(&str, usize, String)> {
    let mut out = Vec::new();
    for (path, text) in tree.under("crates/ui-egui/src/") {
        // The same files `chrome_literals_stay_in_the_theme` reads.
        if !path.ends_with(".rs") || path == "crates/ui-egui/src/theme.rs" || path.contains("tests") {
            continue;
        }
        for (n, line) in rust_lines(text) {
            let named = line.match_indices("Color32::").any(|(i, m)| line[i + m.len()..].starts_with(|c: char| c.is_ascii_uppercase()));
            if named {
                out.push((path, n, line.trim().to_string()));
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
    let lines = colour_lines(tree);
    let mut listed: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for row in baseline.lines().filter(|l| !l.starts_with('#') && !l.is_empty()) {
        *listed.entry(row.split_once('\t').unwrap_or((row, ""))).or_default() += 1;
    }
    let mut out = Vec::new();
    for (path, n, line) in &lines {
        match listed.get_mut(&(*path, line.as_str())) {
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
    tree.text.iter().any(|(p, t)| is_build_script(p) && t.contains("CRAFT_I18N_DIR"))
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

    /// A tree that, like `Tree::read`, holds the text only of the files a rule reads.
    fn tree(files: &[(&str, &str)]) -> Tree {
        Tree {
            paths: files.iter().map(|(p, _)| p.to_string()).collect(),
            text: files.iter().filter(|(p, _)| read_by_a_rule(p)).map(|(p, t)| (p.to_string(), t.to_string())).collect(),
        }
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

    #[test]
    fn a_test_module_ends_at_its_own_closing_brace() {
        let home = "crates/ui-egui/src/home.rs";
        // Braces in comments and literals neither end a test module nor keep it open.
        let tests = r##"fn f() {}
#[cfg(test)]
mod tests {
    // }
    /* } /* } */ } */
    const A: &str = "{\"}";
    const B: &str = r#"}"#;
    const C: &[u8] = br"}";
    const D: &[u8] = b"}";
    const E: char = '}';
    const F: char = '\'';
    const G: char = '\u{7d}';
    fn g<'a>(s: &'a str) -> &'a str { 'outer: loop { break 'outer s; } }
    const MARK: &str = "docs/brand/artcraft-mark.svg";
}
"##;
        passes(artcraft_names, &[(home, tests)]);
        // A `{` in a test module and a `}` in the function after it don't hide the function.
        let hidden = "#[cfg(test)]\nmod tests {\n    const OPEN: &str = \"{\";\n}\nfn f() {\n    let close = \"}\";\n    widgets::artcraft_mark(ui, 28.0);\n}\n";
        fails(artcraft_names, &[(home, hidden)], "home.rs:7");
        // Code after a test module's closing brace on the same line is read.
        let after = "#[cfg(test)]\nmod tests {} fn production() { widgets::artcraft_mark(ui, 28.0); }\n";
        fails(artcraft_names, &[(home, after)], "home.rs:2");
        let attributes = "#[cfg(test)]\n#[allow(dead_code)]\npub(crate) mod tests {\n    const MARK: &str = \"artcraft-mark.svg\";\n}\n";
        passes(artcraft_names, &[(home, attributes)]);
        fails(artcraft_names, &[(home, "#[cfg(test)]\nmod tests;\nfn f() { widgets::artcraft_mark(ui, 28.0); }\n")], "home.rs:3");
        // Source that doesn't lex or balance is read whole, test modules too.
        let module = "#[cfg(test)]\nmod tests {\n    const MARK: &str = \"artcraft-mark.svg\";\n}\n";
        for unread in ["const S: &str = \"unclosed;\n", "/* unclosed\n", "const R: &str = r#\"unclosed\"\n", "const Q: u8 = '{x;\n", "}\n"] {
            fails(artcraft_names, &[(home, format!("{module}{unread}").as_str())], "home.rs:3");
        }
    }

    #[test]
    fn rust_comments_are_left_alone_wherever_they_are() {
        let home = "crates/ui-egui/src/home.rs";
        passes(artcraft_names, &[(home, "fn f() { g(); /* ArtCraft */ }\n/*\n * ArtCraft Launcher\n */\nfn h() {} // ArtCraft\n")]);
        fails(artcraft_names, &[(home, "fn f() { g(\"/* ArtCraft */\"); }\n")], "home.rs:1");
        let raster = "crates/render/src/raster.rs";
        passes(upstream_links, &[(raster, "fn f() {} /* storytold/pdfcraft#623 */\n")]);
        fails(upstream_links, &[(raster, "const URL: &str = r\"\n// https://github.com/storytold/pdfcraft\n\";\n")], "raster.rs:2");
    }

    #[test]
    fn only_update_app_sets_the_update_repo() {
        let other = "pub const UPDATE_APP: App = App {\n    name: \"PlutarchPages\",\n    ..suite::PDFCRAFT\n};\npub const OTHER_APP: App = App {\n    repo: \"yungSNAT/PlutarchPages\",\n    ..suite::PDFCRAFT\n};\n";
        fails(update_source, &[(UPDATE_APP_FILE, other)], "lacks `repo");
        let similar = "pub const UPDATE_APP_V2: App = App {\n    repo: \"yungSNAT/PlutarchPages\",\n};\npub const UPDATE_APP: App = App { ..suite::PDFCRAFT };\n";
        fails(update_source, &[(UPDATE_APP_FILE, similar)], "lacks `repo");
        let commented = "pub const UPDATE_APP: App = App {\n    /*\n    repo: \"yungSNAT/PlutarchPages\",\n    */\n    ..suite::PDFCRAFT\n};\n";
        fails(update_source, &[(UPDATE_APP_FILE, commented)], "lacks `repo");
        let second = "pub const OTHER_APP: App = App {\n    name: \"x\",\n};\n#[cfg(not(target_arch = \"wasm32\"))]\npub const UPDATE_APP: App = App {\n    repo: \"yungSNAT/PlutarchPages\",\n    ..suite::PDFCRAFT\n};\n";
        passes(update_source, &[(UPDATE_APP_FILE, second)]);
        // A `repo` line in a string, or in a value nested in the initialiser, isn't its field.
        let in_string = r##"pub const UPDATE_APP: App = App {
    name: r#"PlutarchPages
    repo: "yungSNAT/PlutarchPages",
"#,
    ..suite::PDFCRAFT
};
"##;
        fails(update_source, &[(UPDATE_APP_FILE, in_string)], "lacks `repo");
        let nested = "pub const UPDATE_APP: App = App {\n    name: pick(App {\n        repo: \"yungSNAT/PlutarchPages\",\n    }),\n    ..suite::PDFCRAFT\n};\n";
        fails(update_source, &[(UPDATE_APP_FILE, nested)], "lacks `repo");
    }

    #[test]
    fn the_update_repo_is_read_however_the_initialiser_is_laid_out() {
        let split = "pub const\nUPDATE_APP: App = App {\n    repo: \"yungSNAT/PlutarchPages\",\n    ..suite::PDFCRAFT\n};\n";
        passes(update_source, &[(UPDATE_APP_FILE, split)]);
        let inline = "pub const UPDATE_APP: App = App { repo: \"yungSNAT/PlutarchPages\", ..suite::PDFCRAFT };\n";
        passes(update_source, &[(UPDATE_APP_FILE, inline)]);
    }

    #[test]
    fn only_links_upstream_credits_upstream() {
        let links = "pub const UPSTREAM: &str = \"https://github.com/storytold/pdfcraft\";\npub const UPSTREAM_RELEASES: &str = \"https://github.com/storytold/pdfcraft/releases\";\n";
        fails(upstream_links, &[("crates/engine/src/links.rs", links)], "links.rs:2");
        fails(
            upstream_links,
            &[("crates/ui-egui/src/about.rs", "pub const UPSTREAM: &str = \"https://github.com/storytold/pdfcraft\";")],
            "about.rs:1",
        );
        // A string holding the credit's line isn't the credit.
        let in_string = r##"pub const NOTES: &str = r#"
pub const UPSTREAM: &str = "https://github.com/storytold/pdfcraft";
"#;
"##;
        fails(upstream_links, &[("crates/engine/src/links.rs", in_string)], "links.rs:2");
    }

    #[test]
    fn artcraft_credits_are_whole_phrases() {
        let home = "crates/ui-egui/src/home.rs";
        passes(
            artcraft_names,
            &[(
                home,
                "line(format_args!(\"Upstream: {}  (PdfCraft by the ArtCraft team)\", UPSTREAM));\nlabel(tl!(\"A fork of PdfCraft by the ArtCraft team. PlutarchPages is not an ArtCraft product.\"));\n",
            )],
        );
        for slogan in
            ["Buy ArtCraft products", "Meet the ArtCraft teams", "Breathe ArtCraft team spirit", "Join an ArtCraft production", "Ask ArtCraft team"]
        {
            fails(artcraft_names, &[(home, format!("label(tl!(\"{slogan}\"));").as_str())], "home.rs:1");
        }
    }

    #[test]
    fn a_build_script_anywhere_makes_translations_a_release_input() {
        let release = (RELEASE, "jobs:\n  linux:\n    env:\n      CRAFT_FONTS_DIR: fonts\n");
        for build in ["build.rs", "xtask/build.rs"] {
            fails(release_i18n, &[(build, "println!(\"cargo::rerun-if-env-changed=CRAFT_I18N_DIR\");"), release], "release.yml:4");
        }
    }
}
