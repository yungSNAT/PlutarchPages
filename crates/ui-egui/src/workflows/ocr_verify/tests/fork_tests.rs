use super::*;

#[test]
fn review_intents_correct_verify_and_restore_the_selected_word() {
    let (mut app, _) = app_review(vec![vec![word("teh", Some(18.0)), word("other", Some(34.0))]]);
    let first = (0, 0);
    let second = (0, 1);
    apply(&mut app, Intents { select_word: Some(first), correct: Some("the".into()), ..Default::default() });
    assert_eq!(app.ocr_verify.text_of(first), Some("the"));
    assert_eq!(app.ocr_verify.correction, "the");

    apply(&mut app, Intents { set_word_verified: Some(true), dictionary_add: true, ..Default::default() });
    assert!(app.ocr_verify.review.verified.contains(&first));
    assert!(app.ocr_verify.review.dictionary.contains("the"));
    apply(&mut app, Intents { next_uncertain: true, ..Default::default() });
    assert_eq!(app.ocr_verify.selected, Some(second));

    apply(&mut app, Intents { set_removed: Some(true), ..Default::default() });
    assert!(app.ocr_verify.is_removed(second));
    apply(&mut app, Intents { undo: true, ..Default::default() });
    assert!(!app.ocr_verify.is_removed(second));
    assert_eq!(app.ocr_verify.text_of(second), Some("other"));

    apply(&mut app, Intents { prev_uncertain: true, ..Default::default() });
    assert_eq!(app.ocr_verify.selected, Some(second));
}

#[test]
fn review_intents_clear_restore_and_apply_only_the_current_page() {
    let (mut app, id) = app_review(vec![vec![word("first", Some(95.0))], vec![word("second", Some(95.0))]]);
    app.ocr_verify.select((0, 0));
    apply(&mut app, Intents { select_page: Some(1), toggle_page_verified: true, ..Default::default() });
    assert_eq!(app.ocr_verify.current, 1);
    assert_eq!(app.ocr_verify.selected, None);
    assert!(app.ocr_verify.review.verified_pages.contains(&1));

    apply(&mut app, Intents { reject_page: true, ..Default::default() });
    assert!(app.ocr_verify.page(1).expect("current page").words.is_empty());
    assert_eq!(app.ocr_verify.text_of((0, 0)), Some("first"));
    assert!(toast(&mut app).contains("were cleared"));

    apply(&mut app, Intents { undo: true, ..Default::default() });
    assert_eq!(app.ocr_verify.text_of((1, 0)), Some("second"));

    apply(&mut app, Intents { accept_page: true, ..Default::default() });
    assert!(toast(&mut app).contains("one undoable step"));
    apply(&mut app, Intents { accept_page: true, ..Default::default() });
    assert!(toast(&mut app).is_empty());
    assert!(page_text(&app.session, id, 0).is_empty());
    assert_eq!(page_text(&app.session, id, 1).matches("second").count(), 1);
}
