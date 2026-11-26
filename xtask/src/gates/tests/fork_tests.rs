//! The fork's tests of the oracle subprocess capture.

use std::process::Command;
use std::time::{Duration, Instant};

use super::super::{capture_oracle_image, run_capture_bytes};

/// An oracle that writes a valid 1 by 1 PGM image and exits with `exit_code`.
fn oracle(exit_code: u8) -> Command {
    #[cfg(unix)]
    {
        let mut command = Command::new("sh");
        command.arg("-c").arg(format!("printf 'P5 1 1 255 A'; exit {exit_code}"));
        command
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        let mut command = Command::new("cmd.exe");
        command.args(["/d", "/c"]).raw_arg(format!("echo P5 1 1 255 A&exit /b {exit_code}"));
        command
    }
}

#[test]
fn failed_oracle_output_is_discarded_even_when_the_image_decodes() {
    assert_eq!(capture_oracle_image(&mut oracle(0), 5), Some((1, 1, vec![b'A'])));
    assert_eq!(capture_oracle_image(&mut oracle(7), 5), None);
}

#[cfg(unix)]
#[test]
fn a_descendant_holding_stdout_cannot_outlast_the_deadline() {
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 4 & printf partial"]);
    let started = Instant::now();
    let capture = run_capture_bytes(&mut command, 1);
    let elapsed = started.elapsed();
    assert_eq!(capture, None, "partial output after the deadline is not kept");
    assert!(elapsed < Duration::from_secs(3), "returned after {elapsed:?}");
}

#[cfg(unix)]
#[test]
fn output_finished_after_the_child_exits_is_kept() {
    // The child exits at once; its descendant writes for a while longer, within the deadline.
    let mut command = Command::new("sh");
    command.args(["-c", "(sleep 0.3; printf late) &"]);
    let (status, stdout) = run_capture_bytes(&mut command, 5).expect("finished within the deadline");
    assert!(status.success());
    assert_eq!(stdout, b"late");
}
