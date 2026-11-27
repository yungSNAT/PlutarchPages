//! The fork's tests of the stable names tools and logs read.

use super::*;

#[test]
fn every_field_type_icon_and_button_set_has_its_name() {
    let types = [
        (FieldType::Text, "text"),
        (FieldType::CheckBox, "checkbox"),
        (FieldType::RadioButton, "radiobutton"),
        (FieldType::ComboBox, "combobox"),
        (FieldType::ListBox, "listbox"),
        (FieldType::Button, "button"),
        (FieldType::Signature, "signature"),
    ];
    for (t, name) in types {
        assert_eq!(t.name(), name);
    }
    for (code, name) in [(None, "error"), (Some(1.0), "warning"), (Some(2.0), "question"), (Some(3.0), "status")] {
        assert_eq!(MessageIcon::from_code(code).name(), name);
    }
    for (code, name) in [(None, "ok"), (Some(1.0), "ok_cancel"), (Some(2.0), "yes_no"), (Some(3.0), "yes_no_cancel"), (Some(1.5), "ok")] {
        assert_eq!(MessageButtons::from_code(code).name(), name);
    }
}
