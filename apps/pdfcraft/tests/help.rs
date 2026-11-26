//! `--help`, `-h` and `--version` print and exit without opening a window (#4076), so a script that
//! calls them returns.

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft");

/// Run the app with `args`, failing if it is still running after 60 s (it opened its window).
fn run(args: &[&str], stdout: Stdio) -> Output {
    run_with(Command::new(BIN).args(args), stdout)
}

fn run_with(command: &mut Command, stdout: Stdio) -> Output {
    let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    let mut child = command.stdin(Stdio::null()).stdout(stdout).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("pdfcraft {args:?} did not exit: it started the app instead");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn help_prints_usage_and_exits() {
    for flag in ["--help", "-h"] {
        let out = run(&[flag], Stdio::piped());
        assert!(out.status.success(), "{flag}: {:?} {}", out.status, String::from_utf8_lossy(&out.stderr));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.starts_with("Usage: pdfcraft [options] [files…]"), "{flag}: {stdout}");
        assert!(stdout.contains("pdfcraft-cli"), "{flag}: points scripts to the CLI: {stdout}");
    }
    // Wherever it is on the command line, and with files named.
    let out = run(&["missing.pdf", "--help"], Stdio::piped());
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("Usage:"));
}

#[test]
fn version_prints_the_version_and_exits() {
    let out = run(&["--version"], Stdio::piped());
    assert!(out.status.success(), "{:?} {}", out.status, String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("pdfcraft "), "{}", String::from_utf8_lossy(&out.stdout));
}

/// An `OPENSSL_ia32cap` that asks for a CPU feature the processor lacks (CPUID leaf 1, EDX bit
/// 10, reserved on every x86) doesn't stop the app (#2916): aws-lc aborted before `main`, with no
/// window and nothing in the log.
// macOS intentionally does not relaunch: Finder delivers files to the original process.
#[cfg(not(target_os = "macos"))]
#[test]
fn a_cpu_override_naming_a_missing_feature_does_not_stop_the_app() {
    let out = run_with(Command::new(BIN).arg("--version").env("OPENSSL_ia32cap", "0x400"), Stdio::piped());
    assert!(out.status.success(), "{:?} {}", out.status, String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("pdfcraft "), "{}", String::from_utf8_lossy(&out.stdout));
}

/// A standard output that can't be written (here a full device) ends the run normally: `println!`
/// would panic and exit 101.
#[cfg(target_os = "linux")]
#[test]
fn help_and_version_survive_an_unwritable_stdout() {
    for flag in ["--help", "--version"] {
        let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
        let out = run(&[flag], Stdio::from(full));
        assert!(out.status.success(), "{flag}: {:?} {}", out.status, String::from_utf8_lossy(&out.stderr));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn startup_migrates_legacy_storage_only_when_current_storage_is_absent() {
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;

    let root = std::env::temp_dir().join(format!("pdfcraft-migration-{}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    for (label, current_exists) in [("fresh", false), ("existing", true)] {
        let home = root.join(label);
        let data = home.join("data");
        let legacy = data.join("printcraft");
        let current = data.join("pdfcraft");
        let runtime = home.join("run");
        fs::create_dir_all(&legacy).unwrap();
        fs::DirBuilder::new().mode(0o700).create(&runtime).unwrap();
        fs::write(legacy.join("prefs.fixture"), b"legacy").unwrap();
        if current_exists {
            fs::create_dir(&current).unwrap();
            fs::write(current.join("prefs.fixture"), b"current").unwrap();
        }
        let out = run_with(
            Command::new(BIN)
                .current_dir(&home)
                .env("HOME", &home)
                .env("XDG_CONFIG_HOME", home.join("config"))
                .env("XDG_DATA_HOME", &data)
                .env("XDG_RUNTIME_DIR", &runtime)
                .env_remove("DISPLAY")
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("WAYLAND_SOCKET"),
            Stdio::null(),
        );
        assert_eq!(out.status.code(), Some(1), "{label}: {}", String::from_utf8_lossy(&out.stderr));
        if current_exists {
            assert_eq!(fs::read(legacy.join("prefs.fixture")).unwrap(), b"legacy");
            assert_eq!(fs::read(current.join("prefs.fixture")).unwrap(), b"current");
        } else {
            assert!(!legacy.exists());
            assert_eq!(fs::read(current.join("prefs.fixture")).unwrap(), b"legacy");
        }
    }
    fs::remove_dir_all(root).unwrap();
}
