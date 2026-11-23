// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// A reply's inline editor, left open with a draft, never saves the draft over another reply and
/// never loses it. The editor's own typing history follows its reply when a reply above it is
/// deleted, so Undo takes the typing back first. Past that history, Undo saves the draft to its
/// own reply, as a step Undo takes back and Redo puts back, then closes the editor: the step can
/// put a reply back in its place or take one away, and the editor would have saved the draft over
/// whichever reply took that place. Redo leaves an unsaved draft open: saving it first would drop
/// the Redo. Undo used to close the editor and drop the draft (R0646).
#[test]
fn undo_and_redo_keep_a_reply_draft_left_open() {
    let mut h = harness_with_bytes(THREE_REPLIES.to_vec(), |app| {
        app.set_option("panel", "comments").unwrap();
        app.views[0].comments.selected = Some((0, 0));
    });
    let texts = |h: &Harness<'static, PdfCraftApp>| -> Vec<String> { comments(h).iter().filter_map(|a| a.contents.clone()).collect() };
    let all = ["The comment", "Reply A", "Reply B", "Reply C"];
    assert_eq!(texts(&h), all);

    draft_reply(&mut h, "Reply B", "B's draft");
    reply_options(&h, "Reply A").click();
    h.run_steps(2);
    h.get_by_label("Delete").click();
    h.run_steps(3);
    assert_eq!(texts(&h), ["The comment", "Reply B", "Reply C"]);
    assert_eq!(h.state().views[0].comments.editing, Some((0, 1, "B's draft".to_string())), "the editor follows B up");

    h.state_mut().undo();
    h.run_steps(3);
    assert_eq!(h.state().views[0].comments.editing, Some((0, 1, "Reply B".to_string())), "undo takes the typing back in the editor");
    assert_eq!(texts(&h), ["The comment", "Reply B", "Reply C"], "and leaves A deleted");
    h.state_mut().redo();
    h.run_steps(3);
    assert_eq!(h.state().views[0].comments.editing, Some((0, 1, "B's draft".to_string())), "redo puts the typing back");

    // Past the editor's own history (here, its state is gone), Undo saves the draft first.
    h.ctx.data_mut(|d| d.clear());
    h.state_mut().undo();
    h.run_steps(3);
    assert_eq!(texts(&h), ["The comment", "Reply B", "Reply C"], "undo takes back the draft, saved to B first");
    assert_eq!(h.state().views[0].comments.editing, None, "the editor closes on undo");
    assert_eq!(h.query_all_by_label("Save").count(), 0, "no draft is left to save over another reply");
    h.state_mut().redo();
    h.run_steps(3);
    assert_eq!(texts(&h), ["The comment", "B's draft", "Reply C"], "redo puts the draft back, saved to B");
    h.state_mut().undo();
    h.run_steps(2);
    h.state_mut().undo();
    h.run_steps(3);
    assert_eq!(texts(&h), all, "the next undo puts A back");

    h.state_mut().views[0].comments.selected = Some((0, 0));
    h.run_steps(2);
    draft_reply(&mut h, "Reply B", "B's second draft");
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::unique("test-menu")));
    h.state_mut().redo();
    h.run_steps(3);
    assert_eq!(texts(&h), all, "redo waits while the editor holds a draft");
    assert_eq!(h.state().views[0].comments.editing, Some((0, 2, "B's second draft".to_string())), "the draft stays");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    h.state_mut().redo();
    h.run_steps(3);
    assert_eq!(texts(&h), ["The comment", "Reply B", "Reply C"], "with the draft cancelled, redo deletes A again");
}

/// Undo from the menu or toolbar with a draft in the Comments panel's editor: the editor takes
/// the typing back and stays open, and Redo puts it back, as for a box on the page; the document
/// is untouched. The menu closed the editor and dropped the draft (R0646).
#[test]
fn undo_from_the_menu_takes_back_typing_in_a_comment_editor() {
    let mut h = harness_with_bytes(THREE_REPLIES.to_vec(), |app| {
        app.set_option("panel", "comments").unwrap();
        app.views[0].comments.selected = Some((0, 0));
    });
    draft_reply(&mut h, "Reply B", "B's draft");
    h.ctx.memory_mut(|m| m.request_focus(egui::Id::unique("test-menu")));
    assert!(h.state_mut().execute("edit.undo"), "Undo is enabled for the editor's typing");
    h.run_steps(3);
    assert_eq!(h.state().views[0].comments.editing, Some((0, 2, "Reply B".to_string())), "the editor takes the typing back and stays open");
    assert!(h.state_mut().execute("edit.redo"), "Redo is enabled for the editor's own step");
    h.run_steps(3);
    assert_eq!(h.state().views[0].comments.editing, Some((0, 2, "B's draft".to_string())), "Redo puts the typing back");
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), None, "the document is untouched");
}

/// The editor starts with no typing history each time it opens, on the same reply or another:
/// Undo never puts back what an earlier, cancelled edit typed.
#[test]
fn a_reopened_comment_editor_has_no_earlier_typing_to_undo() {
    let mut h = harness_with_bytes(THREE_REPLIES.to_vec(), |app| {
        app.set_option("panel", "comments").unwrap();
        app.views[0].comments.selected = Some((0, 0));
    });
    for reopen in ["Reply B", "Reply C"] {
        draft_reply(&mut h, "Reply B", "B's draft");
        h.get_by_label("Cancel").click();
        h.run_steps(2);
        reply_options(&h, reopen).click();
        h.run_steps(2);
        h.query_all_by_label("Edit").last().expect("the reply menu's Edit, after the toolbar tab").click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        h.run_steps(3);
        let index = if reopen == "Reply B" { 2 } else { 3 };
        assert_eq!(h.state().views[0].comments.editing, Some((0, index, reopen.to_string())), "{reopen}: nothing to undo in the editor");
        h.get_by_label("Cancel").click();
        h.run_steps(2);
    }
}

/// Screen-reader mode lays out every card of a long list, so a screen reader reaches the cards
/// out of view too (PATCHES.md; upstream's #5488 lays out only the cards in view).
#[test]
fn screen_reader_mode_lays_out_every_comment_card() {
    let mut h = harness_with_bytes(one_comment_per_page(120), |app| app.set_option("panel", "comments").unwrap());
    h.ctx.memory_mut(|m| m.options.screen_reader = true);
    h.run_steps(4);
    assert!(h.query_by_label("Card 120").is_some(), "the last card is far out of view and still laid out");
}
