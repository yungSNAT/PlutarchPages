//! How often `verify(doc, Level::A1b)` agrees with veraPDF on veraPDF's own PDF/A-1b test files.
//!
//! `data/verapdf-1.30.4-pdfa-1b.tsv` holds what veraPDF 1.30.4 (`--flavour 1b`) said about each
//! file of the `PDF_A-1b` folder of the veraPDF test corpus: its verdict and the rules it found
//! broken. The corpus itself is not in this repository; point `PDFCRAFT_VERAPDF_CORPUS` at a
//! checkout of it (the folder holding `PDF_A-1b`) and run
//!
//! ```text
//! cargo test -p pdfcraft-preflight --test verapdf_agreement -- --ignored --nocapture
//! ```
//!
//! Three counts come out. On a file veraPDF passes, we agree when we report nothing. On a file
//! veraPDF fails, we agree on the verdict when we report anything, and on the clause when one of
//! our issues names an ISO 19005-1 clause veraPDF found broken in it.
//!
//! `data/pdfa-1b-agreement.tsv` records, file by file, how far we agreed when it was last
//! measured (`none`, `verdict`, `clause` or `pass`), every file of the oracle once. The test fails
//! when the record or the oracle lists a file twice, when the record and the oracle list different
//! files, when the corpus is missing or incomplete, when a file's SHA-256 differs from the one
//! recorded (another revision of the corpus), when a file cannot be opened, and when any file
//! agrees less than it did, so a gain on some files cannot hide a loss on others. A file that
//! agrees more than its record is printed as a `RECORD` line, for the change that made it so to
//! record.

use std::path::{Path, PathBuf};

use pdfcraft_cos::Document;
use pdfcraft_preflight::{Level, verify};

/// veraPDF's answers, one file a line: path under `PDF_A-1b`, `PASS` or `FAIL`, the rules
/// (`clause-test`) it found broken, comma-separated, and the file's SHA-256.
const ORACLE: &str = include_str!("data/verapdf-1.30.4-pdfa-1b.tsv");

/// How far each file agreed when last measured: `none`, `verdict`, `clause` or `pass`, a tab and its
/// path.
const RECORDED: &str = include_str!("data/pdfa-1b-agreement.tsv");

/// How far we agree with veraPDF on one file, least first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Agreement {
    None,
    /// veraPDF fails the file and so do we, under none of its clauses.
    Verdict,
    /// veraPDF fails the file and we name one of its clauses; or it passes the file and so do we.
    Full,
}

fn recorded() -> std::collections::HashMap<&'static str, Agreement> {
    let mut out = std::collections::HashMap::new();
    for (status, path) in RECORDED.lines().filter_map(|l| l.split_once('\t')) {
        let agreement = match status {
            "none" => Agreement::None,
            "verdict" => Agreement::Verdict,
            "pass" | "clause" => Agreement::Full,
            other => panic!("unknown status {other:?} for {path}"),
        };
        assert!(out.insert(path, agreement).is_none(), "{path} is recorded twice");
    }
    out
}

struct Expected<'a> {
    path: &'a str,
    pass: bool,
    clauses: Vec<&'a str>,
    sha256: &'a str,
}

fn expected() -> Vec<Expected<'static>> {
    ORACLE
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let mut cols = l.split('\t');
            let path = cols.next().unwrap_or_default();
            let pass = cols.next() == Some("PASS");
            // A rule id is its clause, a dash and the test's number.
            let clauses = cols.next().unwrap_or_default().split(',').filter_map(|r| r.rsplit_once('-').map(|(c, _)| c)).collect();
            let sha256 = cols.next().unwrap_or_default();
            Expected { path, pass, clauses, sha256 }
        })
        .collect()
}

fn corpus() -> PathBuf {
    let root = std::env::var_os("PDFCRAFT_VERAPDF_CORPUS").expect("PDFCRAFT_VERAPDF_CORPUS names the veraPDF corpus checkout");
    Path::new(&root).join("PDF_A-1b")
}

#[test]
#[ignore = "needs the veraPDF corpus: set PDFCRAFT_VERAPDF_CORPUS"]
fn agreement_with_verapdf_on_its_pdfa_1b_files() {
    let dir = corpus();
    let files = expected();
    assert_eq!(files.len(), 568, "the oracle lists every PDF_A-1b file of the corpus");
    let paths: std::collections::HashSet<&str> = files.iter().map(|f| f.path).collect();
    assert_eq!(paths.len(), files.len(), "the oracle lists a file twice");
    let mut record = recorded();
    let (mut worse, mut changed, mut unopened) = (Vec::new(), Vec::new(), Vec::new());
    let (mut pass, mut on_pass, mut fail, mut on_fail, mut on_clause) = (0, 0, 0, 0, 0);
    let mut missed: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for f in &files {
        let before = record.remove(f.path).unwrap_or_else(|| panic!("{} has no line in data/pdfa-1b-agreement.tsv", f.path));
        let bytes = std::fs::read(dir.join(f.path)).unwrap_or_else(|e| panic!("{}: {e}", f.path));
        if sha256_hex(&bytes) != f.sha256 {
            changed.push(f.path);
            continue;
        }
        // A file we cannot open gets no verdict from us, so it agrees on nothing.
        let Ok(doc) = Document::open(std::sync::Arc::new(bytes)) else {
            unopened.push(f.path);
            continue;
        };
        let issues = verify(&doc, Level::A1b);
        let ours_fail = !issues.is_empty();
        let agreement = if f.pass {
            pass += 1;
            on_pass += usize::from(!ours_fail);
            if ours_fail { Agreement::None } else { Agreement::Full }
        } else {
            fail += 1;
            on_fail += usize::from(ours_fail);
            let named = issues.iter().any(|i| f.clauses.contains(&i.clause));
            on_clause += usize::from(named);
            if !named {
                for c in &f.clauses {
                    *missed.entry(c).or_default() += 1;
                }
            }
            match (named, ours_fail) {
                (true, _) => Agreement::Full,
                (false, true) => Agreement::Verdict,
                (false, false) => Agreement::None,
            }
        };
        if agreement < before {
            worse.push(f.path);
        } else if agreement > before {
            let status = match agreement {
                Agreement::Full if f.pass => "pass",
                Agreement::Full => "clause",
                _ => "verdict",
            };
            println!("RECORD\t{status}\t{}", f.path);
        }
    }
    assert!(record.is_empty(), "files recorded that the oracle doesn't list: {:#?}", record.keys());
    println!("veraPDF passes {pass}: we agree on {on_pass}");
    println!("veraPDF fails {fail}: we agree on the verdict for {on_fail} and on a clause for {on_clause}");
    println!("clauses veraPDF found broken in a file where we named none of its clauses: {missed:?}");
    assert!(changed.is_empty(), "files whose SHA-256 differs from the oracle's (not corpus 37772859): {changed:#?}");
    assert!(unopened.is_empty(), "files that could not be opened: {unopened:#?}");
    assert!(worse.is_empty(), "files that agree less than data/pdfa-1b-agreement.tsv records: {worse:#?}");
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
