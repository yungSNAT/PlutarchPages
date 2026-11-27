use super::*;

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_late_paper_list_does_not_interfere_with_the_selected_printer() {
    let mut app = PdfCraftApp::new();
    app.print_draft.printer = Some("Studio".into());
    app.print_draft.papers_loading = Some("Studio".into());
    let original_paper = app.print_draft.paper;
    let stale = ("Office custom".into(), (333.0, 510.0));
    app.receive_papers("Office".into(), vec![stale.clone()], Some(stale.clone()));
    assert_eq!(app.print_draft.papers_loading.as_deref(), Some("Studio"));
    assert_eq!(app.print_draft.paper, original_paper);
    assert!(!app.print_draft.papers.contains(&stale));
    assert_eq!(app.print_tasks.papers_cache["Office"].1.as_ref(), Some(&stale));

    let selected = ("Studio custom".into(), (424.0, 666.0));
    app.receive_papers("Studio".into(), vec![selected.clone()], Some(selected.clone()));
    assert_eq!(app.print_draft.papers_loading, None);
    assert_eq!(app.print_draft.papers[app.print_draft.paper], selected);
}
