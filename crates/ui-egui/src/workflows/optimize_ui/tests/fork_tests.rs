use super::*;

#[test]
fn later_stage_labels_name_their_step() {
    crate::i18n::set_current(crate::i18n::Lang::EN);
    assert_eq!(stage_label(OptimizeStage::Fragments), "Optimizing… merging image fragments");
    assert_eq!(stage_label(OptimizeStage::Images { done: 2, total: 5 }), "Optimizing… image 3 of 5");
    assert_eq!(stage_label(OptimizeStage::Images { done: 5, total: 5 }), "Optimizing… image 5 of 5");
    assert_eq!(stage_label(OptimizeStage::CleaningUp), "Optimizing… cleaning up");
    assert_eq!(stage_label(OptimizeStage::Merging), "Optimizing… merging identical objects");
    assert_eq!(stage_label(OptimizeStage::Writing), "Optimizing… writing the copy");
}
