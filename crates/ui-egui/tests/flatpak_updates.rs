//! Release and source Flatpaks agree, may check for releases, and can't run commands on the host.
fn finish_args(manifest: &str) -> Vec<&str> {
    manifest
        .split("finish-args:")
        .nth(1)
        .unwrap_or_default()
        .lines()
        .take_while(|line| line.is_empty() || line.starts_with(' ') || line.starts_with('#'))
        .filter_map(|line| line.trim().strip_prefix("- "))
        .map(|arg| arg.trim_matches(['\'', '"']))
        .collect()
}

#[test]
fn flatpak_update_permissions() {
    let manifests = [
        include_str!("../../../packaging/linux/flatpak/io.github.yungSNAT.PlutarchPages.bundle.yml"),
        include_str!("../../../packaging/linux/flatpak/io.github.yungSNAT.PlutarchPages.yml"),
    ];
    let args = manifests.map(finish_args);
    assert_eq!(args[0], args[1], "source and release permissions must agree");
    for permissions in args {
        assert!(permissions.contains(&"--share=network"), "the release check needs the network");
        // PlutarchPages leaves out upstream's `--talk-name=org.freedesktop.Flatpak`: it lets the app
        // start commands outside the sandbox (`flatpak-spawn --host`), which upstream's update
        // helper uses. A new bundle is installed by hand instead (PATCHES.md).
        assert!(
            !permissions.iter().any(|arg| arg.starts_with("--talk-name=org.freedesktop.Flatpak")),
            "the Flatpak must not reach the host's Flatpak service: {permissions:?}"
        );
    }
}
