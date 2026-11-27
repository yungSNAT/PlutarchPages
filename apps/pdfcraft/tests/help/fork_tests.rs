use super::*;

#[test]
fn startup_migrates_legacy_storage_only_when_current_storage_is_absent() {
    use std::fs;
    use std::os::unix::fs::DirBuilderExt;

    // Cargo's per-target scratch folder, so no other user can plant the path.
    let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("pdfcraft-migration-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
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
