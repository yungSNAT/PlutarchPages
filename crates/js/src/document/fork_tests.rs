//! The fork's tests of document and field accessors.

use crate::{DocInfo, Event, FieldState, FieldType, Limits, Outcome, Request, run};

fn sample_fields() -> Vec<FieldState> {
    let mut agree = FieldState::new("agree", FieldType::CheckBox, vec![]);
    agree.options = vec![("Yes".into(), "Yes".into())];
    let mut size = FieldState::new("size", FieldType::ComboBox, vec!["m".into()]);
    size.options = vec![("s".into(), "Small".into()), ("m".into(), "Medium".into())];
    let btn = FieldState::new("btn", FieldType::Button, vec![]);
    let price = FieldState::new("price", FieldType::Text, vec!["10".into()]);
    vec![agree, size, btn, price]
}

fn go(script: &str, event: &Event) -> Outcome {
    run(script, event, &DocInfo { num_pages: 2, ..Default::default() }, &sample_fields(), &[], Limits::default())
}

#[test]
fn check_boxes_flags_focus_export_values_and_field_names() {
    let out = go(
        "var a = getField('agree'); \
         var offStr = a.valueAsString; \
         a.checkThisBox(0, true); var onStr = a.valueAsString; \
         a.checkThisBox(0, false); var clearedStr = a.valueAsString; \
         var p = getField('price'); \
         p.required = true; p.hidden = true; var hid1 = p.hidden; p.hidden = false; var hid2 = p.hidden; \
         p.setFocus(); \
         var ev = getField('size').exportValues.join(','); \
         var nth0 = getNthFieldName(0); var nthOob = getNthFieldName(99); \
         mailDoc(); syncAnnotScan(); dirty(); console.show(); \
         [offStr, onStr, clearedStr, p.required, hid1, hid2, ev, nth0, String(nthOob)].join('|')",
        &Event::doc("Console"),
    );
    assert_eq!(out.error, None, "{:?}", out.error);
    assert_eq!(out.result.as_deref(), Some("Off|Yes|Off|true|true|false|s,m|agree|null"));
    assert!(out.requests.contains(&Request::Focus("price".into())));
}

#[test]
fn field_setters_refuse_out_of_range_values() {
    for (script, needle) in [
        ("getField('price').textSize = 999;", "textSize"),
        ("getField('price').lineWidth = 99;", "lineWidth"),
        ("getField('price').borderStyle = 'dotted';", "borderStyle"),
        ("getField('price').userName = 'x'.repeat(4097);", "userName"),
        ("getField('price').rect = [0, 10, 20];", "four coordinates"),
        ("getField('price').setAction('Validate', '('.repeat(100));", "nested too deeply"),
        ("getField('size').setItems(new Array(200).fill('x'.repeat(6000)));", "too long"),
    ] {
        let out = go(script, &Event::doc("Console"));
        assert!(out.error.as_deref().is_some_and(|e| e.contains(needle)), "{script} -> {:?}", out.error);
    }
}

#[test]
fn a_format_event_cannot_change_a_button_caption() {
    let out = go("getField('btn').buttonSetCaption('Hi');", &Event::field("Format", "btn", ""));
    assert!(out.error.as_deref().is_some_and(|e| e.contains("Format event")), "{:?}", out.error);
    assert!(out.requests.is_empty());
}

#[test]
fn appearance_setters_store_what_they_are_given() {
    let out = go(
        "var p = getField('price'); \
         p.fillColor = ['RGB', 1, 0, 0]; p.strokeColor = ['G', 0.5]; p.textSize = 12; p.lineWidth = 2; \
         p.borderStyle = 'dashed'; p.userName = 'Price'; p.display = 9; p.readonly = true; \
         [p.fillColor.join(','), p.strokeColor.join(','), p.textSize, p.lineWidth, p.borderStyle, p.userName, p.display, p.readonly].join('|')",
        &Event::doc("Console"),
    );
    assert_eq!(out.error, None, "{:?}", out.error);
    // A display past the last code is the last code (3, noView).
    assert_eq!(out.result.as_deref(), Some("RGB,1,0,0|G,0.5|12|2|dashed|Price|3|true"));
    assert!(out.changed.iter().any(|f| f.name == "price" && f.readonly && f.user_name == "Price" && f.text_size == 12.0));
}

#[test]
fn document_counts_page_number_submit_and_templates() {
    let doc = DocInfo { num_pages: 3, page: 1, templates: vec![("T".into(), false)], ..Default::default() };
    let out = run(
        "submitForm('https://example.test/a'); submitForm({ cURL: 'https://example.test/b' }); \
         var t = this.getTemplate('T'); t.hidden = true; \
         [this.numFields, this.pageNum, t.name, t.hidden, String(this.getTemplate('none'))].join('|')",
        &Event::doc("Console"),
        &doc,
        &sample_fields(),
        &[],
        Limits::default(),
    );
    assert_eq!(out.error, None, "{:?}", out.error);
    assert_eq!(out.result.as_deref(), Some("4|1|T|true|null"));
    for r in [
        Request::Submit("https://example.test/a".into()),
        Request::Submit("https://example.test/b".into()),
        Request::SetTemplateHidden { name: "T".into(), hidden: true },
    ] {
        assert!(out.requests.contains(&r), "{r:?} in {:?}", out.requests);
    }
}

#[test]
fn a_field_with_an_object_id_is_focused_by_it() {
    let mut fields = sample_fields();
    fields[3].object_id = Some(77);
    let out = run("getField('price').setFocus();", &Event::doc("Console"), &DocInfo::default(), &fields, &[], Limits::default());
    assert_eq!(out.error, None, "{:?}", out.error);
    assert!(out.requests.contains(&Request::FocusField { name: "price".into(), object_id: 77 }), "{:?}", out.requests);
}
