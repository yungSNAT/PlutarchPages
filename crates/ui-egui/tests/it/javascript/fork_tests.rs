// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// A save landing whilst a button's script waits ends that script: its menu goes, and what it
/// would have written reaches neither the field nor the saved file.
#[test]
fn a_save_whilst_a_button_script_runs_discards_the_script() {
    let mut h = harness();
    let id = h.state().active_ids().unwrap().1;
    // The build's own scratch folder, not the shared temporary one.
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("save-during-script-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("form.pdf");
    h.state_mut().save_override = Some(path.to_string_lossy().into_owned());
    let before = value(&h, "greeting");
    h.state_mut().run_button_script(id, "hello", "app.popUpMenuEx({cName:'Wait',cReturn:'yes'});getField('greeting').value='stale';");
    wait_for(&mut h, "Wait");
    assert!(h.state_mut().save_active(pdfcraft_ui_egui::SaveTarget::InPlace));
    h.run_steps(5);
    assert!(h.query_by_label("Wait").is_none(), "the menu of a script the save ended must go");
    // The worker stays reserved until the abandoned menu exits; the script's write never lands.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while h.state_mut().session.run_javascript(id, "1", None).unwrap().execution_refused {
        assert!(std::time::Instant::now() < deadline, "the abandoned worker did not finish");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
    }
    h.run_steps(5);
    assert_eq!(value(&h, "greeting"), before);
    assert!(!h.state().session.get(id).unwrap().dirty, "nothing changed after the save");
    let saved = std::fs::read(&path).unwrap();
    assert!(!saved.windows(5).any(|w| w == b"stale"));
    std::fs::remove_dir_all(&dir).ok();
}
