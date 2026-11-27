//! The fork's tests of the Field Properties tabs, driven through the drawn widgets.

use super::*;
use crate::theme::Tokens;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{FormField, FormFieldKind, FormWidget};

fn text_field() -> FormField {
    FormField {
        name: "amount".into(),
        obj: pdfcraft_cos::ObjRef::new(4, 0),
        kind: FormFieldKind::Text,
        value: Vec::new(),
        default: Vec::new(),
        flags: 0,
        max_len: None,
        options: Vec::new(),
        selected: Vec::new(),
        single_value_is_array: false,
        da: String::new(),
        quadding: 0,
        tooltip: None,
        widgets: vec![FormWidget {
            mouse_up_script: None,
            mouse_down: false,
            obj: pdfcraft_cos::ObjRef::new(4, 0),
            page: Some(0),
            rect: [20.0, 110.0, 140.0, 130.0],
            on_state: None,
            state: None,
            tab: 0,
            locked: false,
            hidden: false,
            read_only: false,
            rotation: 0,
            caption: None,
            list: None,
            round: false,
        }],
        actions: Default::default(),
        button: None,
    }
}

/// A harness drawing `tab` over a text field's draft that starts with `format` and `validate`.
fn harness<'a>(format: Format, validate: Validate, tab: fn(&mut egui::Ui, &mut FieldDraft)) -> egui_kittest::Harness<'a, FieldDraft> {
    crate::i18n::set_current(crate::i18n::Lang::EN);
    let mut draft = FieldDraft::new(&text_field(), 0);
    draft.format = format;
    draft.validate = validate;
    let mut ready = false;
    let mut harness = egui_kittest::Harness::builder().with_accessibility_check(false).build_ui_state(
        move |ui, d: &mut FieldDraft| {
            if !ready {
                crate::theme::install_fonts(ui.ctx());
                ready = true;
                return;
            }
            tab(ui, d);
        },
        draft,
    );
    harness.run_steps(3);
    harness
}

fn format(ui: &mut egui::Ui, d: &mut FieldDraft) {
    format_tab(ui, d, &Tokens::get(ui.ctx()));
}

#[test]
fn picking_a_category_gives_its_default_format_and_an_example() {
    let mut h = harness(Format::None, Validate::None, format);
    h.get_by_label("The value is shown as typed.");
    h.get_by_label("None").click();
    h.run_steps(2);
    h.get_by_label("Percentage").click();
    h.run_steps(3);
    let picked = Format::Percent { decimals: 2, sep: 0 };
    assert_eq!(h.state().format, picked);
    h.get_by_label("Decimal places:");
    h.get_by_label(&format!("Example: {}", format_value(&picked, "-1234.5")));
}

#[test]
fn the_arbitrary_mask_entry_switches_a_special_format_to_a_mask_and_back() {
    let mut h = harness(Format::Special(0), Validate::None, format);
    h.get_by_label("Zip Code").click();
    h.run_steps(2);
    h.get_by_label("Arbitrary Mask").click();
    h.run_steps(3);
    assert_eq!(h.state().format, Format::Mask("999-999".into()));
    h.get_by_label("Mask:");

    h.get_by_label("Arbitrary Mask").click();
    h.run_steps(2);
    h.get_by_label("Phone Number").click();
    h.run_steps(3);
    assert_eq!(h.state().format, Format::Special(2));
}

#[test]
fn a_range_starts_at_0_to_100_and_each_end_can_be_left_open() {
    let mut h = harness(Format::None, Validate::None, validate_tab);
    h.get_by_label("Field value is in range:").click();
    h.run_steps(2);
    assert_eq!(h.state().validate, Validate::Range { min: Some(0.0), max: Some(100.0) });

    h.get_by_label("From:").click();
    h.run_steps(2);
    assert_eq!(h.state().validate, Validate::Range { min: None, max: Some(100.0) });

    h.get_by_label("Field value is not validated").click();
    h.run_steps(2);
    assert_eq!(h.state().validate, Validate::None);
}
