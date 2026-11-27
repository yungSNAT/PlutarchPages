use super::*;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::DocId;
use pdfcraft_render::{DocInfo, PageInfo};

#[test]
fn search_reports_scanned_pages_until_the_results_are_complete() {
    crate::i18n::set_current(crate::i18n::Lang::EN);
    let info = DocInfo {
        pages: (0..3).map(|_| PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }).collect(),
        ..Default::default()
    };
    let mut view = DocView::new(DocId(1), &info, Default::default());
    view.find = Some(crate::canvas::Find { query: "needle".into(), case_query: "needle".into(), ..Default::default() });
    let mut ready = false;
    let mut harness = egui_kittest::Harness::builder().with_accessibility_check(false).build_ui_state(
        move |ui, view: &mut DocView| {
            if !ready {
                crate::theme::install_fonts(ui.ctx());
                ready = true;
                return;
            }
            panel(ui, &Tokens::get(ui.ctx()), view, 3);
        },
        view,
    );
    harness.run_steps(3);
    harness.get_by_label("Searching… 0 of 3 pages");

    harness.state_mut().text_failed.insert(1);
    harness.run_steps(2);
    harness.get_by_label("Searching… 1 of 3 pages");

    harness.state_mut().text_failed.extend([0, 2]);
    harness.run_steps(2);
    harness.get_by_label("0 instances");
}
