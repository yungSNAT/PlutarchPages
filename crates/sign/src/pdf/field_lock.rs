// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Signing a field that locks others (FieldMDP, ISO 32000-2 §12.8.2.4): its `/Lock` names the
//! fields that signing it makes read-only, and the signature records the same in a FieldMDP
//! reference. A field an earlier signature locks, or a read-only one, is not signed, nor one whose
//! `/SV` requires a reason not given. Validating a lock is upstream's ([`super::locks`]).

use super::listing::{Field, sig_fields, walk_fields};
use super::{Arc, Dict, Document, ObjRef, Object, PdfString, SignError};

/// The read-only field flag (`/Ff` bit 1).
const READ_ONLY: i64 = 1;
/// The `/SV` flag that makes its `/Reasons` binding (`/Ff` bit 4).
const REASONS_REQUIRED: i64 = 1 << 3;
/// How far up `/Parent` an inherited `/Ff` is looked for.
const MAX_PARENTS: usize = 32;

/// The fields a lock covers, by full name: a name also covers the fields below it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FieldLock {
    All,
    Include(Vec<String>),
    Exclude(Vec<String>),
}

impl FieldLock {
    /// The lock `d` gives, a `/Lock` or FieldMDP `/TransformParams` dictionary, or `None` when it
    /// can't be read as one.
    fn read(doc: &Document, d: &Dict) -> Option<Self> {
        let names = || -> Option<Vec<String>> {
            let fields = doc.resolve(d.get(b"Fields")?);
            fields.as_array()?.iter().map(|n| doc.resolve(n).as_string().map(|s| s.to_text())).collect()
        };
        match d.get(b"Action").map(|a| doc.resolve(a)).as_deref().and_then(Object::as_name)? {
            b"All" => Some(Self::All),
            b"Include" => Some(Self::Include(names()?)),
            b"Exclude" => Some(Self::Exclude(names()?)),
            _ => None,
        }
    }

    /// The lock `d` gives, or [`FieldLock::All`] when it can't be read as one.
    fn read_or_all(doc: &Document, d: Option<Arc<Object>>) -> Self {
        d.and_then(|d| d.as_dict().and_then(|d| Self::read(doc, d))).unwrap_or(Self::All)
    }

    fn covers(&self, name: &str) -> bool {
        let named = |names: &[String]| names.iter().any(|n| name == n || name.strip_prefix(n.as_str()).is_some_and(|rest| rest.starts_with('.')));
        match self {
            Self::All => true,
            Self::Include(names) => named(names),
            Self::Exclude(names) => !named(names),
        }
    }

    fn params(&self) -> Dict {
        let (action, names) = match self {
            Self::All => ("All", None),
            Self::Include(names) => ("Include", Some(names)),
            Self::Exclude(names) => ("Exclude", Some(names)),
        };
        let mut p = Dict::new();
        p.set(b"Type".to_vec(), Object::name("TransformParams"));
        p.set(b"Action".to_vec(), Object::name(action));
        if let Some(names) = names {
            p.set(b"Fields".to_vec(), Object::Array(names.iter().map(|n| Object::String(PdfString::text(n))).collect()));
        }
        p.set(b"V".to_vec(), Object::name("1.2"));
        p
    }
}

/// The lock of signature field `field`, from its `/Lock`: `None` without one, an error when it is
/// there but can't be read, as signing would then lock less than the author asked.
pub(super) fn field_lock(doc: &Document, field: &Field) -> Result<Option<FieldLock>, SignError> {
    let Some(lock) = field.dict.get(b"Lock").map(|l| doc.resolve(l)) else { return Ok(None) };
    lock.as_dict()
        .and_then(|l| FieldLock::read(doc, l))
        .map(Some)
        .ok_or_else(|| SignError::Pdf(format!("the field {:?} locks fields in a way PdfCraft can't read", field.name)))
}

/// A FieldMDP reference recording `lock` for a signature (ISO 32000-2 Table 255); `/Data` is the
/// catalog, as PDF 1.x readers expect.
pub(super) fn reference(lock: &FieldLock, root: ObjRef) -> Object {
    let mut r = Dict::new();
    r.set(b"Type".to_vec(), Object::name("SigRef"));
    r.set(b"TransformMethod".to_vec(), Object::name("FieldMDP"));
    r.set(b"TransformParams".to_vec(), Object::Dict(lock.params()));
    r.set(b"Data".to_vec(), Object::Ref(root));
    Object::Dict(r)
}

/// The locks signature value `v` in signature field `field` holds to: its FieldMDP references, and
/// the field's own `/Lock`, which a reference that disagrees with it can't narrow. One that can't be
/// read locks everything.
fn policies(doc: &Document, field: &Dict, v: &Dict) -> Vec<FieldLock> {
    let mut locks = Vec::new();
    let references = v.get(b"Reference").map(|r| doc.resolve(r));
    for r in references.as_deref().and_then(Object::as_array).into_iter().flatten() {
        let r = doc.resolve(r);
        if let Some(r) = r.as_dict().filter(|r| r.name(b"TransformMethod") == Some(b"FieldMDP")) {
            locks.push(FieldLock::read_or_all(doc, r.get(b"TransformParams").map(|p| doc.resolve(p))));
        }
    }
    if let Some(lock) = field.get(b"Lock") {
        locks.push(FieldLock::read_or_all(doc, Some(doc.resolve(lock))));
    }
    locks
}

/// `d`'s field flags, its own `/Ff` or the nearest parent's; `None` when the parents go on past
/// [`MAX_PARENTS`], as the flags are then not known.
fn flags(doc: &Document, d: &Dict) -> Option<i64> {
    let own = |d: &Dict| d.get(b"Ff").and_then(|f| doc.resolve(f).as_int());
    if let Some(ff) = own(d) {
        return Some(ff);
    }
    let mut next = d.get(b"Parent").map(|p| doc.resolve(p));
    for _ in 0..MAX_PARENTS {
        let parent = next.take();
        let Some(parent) = parent.as_deref().and_then(Object::as_dict) else { return Some(0) };
        if let Some(ff) = own(parent) {
            return Some(ff);
        }
        next = parent.get(b"Parent").map(|p| doc.resolve(p));
    }
    next.is_none().then_some(0)
}

/// Refuse to sign `field` when it is read-only (an earlier signature's lock, or its author, made it
/// so), when an earlier signature's lock covers it though it is not read-only, or when its `/SV`
/// requires one of its `/Reasons` and `reason`, as signing writes it, isn't one of them. Without
/// `/Reasons`, or with a single reason of `.`, none may be given.
pub(super) fn check_signable(doc: &Document, field: &Field, reason: Option<&str>) -> Result<(), SignError> {
    match flags(doc, &field.dict) {
        None => return Err(SignError::Pdf(format!("the field {:?} inherits its flags from too far up for PdfCraft to read", field.name))),
        Some(ff) if ff & READ_ONLY != 0 => {
            return Err(SignError::Pdf(format!("the field {:?} is read-only, locked by an earlier signature or by the form's author", field.name)));
        }
        Some(_) => {}
    }
    for signed in sig_fields(doc) {
        let Some(v) = signed.signature(doc) else { continue };
        if policies(doc, &signed.dict, &v).iter().any(|l| l.covers(&field.name)) {
            return Err(SignError::Pdf(format!("the field {:?} is read-only, locked by the signature in {:?}", field.name, signed.name)));
        }
    }
    let Some(sv) = field.dict.get(b"SV").map(|sv| doc.resolve(sv)) else { return Ok(()) };
    let Some(sv) = sv.as_dict() else { return Ok(()) };
    if sv.get(b"Ff").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0) & REASONS_REQUIRED == 0 {
        return Ok(());
    }
    let reasons: Option<Vec<String>> = match sv.get(b"Reasons").map(|r| doc.resolve(r)) {
        None => Some(Vec::new()),
        Some(r) if matches!(*r, Object::Null) => Some(Vec::new()),
        Some(r) => r.as_array().and_then(|a| a.iter().map(|r| doc.resolve(r).as_string().map(|s| s.to_text())).collect()),
    };
    let Some(reasons) = reasons else {
        return Err(SignError::Pdf(format!("the field {:?} requires a reason from a list PdfCraft can't read", field.name)));
    };
    let given = reason.filter(|r| !r.trim().is_empty());
    let ok = match (reasons.as_slice(), given) {
        ([], given) => given.is_none(),
        ([only], given) if only == "." => given.is_none(),
        (_, Some(given)) => reasons.iter().any(|r| r == given),
        (_, None) => false,
    };
    if ok {
        Ok(())
    } else if reasons.iter().all(|r| r == ".") {
        Err(SignError::Pdf(format!("the field {:?} must be signed without a reason", field.name)))
    } else {
        Err(SignError::Pdf(format!("the field {:?} must be signed for one of these reasons: {}", field.name, reasons.join("; "))))
    }
}

/// Make every field `lock` covers read-only, by its own `/Ff`. A field with fields below it is
/// left as it is, as they would inherit its flag whether `lock` covers them or not; those it
/// covers are made read-only themselves. One that already is stays as it was, so a signature
/// whose lock repeats an earlier one changes nothing that one locked, and so does a signature field
/// that already holds a signature: its signature protects it, and a later change to a signed
/// widget reads as a change to the page.
pub(super) fn lock_fields(doc: &mut Document, lock: &FieldLock) -> Result<(), SignError> {
    let (fields, whole) = walk_fields(doc, false);
    if !whole {
        return Err(SignError::Pdf(
            "this form has too many fields, or a field name too long, for PdfCraft to lock the ones this signature locks".into(),
        ));
    }
    let mut names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    names.sort_unstable();
    let has_fields_below = |name: &str| {
        let below = format!("{name}.");
        names.get(names.partition_point(|n| *n < below.as_str())).is_some_and(|n| n.starts_with(&below))
    };
    for f in fields.iter().filter(|f| lock.covers(&f.name) && !has_fields_below(&f.name)) {
        let ff =
            flags(doc, &f.dict).ok_or_else(|| SignError::Pdf(format!("the field {:?} inherits its flags from too far up to be locked", f.name)))?;
        if ff & READ_ONLY != 0 || f.dict.get(b"V").is_some_and(|v| doc.resolve(v).as_dict().is_some()) {
            continue;
        }
        let r = f.r.ok_or_else(|| SignError::Pdf(format!("the field {:?} can't be locked: it is not an object of its own", f.name)))?;
        doc.update_dict(r, |d| d.set(b"Ff".to_vec(), Object::Int(ff | READ_ONLY)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::FieldLock;

    /// A lock's name covers that field and the fields below it, not another whose name starts
    /// the same way.
    #[test]
    fn a_name_covers_its_field_and_the_fields_below_it() {
        let lock = FieldLock::Include(vec!["Buyer".into()]);
        assert!(lock.covers("Buyer") && lock.covers("Buyer.Name"));
        assert!(!lock.covers("BuyerX") && !lock.covers("Seller.Buyer"));
        let except = FieldLock::Exclude(vec!["Buyer".into()]);
        assert!(!except.covers("Buyer.Name") && except.covers("Notes"));
        assert!(FieldLock::All.covers("Notes"));
    }
}
