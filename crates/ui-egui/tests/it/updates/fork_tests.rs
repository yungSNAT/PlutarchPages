// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// A launcher's version choice for upstream's `pdfcraft` id must not stop this fork offering its own
/// releases. The file is named by an environment variable, so the check runs in a child process.
#[test]
fn a_launcher_pin_on_the_shared_id_does_not_stop_update_offers() {
    const CHILD: &str = "PLUTARCHPAGES_PIN_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let file = std::env::temp_dir().join(format!("plutarchpages-pin-test-{}.json", std::process::id()));
        std::fs::write(&file, r#"{"apps":{"pdfcraft":{"Pinned":"v0.1.0"}}}"#).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "updates::a_launcher_pin_on_the_shared_id_does_not_stop_update_offers"])
            .env("CRAFT_VERSIONS_FILE", &file)
            .env(CHILD, "1")
            .status();
        std::fs::remove_file(&file).unwrap();
        assert!(status.unwrap().success(), "the pinned check didn't offer the release");
        return;
    }
    let mut h = harness(Ok(Some("v99.0.0")));
    h.state_mut().execute("help.check_updates");
    settle(&mut h);
    h.get_by_label_contains("PlutarchPages 99.0.0 is available");
}

/// Only this fork's own release pages are offered: upstream PdfCraft builds lack its changes, and
/// craft-update's check falls back to the release list for anything else.
#[test]
fn answers_are_read_and_only_our_release_pages_are_offered() {
    use craft_update::release::safe_release_html_url;
    let repo = pdfcraft_ui_egui::UPDATE_APP.repo;
    assert_eq!(pdfcraft_ui_egui::UPDATE_APP.releases_url(), pdfcraft_ui_egui::updates::RELEASES_PAGE);
    let ours = "https://github.com/yungSNAT/PlutarchPages/releases/tag/v0.2.0";
    assert_eq!(safe_release_html_url(repo, ours).as_deref(), Some(ours));
    for elsewhere in [
        "https://example.com/pdfcraft.exe",
        "https://github.com/yungSNAT/PlutarchPages/releases.evil/x",
        "https://github.com/storytold/pdfcraft/releases/tag/v0.15.0",
        // Browsers resolve dot segments, so these would land on upstream's page.
        "https://github.com/yungSNAT/PlutarchPages/releases/../../../storytold/pdfcraft/releases/tag/v0.15.0",
        "https://github.com/yungSNAT/PlutarchPages/releases/tag/%2e%2e/%2e%2e/%2e%2e/%2e%2e/storytold/pdfcraft",
        "https://github.com/yungSNAT/PlutarchPages/releases/./tag/v1",
        // A tab or a newline is removed by URL parsing: `.\t.` is `..`.
        "https://github.com/yungSNAT/PlutarchPages/releases/.\t./.\t./.\t./storytold/pdfcraft/releases/tag/v1",
        "https://github.com/yungSNAT/PlutarchPages/releases/.\n./.\n./.\n./storytold/pdfcraft/releases/tag/v1",
        "https://github.com/yungSNAT/PlutarchPages/releases/tag/v1 x",
        "javascript:alert(1)",
    ] {
        assert_eq!(safe_release_html_url(repo, elsewhere), None, "{elsewhere}");
    }
}
