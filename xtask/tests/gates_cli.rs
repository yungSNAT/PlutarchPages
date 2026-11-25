//! The gate commands, run as `cargo xtask` runs them, with a stand-in `cargo` (`CARGO`), oracles
//! and release CLI. `PATH` holds only the stand-ins, so whatever the machine has installed cannot
//! change the outcome. Unix only: the stand-ins are shell scripts.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Logs its arguments, fails when they contain `FAKE_FAIL`, answers `metadata` with
/// `FAKE_METADATA` (or the real cargo when unset) and prints `FAKE_REPORT` for `test`.
const FAKE_CARGO: &str = r#"printf '%s\n' "$*" >> "$FAKE_LOG"
if [ -n "$FAKE_FAIL" ]; then case "$*" in *"$FAKE_FAIL"*) printf 'fake failure\n' >&2; exit 3;; esac; fi
case "$1" in
  metadata) if [ -n "$FAKE_METADATA" ]; then while IFS= read -r l; do printf '%s\n' "$l"; done < "$FAKE_METADATA"; else PATH="$REAL_PATH" exec "$REAL_CARGO" "$@"; fi ;;
  test) if [ -n "$FAKE_REPORT" ]; then while IFS= read -r l; do printf '%s\n' "$l"; done < "$FAKE_REPORT"; fi ;;
esac
exit 0"#;

/// Two crates on their layers (one through a dev-dependency) and one exempt crate.
const LAYERED: &str = r#"{"packages": [
  {"name": "pdfcraft-cos", "dependencies": []},
  {"name": "pdfcraft-render", "dependencies": [{"name": "pdfcraft-cos", "kind": null}, {"name": "pdfcraft-content", "kind": "dev"}, {"name": "serde", "kind": "build"}]},
  {"name": "pdfcraft-content", "dependencies": [{"name": "pdfcraft-cos", "kind": null}]},
  {"name": "xtask", "dependencies": []}
]}
"#;

/// `content` (layer 2) depending on `render` (layer 3).
const UPWARD: &str = r#"{"packages": [
  {"name": "pdfcraft-render", "dependencies": []},
  {"name": "pdfcraft-content", "dependencies": [{"name": "pdfcraft-render"}]}
]}
"#;

/// A folder of its own under the temporary directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("xtask-gates-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let scratch = Scratch(dir);
        scratch.script("bin/cargo", FAKE_CARGO);
        scratch
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.path(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    }

    fn script(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.write(rel, &format!("#!/bin/sh\n{body}\n"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// The commands the stand-in cargo was given, one per line.
    fn log(&self) -> String {
        std::fs::read_to_string(self.path("cargo.log")).unwrap_or_default()
    }

    /// `cargo xtask <args>` with the stand-ins and `envs` on top.
    fn xtask(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_xtask"));
        c.args(args)
            .env("PATH", self.path("bin"))
            .env("CARGO", self.path("bin/cargo"))
            .env("CARGO_TARGET_DIR", self.path("target"))
            .env("FAKE_LOG", self.path("cargo.log"))
            .env("REAL_CARGO", std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .env("REAL_PATH", std::env::var_os("PATH").unwrap_or_default())
            .env_remove("CI")
            .env_remove("PDFCRAFT_ORACLES_REQUIRED")
            .env_remove("FAKE_FAIL")
            .env_remove("FAKE_METADATA")
            .env_remove("FAKE_REPORT");
        for (k, v) in envs {
            c.env(k, v);
        }
        c.output().unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn layers_lists_each_crate_and_fails_on_an_upward_dependency() {
    let s = Scratch::new("layers");
    let ok = s.write("ok.json", LAYERED);
    let out = s.xtask(&["layers"], &[("FAKE_METADATA", ok.to_str().unwrap())]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.lines().any(|l| l.starts_with("pdfcraft-render") && l.ends_with("cos, content (dev)")), "{text}");
    assert!(text.contains("OK: 4 crates, no layering violations."), "{text}");
    assert_eq!(s.log(), "metadata --format-version 1 --no-deps\n");

    let bad = s.write("bad.json", UPWARD);
    let out = s.xtask(&["layers"], &[("FAKE_METADATA", bad.to_str().unwrap())]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("  - "), "{}", stdout(&out));
    assert!(stderr(&out).contains("xtask layers: error: 1 layering violation(s)"), "{}", stderr(&out));

    // Malformed or failed metadata, and a cargo that cannot start.
    let junk = s.write("junk.json", "not json\n");
    let out = s.xtask(&["layers"], &[("FAKE_METADATA", junk.to_str().unwrap())]);
    assert!(stderr(&out).contains("expected ident"), "{}", stderr(&out));
    let out = s.xtask(&["layers"], &[("FAKE_FAIL", "metadata")]);
    assert!(stderr(&out).contains("cargo metadata failed:\nfake failure"), "{}", stderr(&out));
    let out = s.xtask(&["wasm"], &[("CARGO", "/nonexistent/cargo")]);
    assert!(stderr(&out).starts_with("xtask wasm: error: cargo metadata: "), "{}", stderr(&out));
    let empty = s.write("empty.json", "{}\n");
    let out = s.xtask(&["layers"], &[("FAKE_METADATA", empty.to_str().unwrap())]);
    assert!(stderr(&out).contains("metadata: no packages array"), "{}", stderr(&out));
}

#[test]
fn wasm_checks_every_layered_crate_in_one_call() {
    let s = Scratch::new("wasm");
    let meta = s.write("meta.json", LAYERED);
    let out = s.xtask(&["wasm"], &[("FAKE_METADATA", meta.to_str().unwrap())]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("wasm32: 3 crates ok"));
    let check = s.log().lines().nth(1).unwrap().to_string();
    assert_eq!(check, "check --target wasm32-unknown-unknown --keep-going -p pdfcraft-content -p pdfcraft-cos -p pdfcraft-render");

    let out = s.xtask(&["wasm"], &[("FAKE_METADATA", meta.to_str().unwrap()), ("FAKE_FAIL", "--target")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("cargo check --target wasm32-unknown-unknown --keep-going (3 crates): exited with exit status: 3"),
        "{}",
        stderr(&out)
    );
    let out = s.xtask(&["test"], &[("CARGO", "/nonexistent/cargo")]);
    assert!(stderr(&out).contains("failed to spawn"), "{}", stderr(&out));
}

#[test]
fn test_prefers_nextest_and_falls_back_to_cargo_test() {
    let s = Scratch::new("test");
    let out = s.xtask(&["test"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(s.log(), "nextest --version\nnextest run --no-fail-fast --workspace --lib --bins --tests\ntest --workspace --doc\n");

    let s = Scratch::new("test-fallback");
    let out = s.xtask(&["test"], &[("FAKE_FAIL", "nextest --version")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("test: cargo-nextest is not installed, using cargo test"));
    assert!(s.log().ends_with("test --no-fail-fast --workspace --lib --bins --tests\ntest --workspace --doc\n"), "{}", s.log());
}

#[test]
fn deny_is_skipped_locally_required_in_ci_and_run_when_installed() {
    let s = Scratch::new("deny");
    let out = s.xtask(&["deny"], &[]);
    assert!(out.status.success());
    assert!(stderr(&out).contains("deny: SKIPPED, cargo-deny is not installed"));
    let out = s.xtask(&["deny"], &[("CI", "true")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("cargo-deny is required in CI: cargo install cargo-deny --locked"));

    s.script("bin/cargo-deny", "exit 0");
    let out = s.xtask(&["deny"], &[("CI", "true")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(s.log(), "deny --log-level error check\n");
}

#[test]
fn i18n_report_prints_only_the_report_block() {
    let s = Scratch::new("i18n");
    let report = s.write(
        "report.txt",
        "noise\n=== translation gaps ===\nde: 2 missing (a, b)\nfr: 1 missing (c)\n=== end of translation gaps ===\nmore noise\n",
    );
    let out = s.xtask(&["i18n-report"], &[("FAKE_REPORT", report.to_str().unwrap())]);
    assert!(out.status.success());
    assert_eq!(stdout(&out), "de: 2 missing (a, b)\nfr: 1 missing (c)\n");
    assert_eq!(s.log(), "test -p pdfcraft-ui-egui --lib --quiet -- i18n::tests::translation_gaps_report --exact --nocapture\n");

    let s = Scratch::new("i18n-fork");
    let out = s.xtask(&["i18n-report", "--fork", "--workspace"], &[]);
    assert!(out.status.success());
    assert_eq!(stdout(&out), "i18n-report: no report (cargo test exited with exit status: 0)\n");
    assert_eq!(s.log(), "test --workspace --lib --quiet -- i18n::fork_tests::fork_translation_gaps_report --exact --nocapture\n");

    let out = s.xtask(&["i18n-report"], &[("CARGO", "/nonexistent/cargo")]);
    assert!(out.status.success(), "a report that cannot run is not a failure");
    assert!(stdout(&out).starts_with("i18n-report: no report, cargo test did not start: "), "{}", stdout(&out));
}

#[test]
fn ci_stops_at_the_first_failing_step() {
    let s = Scratch::new("ci-fail");
    let out = s.xtask(&["ci"], &[("FAKE_FAIL", "fmt")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("CI: 0 passed, FAILED at `fmt`: cargo fmt --all -- --check: exited with exit status: 3"), "{}", stdout(&out));
    assert!(stderr(&out).contains("xtask ci: error: ci failed at `fmt`"));
    assert_eq!(s.log(), "fmt --all -- --check\n");
}

/// The steps run in order while the cargo commands pass: the checks that read the repository run
/// for real (with the real `cargo metadata`), the builds and tests are stood in for. fork-check's
/// all-features metadata is failed on purpose, which stops the run there: resolving every fuzz
/// workspace for real takes minutes.
#[test]
fn ci_runs_the_steps_in_order() {
    let s = Scratch::new("ci");
    // license-check reads the tracked files through git.
    s.script("bin/git", r#"PATH="$REAL_PATH" exec git "$@""#);
    let out = s.xtask(&["ci"], &[("FAKE_FAIL", "--all-features")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("CI: 10 passed, FAILED at `fork-check`"), "{}\n{}", stdout(&out), stderr(&out));
    let steps: Vec<String> = s.log().lines().filter(|l| !l.starts_with("metadata")).map(String::from).collect();
    assert_eq!(steps.len(), 9, "{steps:#?}");
    assert_eq!(
        steps[..8],
        [
            "fmt --all -- --check",
            "clippy --workspace --all-targets -- -D warnings",
            "check -p pdfcraft-cli --no-default-features",
            "nextest --version",
            "nextest run --no-fail-fast --workspace --lib --bins --tests",
            "test --workspace --doc",
            "test --features content --lib",
            "test --workspace --lib --quiet -- i18n::tests::translation_gaps_report --exact --nocapture",
        ]
    );
    assert!(steps[8].starts_with("check --target wasm32-unknown-unknown --keep-going -p pdfcraft-"), "{}", steps[8]);
}

#[test]
fn check_refuses_to_sweep_without_the_pinned_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    if root.join("corpus/pdfjs/test/pdfs").is_dir() {
        // A fetched corpus is checked for real, which is not this test's business.
        eprintln!("skipped: corpus/pdfjs is present, so `check` would verify it rather than refuse");
        return;
    }
    let s = Scratch::new("check");
    let out = s.xtask(&["check"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("corpus missing: run `cargo xtask corpus` first"), "{}", stderr(&out));
    assert_eq!(s.log(), "", "nothing is built for a sweep that cannot run");
}

#[test]
fn text_oracle_needs_pdftotext_and_the_cli() {
    let s = Scratch::new("text");
    let out = s.xtask(&["text-oracle"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("oracle `pdftotext` is not on PATH"), "{}", stderr(&out));

    s.script("bin/pdftotext", "printf 'pdftotext version 0\\n' >&2");
    let out = s.xtask(&["text-oracle", "--limit", "1"], &[("FAKE_FAIL", "build")]);
    assert!(stderr(&out).contains("cargo build --release -p pdfcraft-cli: exited with exit status: 3"), "{}", stderr(&out));

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    if !root.join("corpus/pdfjs/test/pdfs").is_dir() {
        let out = s.xtask(&["text-oracle"], &[]);
        assert_eq!(out.status.code(), Some(1));
        assert!(stderr(&out).contains("No such file or directory"), "{}", stderr(&out));
    } else {
        eprintln!("skipped the missing-corpus case: corpus/pdfjs is present");
    }
}

/// The stand-in renderer draws by file name: `dark` is black, `size` is a column wider, `ours-fails`
/// fails; Poppler's stand-in draws everything white except `theirs-fails`.
const FAKE_CLI: &str = r#"for a; do f=$a; done
case "$f" in *ours-fails.pdf) exit 1;; esac
w=2; px='\377\377\377\377'
case "$f" in *dark.pdf) px='\000\000\000\377';; *size.pdf) w=3;; esac
printf 'P7\nWIDTH %s\nHEIGHT 2\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n' "$w" > "$7"
i=0; while [ "$i" -lt "$((w * 2))" ]; do printf "$px" >> "$7"; i=$((i + 1)); done"#;

const FAKE_PDFTOPPM: &str = r#"printf '%s\n' "$*" >> "$FAKE_LOG.ppm"
for a; do f=$a; done
case "$f" in -v) printf 'pdftoppm version 0\n' >&2; exit 0;; *theirs-fails.pdf) exit 1;; esac
printf 'P5\n2 2\n255\n\377\377\377\377'"#;

#[test]
fn render_oracle_scores_what_both_renderers_drew() {
    let s = Scratch::new("render");
    let out = s.xtask(&["render-oracle", "--dir", s.path("pdfs").to_str().unwrap()], &[]);
    assert!(stderr(&out).contains("oracle `pdftoppm` is not on PATH"), "{}", stderr(&out));

    s.script("bin/pdftoppm", FAKE_PDFTOPPM);
    s.script("target/release/pdfcraft-cli", FAKE_CLI);
    let out = s.xtask(&["render-oracle", "--dir", s.path("pdfs").to_str().unwrap()], &[]);
    assert!(stderr(&out).contains("no such directory (cargo xtask corpus, or pass --dir)"), "{}", stderr(&out));

    for name in ["dark.pdf", "ours-fails.pdf", "same.pdf", "size.pdf", "theirs-fails.pdf", "notes.txt"] {
        s.write(&format!("pdfs/{name}"), "%PDF-1.7\n");
    }
    let dir = s.path("pdfs");
    let args = ["render-oracle", "--dir", dir.to_str().unwrap(), "--dpi", "50", "--tolerance", "4", "--hide-annotations"];
    let out = s.xtask(&args, &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("render oracle vs Poppler at 50 dpi, tolerance 4"), "{text}");
    assert!(text.contains("  2 compared: median 100.00%, mean 50.00%, p90 100.00%"), "{text}");
    assert!(text.contains("  1 within 0.1%; 1 size mismatches, 1 we could not render, 1 Poppler could not"), "{text}");
    assert!(text.ends_with("worst:\n  100.00%  dark.pdf\n    0.00%  same.pdf\n"), "{text}");
    let ppm = std::fs::read_to_string(s.path("cargo.log.ppm")).unwrap();
    assert!(ppm.lines().any(|l| l.ends_with(&format!("-singlefile -gray -f 1 -l 1 -r 50 -hide-annotations {}/same.pdf", dir.display()))), "{ppm}");

    // Only the first file, which our stand-in renders black and Poppler white: still comparable.
    let out = s.xtask(&["render-oracle", "--dir", dir.to_str().unwrap(), "--limit", "1"], &[]);
    assert!(stdout(&out).contains("  1 compared: median 100.00%"), "{}", stdout(&out));
    // No files at all: nothing to compare.
    let out = s.xtask(&["render-oracle", "--dir", dir.to_str().unwrap(), "--limit", "0"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no comparable files in"), "{}", stderr(&out));
}

/// `fuzz` builds the release CLI first and refuses to fuzz without it; bad flags stop it before
/// anything is built.
#[test]
fn fuzz_builds_the_cli_and_needs_it() {
    let s = Scratch::new("fuzz");
    let out = s.xtask(&["fuzz", "--time", "soon"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("invalid digit found in string"), "{}", stderr(&out));
    assert_eq!(s.log(), "");

    let out = s.xtask(&["fuzz"], &[("CARGO", "/nonexistent/cargo")]);
    assert!(stderr(&out).contains("xtask fuzz: error: No such file or directory"), "{}", stderr(&out));
    let out = s.xtask(&["fuzz", "--iterations", "0"], &[("FAKE_FAIL", "build")]);
    assert!(stderr(&out).contains("xtask fuzz: error: building pdfcraft-cli failed"), "{}", stderr(&out));
    assert_eq!(s.log(), "build --release -q -p pdfcraft-cli\n");

    let out = s.xtask(&["fuzz", "--iterations", "0"], &[]);
    let cli = s.path("target/release/pdfcraft-cli");
    assert!(stderr(&out).contains(&format!("{} not found after building it", cli.display())), "{}", stderr(&out));

    // With the CLI in place a campaign of no iterations runs nothing and passes. (It makes the
    // git-ignored fuzz-out/ folders in the checkout, as every `cargo xtask fuzz` does.)
    s.script("target/release/pdfcraft-cli", "exit 1");
    let out = s.xtask(&["fuzz", "--iterations", "0", "--jobs", "2", "--seed", "9", "--timeout", "1", "--time", "5"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("seeds, 2 jobs, 5 s, timeout 1 s, seed 9"), "{text}");
    assert!(text.contains("fuzz: 0 runs in"), "{text}");
    assert!(text.trim_end().ends_with("0 distinct crash(es), 0 hang(s)"), "{text}");
}
