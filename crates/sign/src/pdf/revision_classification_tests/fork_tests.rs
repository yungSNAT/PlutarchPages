// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// A field a signature locked (FieldMDP) may not change, though its DocMDP permissions or an
/// approval signature allow form fill; when the form could not be read whole, any form fill may
/// have changed a locked field, and is refused as one.
#[test]
fn a_change_to_a_locked_field_is_refused_whatever_else_is_permitted() {
    let signed = form("/Annots [5 0 R]");
    let filled = TEXT_FIELD.replace("(Ann)", "(Bob)");
    let now = signed.clone().revision(&[(5, Row::Obj(0, 0, &filled))]);
    let locked = |objects: &[u32], whole| super::locks::Locked { objects: objects.iter().copied().collect(), fields: Vec::new(), whole };
    for p in [None, Some(2), Some(3)] {
        let judge = |l: &super::locks::Locked| classify_changes(&now.open(), Some(signed.open()), p, Some(l));
        assert_eq!(judge(&locked(&[5], true)), Modification::Disallowed(vec![super::kind::LOCKED_FORM_FIELDS]), "P={p:?}");
        assert_eq!(judge(&locked(&[], false)), Modification::Disallowed(vec![super::kind::LOCKED_FORM_FIELDS]), "P={p:?}, not read whole");
        assert_eq!(judge(&locked(&[], true)), Modification::Allowed(vec![super::kind::FORM_FILL]), "P={p:?}, another field locked");
    }
}

/// A signed field's own `/Lock` holds whatever its signature's FieldMDP reference says: a reference
/// naming other fields does not narrow a field that locks them all.
#[test]
fn a_reference_narrower_than_the_signed_fields_lock_does_not_narrow_it() {
    let sig = "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /Lock << /Action /All >> /V 8 0 R >>";
    let v = "<< /Type /Sig /Reference [<< /Type /SigRef /TransformMethod /FieldMDP /TransformParams << /Action /Include /Fields [(Other)] >> >>] >>";
    let page3 = page("/Annots [5 0 R 7 0 R]");
    let signed = File::new().revision(&[
        (1, Row::Obj(0, 0, "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 7 0 R] >> >>")),
        (2, Row::Obj(0, 0, PAGES)),
        (3, Row::Obj(0, 0, &page3)),
        (4, Row::Obj(0, 0, &page("/Annots []"))),
        (5, Row::Obj(0, 0, TEXT_FIELD)),
        (7, Row::Obj(0, 0, sig)),
        (8, Row::Obj(0, 0, v)),
    ]);
    let old = signed.open();
    let (field, v) = (old.get(r(7, 0)), old.get(r(8, 0)));
    let locked = super::locks::locked(&old, field.as_dict().unwrap(), v.as_dict().unwrap()).unwrap();
    let filled = TEXT_FIELD.replace("(Ann)", "(Bob)");
    let now = signed.clone().revision(&[(5, Row::Obj(0, 0, &filled))]);
    assert_eq!(classify_changes(&now.open(), Some(old), None, Some(&locked)), Modification::Disallowed(vec![super::kind::LOCKED_FORM_FIELDS]));
}
