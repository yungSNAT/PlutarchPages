// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// A signature field takes the fields it locks (`/Lock`) and the reasons a signer must choose from
/// (`/SV /Reasons`, with `/Ff` bit 4), each an object of its own; clearing either removes it and
/// keeps the field's other seed values. Other fields, a signed signature field and a blank reason
/// are refused.
#[test]
fn a_signature_field_takes_a_lock_and_reasons_to_sign_for() {
    let mut doc = fixture();
    let sig = add_field(&mut doc, 0, [300.0, 50.0, 500.0, 100.0], &NewField::Signature, Some("Approval")).unwrap();
    let obj = fields(&doc).into_iter().find(|f| f.name == sig).unwrap().obj;
    let entry = |doc: &Document, key: &[u8]| doc.get(obj).as_dict().and_then(|d| d.get(key).map(|v| (*doc.resolve(v)).clone()));
    let texts =
        |o: Option<&Object>| -> Vec<String> { o.and_then(Object::as_array).unwrap().iter().map(|s| s.as_string().unwrap().to_text()).collect() };
    let both = |lock, reasons| FieldProps { signature_lock: Some(lock), signature_reasons: Some(reasons), ..FieldProps::default() };
    set_props(&mut doc, &sig, &both(Some(SigLock::Include(vec!["name".into(), "id".into()])), Some(vec!["Approved".into()]))).unwrap();
    let indirect = |doc: &Document, key: &[u8]| doc.get(obj).as_dict().and_then(|d| d.get(key).and_then(Object::as_ref));
    assert!(indirect(&doc, b"Lock").is_some() && indirect(&doc, b"SV").is_some());
    let lock = entry(&doc, b"Lock").unwrap();
    let lock = lock.as_dict().unwrap();
    assert_eq!((lock.name(b"Type"), lock.name(b"Action")), (Some(&b"SigFieldLock"[..]), Some(&b"Include"[..])));
    assert_eq!(texts(lock.get(b"Fields")), ["name", "id"]);
    let sv = entry(&doc, b"SV").unwrap();
    let sv = sv.as_dict().unwrap();
    assert_eq!((texts(sv.get(b"Reasons")), sv.get(b"Ff").and_then(Object::as_int)), (vec!["Approved".to_string()], Some(8)));
    set_props(&mut doc, &sig, &FieldProps { signature_lock: Some(Some(SigLock::All)), ..FieldProps::default() }).unwrap();
    let lock = entry(&doc, b"Lock").unwrap();
    assert_eq!((lock.as_dict().unwrap().name(b"Action"), lock.as_dict().unwrap().get(b"Fields")), (Some(&b"All"[..]), None));
    // Another seed value (the SubFilter, Ff bit 2) stays when the reasons go; with nothing left, so
    // does /SV. The seed values it had are left as they were.
    let seed = indirect(&doc, b"SV").unwrap();
    doc.update_dict(seed, |sv| {
        sv.set(b"SubFilter".to_vec(), Object::Array(vec![Object::name("ETSI.CAdES.detached")]));
        sv.set(b"Ff".to_vec(), Object::Int(8 | 2));
    })
    .unwrap();
    set_props(&mut doc, &sig, &both(None, None)).unwrap();
    assert_eq!(entry(&doc, b"Lock"), None);
    let sv = entry(&doc, b"SV").unwrap();
    let sv = sv.as_dict().unwrap();
    assert_eq!((sv.get(b"Reasons"), sv.get(b"Ff").and_then(Object::as_int), sv.contains(b"SubFilter")), (None, Some(2), true));
    assert!(indirect(&doc, b"SV").is_some_and(|r| r != seed) && doc.get(seed).as_dict().unwrap().contains(b"Reasons"));
    let seed = indirect(&doc, b"SV").unwrap();
    doc.update_dict(seed, |sv| {
        sv.remove(b"SubFilter");
        sv.remove(b"Ff");
    })
    .unwrap();
    set_props(&mut doc, &sig, &FieldProps { signature_reasons: Some(Some(vec!["Approved".into()])), ..FieldProps::default() }).unwrap();
    set_props(&mut doc, &sig, &FieldProps { signature_reasons: Some(None), ..FieldProps::default() }).unwrap();
    assert_eq!(entry(&doc, b"SV"), None);
    // Refused before anything is written.
    let refused = |doc: &mut Document, name: &str, props: &FieldProps| matches!(set_props(doc, name, props), Err(FormError::Invalid(_)));
    assert!(refused(&mut doc, "name", &FieldProps { signature_lock: Some(Some(SigLock::All)), ..FieldProps::default() }));
    assert!(refused(&mut doc, "name", &FieldProps { signature_reasons: Some(None), ..FieldProps::default() }));
    assert!(refused(&mut doc, &sig, &FieldProps { signature_reasons: Some(Some(vec![" ".into()])), ..FieldProps::default() }));
    assert!(refused(&mut doc, &sig, &FieldProps { signature_reasons: Some(Some(Vec::new())), ..FieldProps::default() }));
    doc.update_dict(obj, |d| d.set(b"V".to_vec(), Object::Dict(pdfcraft_cos::Dict::new()))).unwrap();
    assert!(refused(&mut doc, &sig, &FieldProps { signature_lock: Some(None), ..FieldProps::default() }));
}
