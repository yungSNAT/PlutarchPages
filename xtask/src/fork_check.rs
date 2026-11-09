//! `cargo xtask fork-check [--write]`: what PATCHES.md says an upstream merge must not undo.
//! Each merge brings upstream's side of these back, so this runs after every merge and in
//! `cargo xtask ci`: Red Queen work, running to keep the fork in the same place. It fails when:
//! - a path names ArtCraft, or app code, packaging, `.github/` or the xtask names it other than as
//!   'the ArtCraft team' or 'an ArtCraft product' (comments and test modules in Rust are left alone);
//! - app code or packaging names `storytold/pdfcraft` anywhere but `links::UPSTREAM` and the XFDF
//!   namespace `PDFCRAFT_NS`;
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

use std::borrow::Cow;
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
/// The lines in app code that may name upstream: `links::UPSTREAM`, which credits it, and
/// `PDFCRAFT_NS`, the XFDF namespace of PdfCraft's own attributes, kept so XFDF files interchange.
const UPSTREAM_KEPT: [(&str, &str); 2] = [
    ("crates/engine/src/links.rs", "pub const UPSTREAM: &str = \"https://github.com/storytold/pdfcraft\";"),
    ("crates/xfdf/src/xfdf.rs", "pub(crate) const PDFCRAFT_NS: &str = \"https://github.com/storytold/pdfcraft#xfdf\";"),
];
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
    workspace_manifests: Vec<String>,
}

impl Tree {
    fn read(root: &Path, checkout: &Path, paths: Vec<String>) -> Result<Self> {
        // Cargo diagnostics may quote manifest text, including from a symlink.
        let metadata = crate::gates::metadata().map_err(|_| anyhow::anyhow!("cargo metadata failed; check the workspace manifests locally"))?;
        // With --no-deps, Cargo reports only workspace members, including implicit path members.
        let workspace_manifests = metadata["packages"]
            .as_array()
            .context("cargo metadata did not list workspace packages")?
            .iter()
            .map(|package| {
                let manifest = package["manifest_path"].as_str().context("workspace package has no manifest path")?;
                let relative =
                    Path::new(manifest).strip_prefix(root).with_context(|| format!("workspace manifest is outside the repository: {manifest}"))?;
                Ok(relative.to_str().context("workspace manifest path is not UTF-8")?.replace('\\', "/"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut text = BTreeMap::new();
        for path in paths.iter().filter(|p| read_by_a_rule(p)).chain(&workspace_manifests) {
            if text.contains_key(path) {
                continue;
            }
            let file = checkout.join(path).canonicalize().with_context(|| format!("locating {path}"))?;
            if !file.starts_with(checkout) || !file.is_file() {
                bail!("{path}: expected a regular file inside the repository");
            }
            let bytes = std::fs::read(file).with_context(|| format!("reading {path}"))?;
            // Icons and test PDFs aren't text, and no rule reads them.
            if let Ok(s) = String::from_utf8(bytes) {
                text.insert(path.clone(), s);
            }
        }
        Ok(Self { paths, text, workspace_manifests })
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
    let checkout = root.canonicalize().context("locating the repository root")?;
    let mut tree = Tree::read(&root, &checkout, crate::assets::repo_files(&root)?)?;
    if args.iter().any(|a| a == "--write") {
        let baseline = colour_baseline(&tree);
        write_baseline(&checkout, &baseline)?;
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

fn write_baseline(checkout: &Path, baseline: &str) -> Result<()> {
    let path = checkout.join(COLOUR_BASELINE);
    let parent = path.parent().context("the colour baseline needs a parent directory")?.canonicalize()?;
    if !parent.starts_with(checkout) {
        bail!("{COLOUR_BASELINE}: expected a parent directory inside the repository");
    }
    let file = parent.join(path.file_name().context("the colour baseline needs a file name")?);
    match file.symlink_metadata() {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => bail!("{COLOUR_BASELINE}: expected a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context(format!("checking {COLOUR_BASELINE}")),
    }
    std::fs::write(file, baseline).with_context(|| format!("writing {COLOUR_BASELINE}"))
}

fn check(tree: &Tree) -> Vec<String> {
    RULES.iter().flat_map(|(name, rule)| rule(tree).into_iter().map(move |p| format!("{name}: {p}"))).collect()
}

/// Whether `path` is test code: named for tests and outside `src/`, as integration tests are, or
/// named for tests in `src/` and declared only for them ([`declared_for_tests`]).
fn is_test(tree: &Tree, path: &str) -> bool {
    let named = path.split('/').any(|c| c == "tests" || c == "tests.rs" || c.ends_with("_tests.rs"));
    named && (!path.contains("/src/") || declared_for_tests(tree, path))
}

/// Whether each `mod` that declares the source file `path` in its parent file is in a test module,
/// or the parent is test code itself. A file only `#[path]` declares, or whose parent doesn't lex,
/// isn't, so it is read.
fn declared_for_tests(tree: &Tree, path: &str) -> bool {
    let Some((dir, file)) = path.rsplit_once('/') else { return false };
    let module = if file == "mod.rs" { dir.rsplit_once('/') } else { file.strip_suffix(".rs").map(|name| (dir, name)) };
    let Some((dir, name)) = module else { return false };
    let parents =
        if dir.ends_with("/src") { [format!("{dir}/lib.rs"), format!("{dir}/main.rs")] } else { [format!("{dir}.rs"), format!("{dir}/mod.rs")] };
    let mut declared = false;
    for parent in parents {
        let Some(text) = tree.get(&parent) else { continue };
        let Some(lexed) = lex(text) else { return false };
        let Some(level) = item_level(&lexed.code, true) else { return false };
        let code = lexed.code.as_str();
        let declares = |i: usize| {
            let after = code[i + "mod".len()..].trim_start_matches(is_space);
            found_at(code, i, "mod") && found_at(code, code.len() - after.len(), name)
        };
        let declarations: Vec<usize> = code.match_indices("mod").map(|(i, _)| i).filter(|&i| declares(i)).collect();
        if declarations.is_empty() {
            continue;
        }
        declared = true;
        let tests = test_modules(code, &level);
        if !is_test(tree, &parent) && !declarations.iter().all(|d| tests.iter().any(|t| t.contains(d))) {
            return false;
        }
    }
    declared
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
        .filter(|(p, _)| (p.starts_with("crates/") || p.starts_with("apps/")) && p.contains("/src/") && p.ends_with(".rs") && !is_test(tree, p))
        .map(|(p, t)| (p.as_str(), t.as_str()))
}

fn artcraft_names(tree: &Tree) -> Vec<String> {
    let mut out: Vec<String> =
        tree.paths.iter().filter(|p| p.to_ascii_lowercase().contains("artcraft")).map(|p| format!("{p}: the path names ArtCraft")).collect();
    let scanned = tree.text.iter().filter(|(p, _)| {
        let code = ["crates/", "apps/", "xtask/src/"].iter().any(|d| p.starts_with(d)) && !is_test(tree, p) && *p != THIS_FILE;
        code || p.starts_with("packaging/") || p.starts_with(".github/")
    });
    for (path, text) in scanned {
        let text = if path.ends_with(".rs") { production(text) } else { Cow::Borrowed(text.as_str()) };
        for (n, line) in text.lines().enumerate() {
            let line = if path.ends_with(".tsv") { catalogue_source(line) } else { line };
            if names_artcraft(line) {
                out.push(format!("{path}:{}: {}", n + 1, excerpt(line)));
            }
        }
    }
    out
}

/// Rust source with its comments and test modules blanked, each from its `#[cfg(test)]` to its
/// closing brace or semicolon, so code before or after it on the same line is still read. Source
/// that doesn't lex, or whose brackets don't pair up, is read whole and as it is, comments and
/// test modules too: a false failure rather than a miss.
fn production(source: &str) -> Cow<'_, str> {
    let blanked = lex(source).and_then(|lexed| {
        let tests = test_modules(&lexed.code, &item_level(&lexed.code, true)?);
        let mut text = lexed.text.into_bytes();
        for t in tests {
            text[t].iter_mut().filter(|b| **b != b'\n').for_each(|b| *b = b' ');
        }
        String::from_utf8(text).ok()
    });
    blanked.map_or(Cow::Borrowed(source), Cow::Owned)
}

/// The bytes of each test module in `code` ([`Lexed::code`]), from its `#[cfg(test)]` to its
/// closing brace or semicolon. Only at item level (`level`, from [`item_level`]) does the
/// attribute certainly gate a module: in a macro's tokens it may be rewritten.
fn test_modules(code: &str, level: &[bool]) -> Vec<Range<usize>> {
    const ATTRIBUTE: &str = "#[cfg(test)]";
    code.match_indices(ATTRIBUTE)
        .filter(|&(i, _)| level[i])
        .filter_map(|(i, attribute)| Some(i..test_mod_end(code.as_bytes(), i + attribute.len())?))
        .collect()
}

/// For each byte of `code` ([`Lexed::code`]), whether it is outside brackets. When
/// `allow_modules` is set, the braces of a `mod` also contain items. In a function body or a
/// macro's tokens, a `#[cfg(test)]` or a `const` may not be what it seems.
/// `None` when the brackets don't pair up.
fn item_level(code: &str, allow_modules: bool) -> Option<Vec<bool>> {
    let (mut open, mut others, mut level) = (Vec::new(), 0usize, Vec::with_capacity(code.len()));
    for (k, b) in code.bytes().enumerate() {
        match b {
            b'{' | b'(' | b'[' => {
                level.push(others == 0);
                let module = allow_modules && b == b'{' && opens_a_module(code, k);
                others += usize::from(!module);
                open.push((b, module));
            }
            b'}' | b')' | b']' => {
                let (opener, module) = open.pop()?;
                if !matches!((opener, b), (b'{', b'}') | (b'(', b')') | (b'[', b']')) {
                    return None;
                }
                others -= usize::from(!module);
                level.push(others == 0);
            }
            _ => level.push(others == 0),
        }
    }
    open.is_empty().then_some(level)
}

/// Whether the `{` at byte `k` of `code` opens a module's items: `mod` and its name come before it.
fn opens_a_module(code: &str, k: usize) -> bool {
    let before = code[..k].trim_end_matches(|c: char| c.is_ascii_whitespace());
    let name = before.trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '_');
    let keyword = name.trim_end_matches(|c: char| c.is_ascii_whitespace());
    name.len() < before.len() && keyword.len() < name.len() && keyword.ends_with("mod") && starts_word(code.as_bytes(), keyword.len() - "mod".len())
}

/// Rust's whitespace, and more besides.
fn is_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{200e}' | '\u{200f}')
}

/// Whether `word` is at byte `i` of `code` with no ASCII letter, digit or `_` either side: where a
/// rule looks for something, a character outside ASCII beside it doesn't hide it.
fn found_at(code: &str, i: usize, word: &str) -> bool {
    let ident = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    code[i..].starts_with(word) && !ident(code[..i].chars().next_back()) && !ident(code[i + word.len()..].chars().next())
}

/// Each place `ident` starts a path in `code` ([`Lexed::code`]): its offset, and the offset of
/// what follows its `::`. Whitespace may come between them, comments too as they are blanks.
fn path_heads<'a>(code: &'a str, ident: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
    code.match_indices(ident).filter(move |&(i, _)| found_at(code, i, ident)).filter_map(move |(i, _)| {
        let rest = code[i + ident.len()..].trim_start_matches(is_space).strip_prefix("::")?;
        Some((i, code.len() - rest.trim_start_matches(is_space).len()))
    })
}

/// The line, from 0, that byte `i` of `text` is on.
fn line_index(text: &str, i: usize) -> usize {
    text[..i].matches('\n').count()
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

/// Each `const` called `name`: the offset of its name, and the bytes between the braces of its
/// initialiser (`None` when that isn't in braces).
fn initialisers(lexed: &Lexed, name: &str) -> Vec<(usize, Option<Range<usize>>)> {
    let code = lexed.code.as_str();
    let declared = |i: usize| {
        let before = code[..i].trim_end_matches(is_space).strip_suffix("const");
        found_at(code, i, name) && before.is_some_and(|b| found_at(code, b.len(), "const"))
    };
    let body = |i: usize| {
        let rest = &code[i..];
        let open = i + rest.find(['{', ';']).filter(|&n| rest.as_bytes()[n] == b'{')?;
        Some(open + 1..group_end(code.as_bytes(), open)? - 1)
    };
    code.match_indices(name).map(|(i, _)| i).filter(|&i| declared(i)).map(|i| (i, body(i))).collect()
}

/// Whether what comes before the item named at byte `i` of `code`, back to the item before it or
/// the brace that opens its module, mentions `test`, as `#[cfg(test)]` on the item would.
fn mentions_test(code: &str, level: &[bool], i: usize) -> bool {
    let start = (0..i).rev().find(|&k| level[k] && matches!(code.as_bytes()[k], b';' | b'{' | b'}')).map_or(0, |k| k + 1);
    code[start..i].match_indices("test").any(|(k, _)| found_at(code, start + k, "test"))
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
    // Each kept line's code with its string blanked, so a string holding the same line isn't it.
    let kept: Vec<(&str, &str, Option<String>)> = UPSTREAM_KEPT.iter().map(|&(p, l)| (p, l, lex(l).map(|x| x.code))).collect();
    let mut out = Vec::new();
    for (path, text) in app_code(tree).chain(tree.under("packaging/")) {
        let lexed = if path.ends_with(".rs") { lex(text) } else { None };
        let code: Vec<&str> = lexed.as_ref().map(|l| l.code.lines().collect()).unwrap_or_default();
        for (n, line) in lexed.as_ref().map_or(text, |l| l.text.as_str()).lines().enumerate() {
            let is_kept =
                kept.iter().any(|(p, l, c)| (path, line.trim()) == (*p, *l) && code.get(n).map(|x| x.trim()) == c.as_deref().map(str::trim));
            if !is_kept && line.to_ascii_lowercase().contains("storytold/pdfcraft") {
                out.push(format!("{path}:{}: {} (point it at this fork)", n + 1, excerpt(line)));
            }
        }
    }
    out
}

/// Upstream's `UPDATE_APP` takes its repository from `craft_update::suite::PDFCRAFT`, so a merge
/// that drops the fork's `repo` field reads and downloads upstream's releases. Each `UPDATE_APP`
/// initialiser needs the field itself. The updater reads a public item at the crate root, so only
/// that declaration can satisfy the check, and its attributes must not mention `test`.
fn update_source(tree: &Tree) -> Vec<String> {
    let Some(text) = tree.get(UPDATE_APP_FILE) else {
        return vec![format!("{UPDATE_APP_FILE} is missing: find where `UPDATE_APP` went and keep its `repo`")];
    };
    if let Some(lexed) = lex(text)
        && let Some(level) = item_level(&lexed.code, false)
    {
        let production = |i: usize| {
            let code = &lexed.code;
            let public =
                code[..i].trim_end_matches(is_space).strip_suffix("const").and_then(|prefix| prefix.trim_end_matches(is_space).strip_suffix("pub"));
            level[i] && public.is_some_and(|prefix| found_at(code, prefix.len(), "pub")) && !mentions_test(code, &level, i)
        };
        let repo = |body: &Option<Range<usize>>| body.clone().is_some_and(|b| field(&lexed, b, "repo") == Some(FORK_REPO));
        let declared = initialisers(&lexed, "UPDATE_APP");
        if declared.iter().any(|(i, _)| production(*i)) && declared.iter().all(|(_, body)| repo(body)) {
            return Vec::new();
        }
    }
    vec![format!("{UPDATE_APP_FILE}: `UPDATE_APP` lacks `repo: {FORK_REPO}`, so updates would come from upstream")]
}

/// Lines in interface code, outside `theme.rs` and tests, that use a named `Color32` constant:
/// `Color32`, `::` and a capital, whatever whitespace or comments come between.
fn colour_lines(tree: &Tree) -> Vec<(&str, usize, String)> {
    let mut out = Vec::new();
    for (path, text) in tree.under("crates/ui-egui/src/") {
        if !path.ends_with(".rs") || path == "crates/ui-egui/src/theme.rs" || is_test(tree, path) {
            continue;
        }
        let read = production(text);
        let lines: Vec<&str> = read.lines().collect();
        let capital = |next: usize| read[next..].strip_prefix("r#").unwrap_or(&read[next..]).starts_with(|c: char| c.is_ascii_uppercase());
        let mut named: Vec<usize> = path_heads(&read, "Color32").filter(|&(_, next)| capital(next)).map(|(i, _)| line_index(&read, i)).collect();
        named.dedup();
        out.extend(named.into_iter().map(|n| (path, n + 1, lines[n].trim().to_string())));
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
    let mut out = Vec::new();
    for manifest in &tree.workspace_manifests {
        let publish = tree.get(manifest).and_then(|t| t.parse::<toml::Table>().ok()).and_then(|t| t.get("package")?.get("publish").cloned());
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
        let Some(lexed) = lex(text) else {
            out.push(format!("{path} doesn't lex, so its NOSONAR comments can't be told from strings: check them by hand"));
            continue;
        };
        let lines: Vec<(&str, &str)> = text.lines().zip(lexed.text.lines()).collect();
        let heads = ["Md5", "Ripemd160"].into_iter().flat_map(|hash| path_heads(&lexed.code, hash));
        let mut hashes: Vec<usize> = heads.map(|(i, _)| line_index(&lexed.code, i)).collect();
        hashes.sort_unstable();
        hashes.dedup();
        if hashes.is_empty() {
            out.push(format!("{path}: no MD5 or RIPEMD-160 line: find where they went and mark them"));
        }
        for (n, (line, _)) in hashes.into_iter().map(|n| (n, lines[n])).filter(|(_, (line, kept))| !marked_in_comment(line, kept)) {
            out.push(format!("{path}:{}: {} has lost its NOSONAR marker (rust:S4790)", n + 1, excerpt(line)));
        }
    }
    out
}

/// Whether a source line has `NOSONAR` in a comment: there, the line without its comments
/// (`kept`, from [`Lexed`]) has blanks in its place.
fn marked_in_comment(line: &str, kept: &str) -> bool {
    line.match_indices("NOSONAR").any(|(i, m)| kept.get(i..i + m.len()).is_some_and(|s| s.trim().is_empty()))
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
        let required = block.iter().any(|l| set_to(l, "CRAFT_I18N_REQUIRED").is_some_and(|value| value.trim() == "1"));
        if !(dir && required) {
            out.push(format!("{RELEASE}:{}: CRAFT_FONTS_DIR is set without CRAFT_I18N_DIR and CRAFT_I18N_REQUIRED=1 beside it", i + 1));
        }
    }
    out
}

/// The value in a release YAML env entry, shell export or leading shell assignment.
fn set_to<'a>(line: &'a str, var: &str) -> Option<&'a str> {
    let line = line.trim_start();
    if let Some(value) = line.strip_prefix(var).and_then(|rest| rest.strip_prefix(':')) {
        let end = unquoted_end(value, |i, c| c == '#' && (i == 0 || value[..i].ends_with(char::is_whitespace)));
        return unquote(value[..end].trim());
    }

    let export = line.strip_prefix("export").filter(|args| args.starts_with(char::is_whitespace));
    let mut args = export.unwrap_or(line).trim_start();
    while !args.is_empty() && !args.starts_with('#') {
        let end = unquoted_end(args, |_, c| c.is_whitespace() || matches!(c, ';' | '|' | '&' | '(' | ')' | '<' | '>'));
        let (word, rest) = args.split_at(end);
        match word.split_once('=') {
            Some((name, value)) if name == var => return unquote(value),
            Some((name, _)) if shell_name(name) => {}
            None if export.is_some() && shell_name(word) => {}
            _ => break,
        }
        if !rest.starts_with(char::is_whitespace) {
            break;
        }
        args = rest.trim_start();
    }
    None
}

fn shell_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_') && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn unquote(value: &str) -> Option<&str> {
    match value.chars().next() {
        Some(quote @ ('"' | '\'')) => value.strip_prefix(quote)?.strip_suffix(quote),
        _ => Some(value),
    }
}

/// End of a simple YAML value or shell word, skipping quoted delimiters.
fn unquoted_end(text: &str, stop: impl Fn(usize, char) -> bool) -> usize {
    let mut quote = None;
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            (None, _) if stop(i, c) => return i,
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), close) if open == close => quote = None,
            (None | Some('"'), '\\') => {
                chars.next();
            }
            _ => {}
        }
    }
    text.len()
}

/// The lines around `i` indented at least as far: its `env:` map, or the rest of its script.
fn block_around<'a>(lines: &[&'a str], i: usize) -> Vec<&'a str> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let inside = |l: &str| l.trim().is_empty() || l.trim_start().starts_with('#') || indent(l) >= indent(lines[i]);
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
            workspace_manifests: Vec::new(),
        }
    }

    fn passes(rule: Rule, files: &[(&str, &str)]) {
        let problems = rule(&tree(files));
        assert!(problems.is_empty(), "{problems:#?}");
    }

    fn fails(rule: Rule, files: &[(&str, &str)], expected: &str) {
        fails_all(rule, files, &[expected]);
    }

    fn fails_all(rule: Rule, files: &[(&str, &str)], expected: &[&str]) {
        let problems = rule(&tree(files));
        let missed: Vec<_> = expected.iter().filter(|e| !problems.iter().any(|p| p.contains(*e))).collect();
        assert!(missed.is_empty(), "expected problems naming {missed:?}, got {problems:#?}");
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
            ("crates/engine/src/links.rs", "pub const UPSTREAM: &str = \"https://github.com/storytold/pdfcraft\";\n#[cfg(test)]\nmod tests;\n"),
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
        let others = [
            ("crates/ui-egui/src/theme.rs", "pub const PAGE: Color32 = Color32::WHITE;\n#[cfg(test)]\nmod tests;\n"),
            ("crates/ui-egui/src/theme/tests.rs", "Color32::BLACK"),
        ];
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
        let unpublished = "[package]\nname = \"x\"\npublish = false\n";
        let mut workspace = tree(&[
            ("crates/cos/Cargo.toml", unpublished),
            ("crates/cos/codec/Cargo.toml", unpublished),
            ("crates/cos/fuzz/Cargo.toml", "[package]\nname = \"not-a-member\"\n"),
            ("xtask/Cargo.toml", unpublished),
            ("vendor/lopdf/Cargo.toml", "[package]\nname = \"lopdf\"\n"),
        ]);
        workspace.workspace_manifests = ["crates/cos/Cargo.toml", "crates/cos/codec/Cargo.toml", "xtask/Cargo.toml"].map(str::to_owned).to_vec();
        assert!(unpublished_crates(&workspace).is_empty());
        workspace.text.insert("crates/cos/codec/Cargo.toml".into(), "[package]\nname = \"codec\"\n".into());
        workspace.text.insert("xtask/Cargo.toml".into(), "[package]\nname = \"xtask\"\npublish = true\n".into());
        let problems = unpublished_crates(&workspace);
        assert_eq!(problems.len(), 2, "{problems:#?}");
        assert!(problems[0].starts_with("crates/cos/codec/Cargo.toml:"));
        assert!(problems[1].starts_with("xtask/Cargo.toml:"));
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
        // The marker counts only in a comment, and a hash only in code.
        let in_string = crypt.replace(" // NOSONAR: ISO 32000 mandates MD5", r#"; log("NOSONAR");"#);
        fails(weak_hash_markers, &[(WEAK_HASH_FILES[0], in_string.as_str()), (WEAK_HASH_FILES[1], keys)], "lib.rs:2");
        let block = crypt.replace("// NOSONAR: ISO 32000 mandates MD5", "/* NOSONAR: ISO 32000 */");
        passes(weak_hash_markers, &[(WEAK_HASH_FILES[0], block.as_str()), (WEAK_HASH_FILES[1], keys)]);
        let mentioned = format!("{crypt}// Md5::new() is ISO 32000's\nlet s = \"Md5::digest\";\n");
        passes(weak_hash_markers, &[(WEAK_HASH_FILES[0], mentioned.as_str()), (WEAK_HASH_FILES[1], keys)]);
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
        let incorrect_value = with.replace("CRAFT_I18N_REQUIRED: \"1\"", "CRAFT_I18N_REQUIRED: \"1 # required\"");
        fails(release_i18n, &[build, (RELEASE, incorrect_value.as_str())], "release.yml:5");
        let commented = with.replace("      CRAFT_I18N_", "      # CRAFT_I18N_");
        fails(release_i18n, &[build, (RELEASE, commented.as_str())], "release.yml:5");
        let empty_dir = with.replace("${{ github.workspace }}/craft-i18n", "# checkouts belong here");
        fails(release_i18n, &[build, (RELEASE, empty_dir.as_str())], "release.yml:5");
        let inline_comment =
            without.replace("CRAFT_FONTS_REQUIRED: \"1\"", "CRAFT_FONTS_REQUIRED: \"1\" # CRAFT_I18N_DIR: fake CRAFT_I18N_REQUIRED: 1");
        fails(release_i18n, &[build, (RELEASE, inline_comment.as_str())], "release.yml:5");
        let dedented_comment = with.replace("      CRAFT_I18N_DIR:", "# checkout supplied by the workflow\n      CRAFT_I18N_DIR:");
        passes(release_i18n, &[build, (RELEASE, dedented_comment.as_str())]);

        let script = "      - run: |\n          export CRAFT_FONTS_DIR=\"$PWD/craft-fonts\" CRAFT_FONTS_REQUIRED=1\n          bash package.sh\n";
        let both = script.replace("REQUIRED=1\n", "REQUIRED=1 CRAFT_I18N_DIR=\"$PWD/craft-i18n\" CRAFT_I18N_REQUIRED=1\n");
        passes(release_i18n, &[build, (RELEASE, format!("{with}  freebsd:\n    steps:\n{both}").as_str())]);
        let commented =
            script.replace("          bash", "          # export CRAFT_I18N_DIR=\"$PWD/craft-i18n\" CRAFT_I18N_REQUIRED=1\n          bash");
        fails(release_i18n, &[build, (RELEASE, format!("{with}  freebsd:\n    steps:\n{commented}").as_str())], "release.yml:14");
        let inline_comment = both.replace(" CRAFT_I18N_DIR=", " # CRAFT_I18N_DIR=");
        fails(release_i18n, &[build, (RELEASE, format!("{with}  freebsd:\n    steps:\n{inline_comment}").as_str())], "release.yml:14");
        let after_operator = both.replace(" CRAFT_I18N_DIR=", ";# CRAFT_I18N_DIR=");
        fails(release_i18n, &[build, (RELEASE, format!("{with}  freebsd:\n    steps:\n{after_operator}").as_str())], "release.yml:14");
        let quoted_hash = both.replace("$PWD/craft-fonts", "$PWD/craft # fonts");
        passes(release_i18n, &[build, (RELEASE, format!("{with}  freebsd:\n    steps:\n{quoted_hash}").as_str())]);
        let message = script.replace("          bash", "          echo \" Set CRAFT_I18N_DIR=translations CRAFT_I18N_REQUIRED=1\"\n          bash");
        fails(release_i18n, &[build, (RELEASE, format!("{with}  freebsd:\n    steps:\n{message}").as_str())], "release.yml:14");
        let inline_env = |s: &str| s.replace("export ", "").replace("\n          bash", " bash");
        passes(release_i18n, &[build, (RELEASE, format!("{with}  freebsd:\n    steps:\n{}", inline_env(&both)).as_str())]);
        fails(release_i18n, &[build, (RELEASE, format!("{with}  freebsd:\n    steps:\n{}", inline_env(script)).as_str())], "release.yml:14");
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
        for unread in ["const S: &str = \"unclosed;\n", "/* unclosed\n", "const R: &str = r#\"unclosed\"\n", "const Q: u8 = '{x;\n", "}\n", "f(];\n"]
        {
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
    fn the_xfdf_namespace_keeps_upstreams_url() {
        let ns = "pub(crate) const PDFCRAFT_NS: &str = \"https://github.com/storytold/pdfcraft#xfdf\";\n";
        passes(upstream_links, &[("crates/xfdf/src/xfdf.rs", ns)]);
        fails(upstream_links, &[("crates/xfdf/src/fdf.rs", ns)], "fdf.rs:1");
        let in_string = format!("pub const NOTES: &str = r#\"\n{ns}\"#;\n");
        fails(upstream_links, &[("crates/xfdf/src/xfdf.rs", in_string.as_str())], "xfdf.rs:2");
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
    fn update_app_counts_only_as_a_production_item() {
        let app = "pub const UPDATE_APP: App = App {\n    repo: \"yungSNAT/PlutarchPages\",\n    ..suite::PDFCRAFT\n};\n";
        let tested = format!("#[cfg(test)]\nmod tests {{\n{app}}}\n");
        passes(update_source, &[(UPDATE_APP_FILE, format!("{app}{tested}").as_str())]);
        let merged = app.replace("    repo: \"yungSNAT/PlutarchPages\",\n", "");
        fails(update_source, &[(UPDATE_APP_FILE, format!("{merged}{tested}").as_str())], "lacks `repo");
        let missed: Vec<&str> = [
            ("a test module", tested.clone()),
            ("a module gated by a compound test cfg", format!("#[cfg(all(test, not(target_arch = \"wasm32\")))]\nmod updater_tests {{\n{app}}}\n")),
            ("a private item", app.replacen("pub const", "const", 1)),
            ("a macro_rules! body", format!("macro_rules! app {{\n    () => {{\n{app}    }};\n}}\n")),
            ("a macro item", format!("pub macro app() {{\n{app}}}\n")),
            ("a macro call", format!("app! {{\n{app}}}\n")),
            ("a function", format!("fn f() {{\n{app}}}\n")),
            ("a #[cfg(test)] item", format!("#[cfg(test)]\n{app}")),
            ("a #[cfg(any(test))] item", format!("#[cfg(any(test))]\n{app}")),
        ]
        .into_iter()
        .filter(|(_, text)| !update_source(&tree(&[(UPDATE_APP_FILE, text.as_str())])).iter().any(|p| p.contains("lacks `repo")))
        .map(|(place, _)| place)
        .collect();
        assert!(missed.is_empty(), "UPDATE_APP counted from {missed:?}");
    }

    #[test]
    fn a_hash_path_is_read_across_whitespace_and_comments() {
        let crypt = "use md5::{Digest as _, Md5};\nlet mut h = Md5::new(); // NOSONAR: ISO 32000 mandates MD5\n";
        let keys = "DigestAlg::Ripemd160 => {\n    let mut h = ripemd::Ripemd160::new(); // NOSONAR: existing signatures\n";
        let crypt = format!("{crypt}digest = Md5/* c */::digest(&digest).to_vec();\nlet h = Md5 ::new();\nlet h = md5::Md5\n    ::new();\n");
        let keys = format!("{keys}let h = ripemd::Ripemd160/* c */\u{200e}::new();\n");
        fails_all(
            weak_hash_markers,
            &[(WEAK_HASH_FILES[0], crypt.as_str()), (WEAK_HASH_FILES[1], keys.as_str())],
            &["lib.rs:3", "lib.rs:4", "lib.rs:5", "keys.rs:3"],
        );
    }

    #[test]
    fn a_colour_path_is_read_across_whitespace_and_comments() {
        let canvas = "fn page() {\n    painter.rect_filled(r, 0.0, Color32::WHITE);\n}\n";
        let mut files = vec![("crates/ui-egui/src/canvas.rs", canvas)];
        let baseline = colour_baseline(&tree(&files));
        files.push((COLOUR_BASELINE, baseline.as_str()));
        passes(named_colours, &files);
        let added = format!(
            "{canvas}fn border() {{\n    painter.rect_filled(r, 0.0, Color32/* c */::BLACK);\n    painter.rect_filled(r, 0.0, egui::Color32 :: GRAY);\n    painter.rect_filled(r, 0.0, Color32\n        ::RED);\n    painter.rect_filled(r, 0.0, Color32::\u{200e}r#BLUE);\n}}\n"
        );
        files[0].1 = added.as_str();
        fails_all(named_colours, &files, &["canvas.rs:5", "canvas.rs:6", "canvas.rs:7", "canvas.rs:9"]);
    }

    #[test]
    fn a_test_file_is_test_code_only_when_declared_for_tests() {
        let lib = "crates/engine/src/lib.rs";
        let file = ("crates/engine/src/tests.rs", "const URL: &str = \"https://github.com/storytold/pdfcraft\";\n");
        let nested = ("crates/engine/src/tests/more.rs", file.1);
        passes(upstream_links, &[(lib, "#[cfg(test)]\nmod tests;\n"), (file.0, "mod more;\n"), nested]);
        passes(upstream_links, &[(lib, "#[cfg(test)]\n#[allow(dead_code)]\nmod tests;\n"), file]);
        let canvas = ("crates/ui-egui/src/canvas.rs", "#[cfg(test)]\nmod tests;\n");
        passes(named_colours, &[(COLOUR_BASELINE, ""), canvas, ("crates/ui-egui/src/canvas/tests.rs", "Color32::RED\n")]);
        let mut missed: Vec<&str> = [
            ("an undeclared tests.rs", vec![file]),
            ("a tests.rs declared outside a test module", vec![(lib, "mod tests;\n"), file]),
            ("a tests.rs declared twice", vec![(lib, "#[cfg(test)]\nmod tests;\n#[cfg(not(test))]\nmod tests;\n"), file]),
            ("a tests.rs declared in a macro", vec![(lib, "m! {\n    #[cfg(test)]\n    mod tests;\n}\n"), file]),
            ("a file under a production tests.rs", vec![(lib, "pub mod tests;\n"), (file.0, "mod more;\n"), nested]),
        ]
        .into_iter()
        .filter(|(_, files)| !upstream_links(&tree(files)).iter().any(|p| p.contains("tests") && p.contains(".rs:1")))
        .map(|(case, _)| case)
        .collect();
        let contests = ("crates/ui-egui/src/contests.rs", "fn f() { painter.rect_filled(r, 0.0, Color32::WHITE); }\n");
        if named_colours(&tree(&[(COLOUR_BASELINE, ""), contests])).is_empty() {
            missed.push("a file whose name contains 'tests'");
        }
        assert!(missed.is_empty(), "read as test code: {missed:?}");
    }

    #[test]
    fn only_item_level_test_modules_are_left_alone() {
        let home = "crates/ui-egui/src/home.rs";
        let module = "#[cfg(test)]\n    mod tests {\n        fn f() { widgets::artcraft_mark(ui, 28.0); }\n    }\n";
        passes(artcraft_names, &[(home, format!("mod a {{\n    {module}}}\n").as_str())]);
        let missed: Vec<&str> = [("a macro call", format!("m! {{\n    {module}}}\n")), ("a function", format!("fn g() {{\n    {module}}}\n"))]
            .into_iter()
            .filter(|(_, text)| !artcraft_names(&tree(&[(home, text.as_str())])).iter().any(|p| p.contains("home.rs:4")))
            .map(|(place, _)| place)
            .collect();
        assert!(missed.is_empty(), "a test module left alone in {missed:?}");
    }

    #[test]
    fn a_build_script_anywhere_makes_translations_a_release_input() {
        let release = (RELEASE, "jobs:\n  linux:\n    env:\n      CRAFT_FONTS_DIR: fonts\n");
        for build in ["build.rs", "xtask/build.rs"] {
            fails(release_i18n, &[(build, "println!(\"cargo::rerun-if-env-changed=CRAFT_I18N_DIR\");"), release], "release.yml:4");
        }
    }
}
