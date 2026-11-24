//! `pdfcraft-cli completions <shell>` prints shell completion scripts for bash, zsh, fish and PowerShell.

use std::io::pipe;
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn generates_completions_for_every_supported_shell() {
    let cases = [
        ("bash", "complete -F _pdfcraft_cli -o bashdefault -o default pdfcraft-cli"),
        ("zsh", "#compdef pdfcraft-cli"),
        ("fish", "complete -c pdfcraft-cli"),
        ("powershell", "Register-ArgumentCompleter -Native -CommandName @('pdfcraft-cli', 'pdfcraft-cli.exe')"),
    ];
    for (shell, marker) in cases {
        let out = run(&["completions", shell]);
        assert!(out.status.success(), "{shell}: {:?}: {}", out.status, text(&out.stderr));
        assert!(out.stderr.is_empty(), "{shell}: {}", text(&out.stderr));
        let stdout = text(&out.stdout);
        assert!(stdout.ends_with('\n'), "{shell} output should end with a newline");
        assert!(stdout.contains(marker), "{shell}: missing marker {marker}");
        for expected in [
            "info",
            "render",
            "text",
            "edit",
            "combine",
            "extract",
            "split",
            "check",
            "audit",
            "batch",
            "tools",
            "run",
            "ui",
            "completions",
            "version",
            "help",
            "powershell",
        ] {
            assert!(stdout.contains(expected), "{shell}: missing {expected}");
        }
        let compact_needle = if shell == "fish" { "-l 'compact'" } else { "--compact" };
        assert_eq!(stdout.contains(compact_needle), cfg!(feature = "mcp"), "{shell}: mcp compact mismatch");
        assert_eq!(stdout.contains("automation-read-root"), cfg!(feature = "mcp"), "{shell}: mcp read-root mismatch");
        assert_eq!(stdout.contains("automation-write-root"), cfg!(feature = "mcp"), "{shell}: mcp write-root mismatch");
        assert!(!stdout.contains("check-one"), "{shell}: internal check-one must not be offered");
    }
}

#[test]
fn completions_match_public_usage_commands_and_omit_check_one() {
    let help = run(&["--help"]);
    assert!(help.status.success(), "{:?}", help.status);
    let usage = text(&help.stdout);
    let usage_cmds: Vec<&str> =
        usage.lines().filter_map(|line| line.strip_prefix("pdfcraft-cli ")).filter_map(|rest| rest.split_whitespace().next()).collect();
    assert!(!usage_cmds.is_empty());
    assert!(!usage_cmds.contains(&"check-one"));
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let out = run(&["completions", shell]);
        assert!(out.status.success(), "{shell}: {:?}", out.status);
        let stdout = text(&out.stdout);
        assert!(!stdout.contains("check-one"), "{shell}: must not offer internal check-one");
        for cmd in &usage_cmds {
            assert!(stdout.contains(cmd), "{shell}: missing USAGE command {cmd}");
        }
    }
}

#[test]
fn help_lists_completions_and_supports_subcommand_help() {
    for args in [&["--help"][..], &["completions", "--help"], &["completions", "-h"]] {
        let out = run(args);
        assert!(out.status.success(), "{args:?}: {:?}", out.status);
        let stdout = text(&out.stdout);
        assert!(stdout.contains("pdfcraft-cli completions <bash|zsh|fish|powershell>"), "{args:?}: {stdout}");
        assert!(out.stderr.is_empty(), "{args:?}: {}", text(&out.stderr));
    }
}

#[test]
fn invalid_completions_invocations_fail_with_actionable_errors() {
    let missing = run(&["completions"]);
    assert!(!missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(text(&missing.stderr).contains("missing shell"));

    let unknown = run(&["completions", "csh"]);
    assert!(!unknown.status.success());
    assert!(unknown.stdout.is_empty());
    assert!(text(&unknown.stderr).contains("unknown shell 'csh'"));

    let extra = run(&["completions", "bash", "zsh"]);
    assert!(!extra.status.success());
    assert!(extra.stdout.is_empty());
    assert!(text(&extra.stderr).contains("unexpected argument `zsh`"));

    let flag = run(&["completions", "--bogus"]);
    assert!(!flag.status.success());
    assert!(flag.stdout.is_empty());
    assert!(text(&flag.stderr).contains("unknown option --bogus"));
}

#[test]
fn closed_stdout_pipe_exits_cleanly() {
    let (reader, writer) = pipe().unwrap();
    drop(reader);
    let out = Command::new(BIN).args(["completions", "bash"]).stdout(writer).stderr(Stdio::piped()).spawn().unwrap().wait_with_output().unwrap();
    assert!(out.status.success(), "{}: {}", out.status, text(&out.stderr));
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
}

#[cfg(target_os = "linux")]
#[test]
fn full_stdout_fails_without_panicking() {
    let full = std::fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
    let out = Command::new(BIN).args(["completions", "bash"]).stdout(full).stderr(Stdio::piped()).spawn().unwrap().wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("stdout") && !stderr.contains("panicked"), "{stderr}");
}
