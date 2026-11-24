// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

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
    let field = super::super::listing::sig_fields(&old).into_iter().find(|f| f.name == "Approval").unwrap();
    let v = old.get(r(8, 0));
    let filled = TEXT_FIELD.replace("(Ann)", "(Bob)");
    let now = signed.clone().revision(&[(5, Row::Obj(0, 0, &filled))]);
    assert_eq!(super::super::locks::locked_fields(&now.open(), &old, &field, v.as_dict().unwrap()), super::super::locks::Locked::Changed);
}
