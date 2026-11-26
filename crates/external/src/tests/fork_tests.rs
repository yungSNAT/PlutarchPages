//! The fork's tests of the external program checks.

use super::*;

#[cfg(unix)]
#[test]
fn a_configured_program_requires_execute_permission() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("office");
    std::fs::write(&program, b"synthetic program").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(find_office(Some(&program)), Err(ExternalError::Unavailable(_))), "a file without execute permission is refused");

    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(find_office(Some(&program)).unwrap(), program);
}
