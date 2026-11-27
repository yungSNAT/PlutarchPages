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
//! measured (`pass`, `verdict` or `clause`). The test fails when the corpus is missing or
//! incomplete, and when any recorded file agrees less than it did, so a gain on some files cannot
//! hide a loss on others. A file that agrees more than its record is printed as a `RECORD` line,
//! for the change that made it so to record; the figure the documentation quotes is the one this
//! file holds.

use std::path::{Path, PathBuf};

use pdfcraft_cos::Document;
use pdfcraft_preflight::{Level, verify};

/// veraPDF's answers, one file a line: path under `PDF_A-1b`, `PASS` or `FAIL`, and the rules
/// (`clause-test`) it found broken, comma-separated.
const ORACLE: &str = include_str!("data/verapdf-1.30.4-pdfa-1b.tsv");

/// How far each file agreed when last measured: `pass`, `verdict` or `clause`, a tab and its path.
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
    RECORDED
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(status, path)| {
            let agreement = match status {
                "verdict" => Agreement::Verdict,
                "pass" | "clause" => Agreement::Full,
                other => panic!("unknown status {other:?} for {path}"),
            };
            (path, agreement)
        })
        .collect()
}

struct Expected<'a> {
    path: &'a str,
    pass: bool,
    clauses: Vec<&'a str>,
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
            Expected { path, pass, clauses }
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
    let record = recorded();
    let mut worse = Vec::new();
    let (mut pass, mut on_pass, mut fail, mut on_fail, mut on_clause) = (0, 0, 0, 0, 0);
    let mut missed: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for f in &files {
        let bytes = std::fs::read(dir.join(f.path)).unwrap_or_else(|e| panic!("{}: {e}", f.path));
        // A file we cannot open is one we would refuse: no issue list, so it counts as a fail.
        let issues = Document::open(std::sync::Arc::new(bytes)).map(|doc| verify(&doc, Level::A1b));
        let ours_fail = issues.as_ref().map_or(true, |i| !i.is_empty());
        let agreement = if f.pass {
            pass += 1;
            on_pass += usize::from(!ours_fail);
            if ours_fail { Agreement::None } else { Agreement::Full }
        } else {
            fail += 1;
            on_fail += usize::from(ours_fail);
            let named = issues.as_ref().is_ok_and(|i| i.iter().any(|i| f.clauses.contains(&i.clause)));
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
        let before = record.get(f.path).copied().unwrap_or(Agreement::None);
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
    println!("veraPDF passes {pass}: we agree on {on_pass}");
    println!("veraPDF fails {fail}: we agree on the verdict for {on_fail} and on a clause for {on_clause}");
    println!("clauses veraPDF found broken in a file where we named none of its clauses: {missed:?}");
    assert!(worse.is_empty(), "files that agree less than data/pdfa-1b-agreement.tsv records: {worse:#?}");
}
