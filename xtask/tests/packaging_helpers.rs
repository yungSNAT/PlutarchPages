//! The standalone packaging helpers are built with plain `rustc`, not Cargo, so their
//! `#[cfg(test)]` modules would run only by hand. Each test here compiles one helper with
//! `--test`, as strictly as its scripts build it, and runs its unit tests.

use std::path::Path;
use std::process::Command;

fn run_unit_tests(source: &str, name: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the workspace root");
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("packaging-helpers");
    std::fs::create_dir_all(&out).expect("create the helper test folder");
    let bin = out.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    let build = Command::new("rustc")
        .args(["--edition=2024", "-D", "warnings", "--test"])
        .arg(root.join(source))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("run rustc");
    assert!(build.status.success(), "rustc --test {source} failed:\n{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&bin).current_dir(root).output().expect("run the helper's unit tests");
    assert!(run.status.success(), "{source} unit tests failed:\n{}\n{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
}

#[test]
fn unix_packager_unit_tests_pass() {
    run_unit_tests("packaging/unix_package.rs", "unix_package");
}

#[test]
fn windows_packager_unit_tests_pass() {
    run_unit_tests("packaging/windows/package.rs", "windows_package");
}

#[test]
fn windows_checks_unit_tests_pass() {
    run_unit_tests("packaging/windows/win_checks.rs", "win_checks");
}
