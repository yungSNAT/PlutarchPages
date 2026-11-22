// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// One page with the signature fields "Approval" (6, with `approval` added to it) and "Seller" (9,
/// with `seller`), and the text fields "Buyer" (7) and "Notes" (8).
fn locking_fixture(approval: &str, seller: &str) -> Vec<u8> {
    let approval = format!("<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 {approval} >>");
    let seller = format!("<< /Type /Annot /Subtype /Widget /FT /Sig /T (Seller) /Rect [150 80 280 130] /P 3 0 R /F 4 {seller} >>");
    write_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 7 0 R 8 0 R 9 0 R] >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R 7 0 R 8 0 R 9 0 R] >>",
        b"<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        approval.as_bytes(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (Buyer) /V (Ann) /Rect [20 20 120 40] /P 3 0 R >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (Notes) /V (none) /Rect [20 50 120 70] /P 3 0 R >>",
        seller.as_bytes(),
    ])
}

/// A `/Lock` that locks the fields `names` (written as PDF strings).
fn lock_of(names: &str) -> String {
    format!("/Lock << /Type /SigFieldLock /Action /Include /Fields [{names}] >>")
}

fn set_value(num: u32, value: &'static str) -> impl FnOnce(&mut Document) {
    move |doc| doc.update_dict(ObjRef { num, generation: 0 }, |d| d.set(b"V".to_vec(), PdfString::text(value))).unwrap()
}

/// Signing a field that locks others makes them read-only in the signed revision and records the
/// lock in a FieldMDP reference, after any DocMDP one. Validation then refuses a later change to a
/// locked field, though an approval signature or a form-fill certification permits form fill,
/// while the fields it doesn't lock can still be filled.
#[test]
fn a_signature_fields_lock_is_kept_by_signing_and_checked_by_validation() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let fixture = locking_fixture(&lock_of("(Buyer)"), "");
    for certify in [None, Some(2)] {
        let signed = pdfcraft_sign::sign(&open(&fixture), &id, &SignOptions { field: Some("Approval".into()), certify, ..opts() }).unwrap();
        let doc = open(&signed);
        let flags = |num| doc.get(ObjRef { num, generation: 0 }).as_dict().and_then(|d| d.get(b"Ff").and_then(Object::as_int)).unwrap_or(0) & 1;
        assert_eq!((flags(7), flags(8), flags(6)), (1, 0, 0), "only Buyer is read-only ({certify:?})");
        let field = doc.get(ObjRef { num: 6, generation: 0 });
        let v = doc.resolve(field.as_dict().unwrap().get(b"V").unwrap());
        let methods: Vec<Vec<u8>> = doc
            .resolve(v.as_dict().unwrap().get(b"Reference").unwrap())
            .as_array()
            .unwrap()
            .iter()
            .map(|r| doc.resolve(r).as_dict().unwrap().name(b"TransformMethod").unwrap().to_vec())
            .collect();
        let expected: Vec<&[u8]> = if certify.is_some() { vec![b"DocMDP", b"FieldMDP"] } else { vec![b"FieldMDP"] };
        assert_eq!(methods, expected);
        let approval = |bytes: &[u8]| signatures(&open(bytes), bytes, &TrustStore::default()).into_iter().find(|s| s.field == "Approval").unwrap();
        assert_eq!(approval(&signed).modification, Modification::None);
        assert_eq!(approval(&edit_after(&signed, set_value(8, "later"))).modification, Modification::Allowed(vec![kind::FORM_FILL]));
        assert_eq!(
            approval(&edit_after(&signed, set_value(7, "Bob"))).modification,
            Modification::Disallowed(vec![kind::LOCKED_FORM_FIELDS]),
            "{certify:?}"
        );
    }
}

/// Signers in order: the second signature's lock repeats the first one's and adds the first
/// signature field, and leaves the first signature's locked fields as they were. Out of order, a
/// field an earlier signature locked can no longer be signed. A lock PdfCraft can't read refuses
/// the signing rather than locking less.
#[test]
fn signers_in_order_keep_each_lock_and_a_locked_field_cannot_be_signed() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let sign_field = |bytes: &[u8], field: &str| pdfcraft_sign::sign(&open(bytes), &id, &SignOptions { field: Some(field.into()), ..opts() });
    let first = sign_field(&locking_fixture(&lock_of("(Buyer)"), &lock_of("(Buyer) (Approval)")), "Approval").unwrap();
    let second = sign_field(&first, "Seller").unwrap();
    let sigs = signatures(&open(&second), &second, &TrustStore::default());
    let modification = |field: &str| sigs.iter().find(|s| s.field == field).unwrap().modification.clone();
    assert!(matches!(modification("Approval"), Modification::Allowed(k) if !k.contains(&kind::LOCKED_FORM_FIELDS)), "{:?}", modification("Approval"));
    assert_eq!(modification("Seller"), Modification::None);
    let locks_seller = sign_field(&locking_fixture(&lock_of("(Seller)"), ""), "Approval").unwrap();
    assert!(matches!(sign_field(&locks_seller, "Seller"), Err(SignError::Pdf(m)) if m.contains("read-only")));
    let unreadable = locking_fixture("/Lock << /Type /SigFieldLock /Action /Some >>", "");
    assert!(matches!(sign_field(&unreadable, "Approval"), Err(SignError::Pdf(m)) if m.contains("can't read")));
}

/// A field whose seed values require a reason (`/SV /Ff` bit 4) is signed only for one of its
/// `/Reasons`, and for none when that list is a single period; without the bit they are
/// suggestions.
#[test]
fn a_field_asking_for_a_reason_is_signed_only_for_one_of_them() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let sign_for = |sv: &str, reason: Option<&str>| {
        let doc = open(&locking_fixture(sv, ""));
        pdfcraft_sign::sign(&doc, &id, &SignOptions { field: Some("Approval".into()), reason: reason.map(Into::into), ..opts() })
    };
    let required = "/SV << /Type /SV /Reasons [(Approved as buyer) (Witnessed)] /Ff 8 >>";
    assert!(sign_for(required, Some("Witnessed")).is_ok());
    for reason in [Some("I approve this document"), None] {
        assert!(matches!(sign_for(required, reason), Err(SignError::Pdf(m)) if m.contains("Approved as buyer; Witnessed")), "{reason:?}");
    }
    assert!(sign_for("/SV << /Type /SV /Reasons [(Witnessed)] >>", Some("Another")).is_ok());
    let none = "/SV << /Type /SV /Reasons [(.)] /Ff 8 >>";
    assert!(sign_for(none, None).is_ok());
    assert!(sign_for(none, Some("Witnessed")).is_err());
}

/// One page with the signature field "Approval" (6, with `lock` added to it), the fields
/// "Party.Buyer" (7, its value the string object 11 and its appearance 12, which draws the form
/// 13, whose font is 15) and "Party.Notes" (8) under "Party" (10), and the field "Total" (9). The
/// form (14) is an object of its own.
fn locked_parts_fixture(lock: &str) -> Vec<u8> {
    let approval = format!("<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 {lock} >>");
    let stream = |resources: &str, body: &str| {
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 100 20] /Resources {resources} /Length {} >>\nstream\n{body}\nendstream", body.len())
    };
    let appearance = stream("<< /XObject << /X1 13 0 R >> >>", "/X1 Do");
    let drawn = stream("<< /Font << /F2 15 0 R >> >>", "BT /F2 10 Tf 2 5 Td (Ann) Tj ET");
    write_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm 14 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R 7 0 R 8 0 R 9 0 R] >>",
        b"<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        approval.as_bytes(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (Buyer) /Parent 10 0 R /V 11 0 R /Rect [20 20 120 40] /P 3 0 R /AP << /N 12 0 R >> >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (Notes) /Parent 10 0 R /V (none) /Rect [20 50 120 70] /P 3 0 R >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (Total) /V (10) /Rect [150 150 250 170] /P 3 0 R >>",
        b"<< /T (Party) /Kids [7 0 R 8 0 R] >>",
        b"(Ann)",
        appearance.as_bytes(),
        drawn.as_bytes(),
        b"<< /Fields [6 0 R 10 0 R 9 0 R] >>",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
    ])
}

fn ff(doc: &Document, num: u32) -> Option<i64> {
    doc.get(ObjRef { num, generation: 0 }).as_dict().and_then(|d| d.get(b"Ff").and_then(Object::as_int))
}

/// A locked field is more than its own object: a change to its value written as an object of its
/// own, to its parent's name, or to a font its appearance's nested form uses, or the field replaced
/// in the form by another of its name, is a change to a locked field, while a field the lock
/// leaves out can still be filled.
#[test]
fn what_a_locked_field_is_made_of_is_locked_with_it() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let fixture = locked_parts_fixture(&lock_of("(Party.Buyer) (Total)"));
    fn r(num: u32) -> ObjRef {
        ObjRef { num, generation: 0 }
    }
    for certify in [None, Some(2)] {
        let signed = pdfcraft_sign::sign(&open(&fixture), &id, &SignOptions { field: Some("Approval".into()), certify, ..opts() }).unwrap();
        let approval = |bytes: &[u8]| signatures(&open(bytes), bytes, &TrustStore::default()).into_iter().find(|s| s.field == "Approval").unwrap();
        let locked = |edit: Box<dyn FnOnce(&mut Document)>| {
            let m = approval(&edit_after(&signed, edit)).modification;
            matches!(&m, Modification::Disallowed(k) if k.contains(&kind::LOCKED_FORM_FIELDS)).then_some(()).ok_or(m)
        };
        assert_eq!(locked(Box::new(|doc| doc.set(r(11), PdfString::text("Bob")))), Ok(()), "value ({certify:?})");
        assert_eq!(
            locked(Box::new(|doc| doc.update_dict(r(10), |d| d.set(b"T".to_vec(), PdfString::text("Side"))).unwrap())),
            Ok(()),
            "parent ({certify:?})"
        );
        assert_eq!(
            locked(Box::new(|doc| doc.update_dict(r(15), |d| d.set(b"BaseFont".to_vec(), Object::name("Times-Roman"))).unwrap())),
            Ok(()),
            "nested appearance font ({certify:?})"
        );
        let replaced = |doc: &mut Document| {
            let total = doc.add(Object::Dict(doc.get(r(9)).as_dict().unwrap().clone()));
            doc.update_dict(total, |d| d.set(b"V".to_vec(), PdfString::text("99"))).unwrap();
            doc.update_dict(r(14), |d| d.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(r(6)), Object::Ref(r(10)), Object::Ref(total)])))
                .unwrap();
        };
        assert_eq!(locked(Box::new(replaced)), Ok(()), "replaced ({certify:?})");
        assert_eq!(approval(&edit_after(&signed, set_value(8, "later"))).modification, Modification::Allowed(vec![kind::FORM_FILL]), "{certify:?}");
    }
}

/// A lock that leaves out a field below another makes read-only the fields it covers, not their
/// parent, which the field left out would inherit it from.
#[test]
fn a_lock_leaving_out_a_field_does_not_lock_it_through_its_parent() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let fixture = locked_parts_fixture("/Lock << /Type /SigFieldLock /Action /Exclude /Fields [(Party.Notes)] >>");
    let signed = pdfcraft_sign::sign(&open(&fixture), &id, &SignOptions { field: Some("Approval".into()), ..opts() }).unwrap();
    let doc = open(&signed);
    assert_eq!((ff(&doc, 10), ff(&doc, 8), ff(&doc, 7), ff(&doc, 9)), (None, None, Some(1), Some(1)));
}

/// A field an earlier signature locked is not signed, though a later revision cleared its
/// read-only flag; nor is one whose flags it inherits from further up than PdfCraft reads.
#[test]
fn a_field_an_earlier_signature_locked_is_not_signed_once_unflagged() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let sign_field = |bytes: &[u8], field: &str| pdfcraft_sign::sign(&open(bytes), &id, &SignOptions { field: Some(field.into()), ..opts() });
    let locks_seller = sign_field(&locking_fixture(&lock_of("(Seller)"), ""), "Approval").unwrap();
    let unflagged =
        edit_after(&locks_seller, |doc| doc.update_dict(ObjRef { num: 9, generation: 0 }, |d| d.set(b"Ff".to_vec(), Object::Int(0))).unwrap());
    assert!(matches!(sign_field(&unflagged, "Seller"), Err(SignError::Pdf(m)) if m.contains("locked by the signature in \"Approval\"")));
    // Approval (6) under 33 fields, the top one read-only.
    let mut objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [7 0 R] >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>".to_vec(),
        b"<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Parent 39 0 R /Rect [150 20 280 70] /P 3 0 R /F 4 >>".to_vec(),
    ];
    for num in 7..=39 {
        let parent = if num == 7 { "/Ff 1".to_string() } else { format!("/Parent {} 0 R", num - 1) };
        let kid = if num == 39 { 6 } else { num + 1 };
        objs.push(format!("<< /T (P{num}) {parent} /Kids [{kid} 0 R] >>").into_bytes());
    }
    let deep = write_pdf(&objs.iter().map(Vec::as_slice).collect::<Vec<_>>());
    let name = (7..=39).map(|n| format!("P{n}")).chain(["Approval".to_string()]).collect::<Vec<_>>().join(".");
    assert!(matches!(sign_field(&deep, &name), Err(SignError::Pdf(m)) if m.contains("too far up")));
}

/// A field that requires a reason but lists none is signed only without one; a reason is matched
/// as it is written, padding and all; and a list that is not one refuses the signing.
#[test]
fn a_required_reason_is_matched_as_written_and_never_from_a_list_that_is_not_there() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let sign_for = |sv: &str, reason: Option<&str>| {
        let doc = open(&locking_fixture(sv, ""));
        pdfcraft_sign::sign(&doc, &id, &SignOptions { field: Some("Approval".into()), reason: reason.map(Into::into), ..opts() })
    };
    let no_list = "/SV << /Type /SV /Ff 8 >>";
    assert!(matches!(sign_for(no_list, Some("Witnessed")), Err(SignError::Pdf(m)) if m.contains("without a reason")));
    assert!(sign_for(no_list, None).is_ok());
    assert!(sign_for("/SV << /Type /SV /Reasons [(Witnessed)] /Ff 8 >>", Some(" Witnessed ")).is_err());
    assert!(matches!(sign_for("/SV << /Type /SV /Reasons 5 /Ff 8 >>", Some("Witnessed")), Err(SignError::Pdf(m)) if m.contains("can't read")));
}
