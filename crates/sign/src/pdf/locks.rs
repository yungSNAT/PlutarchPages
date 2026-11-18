//! Field locks (FieldMDP, ISO 32000-2 §12.8.2.4): a signature field's `/Lock` names the fields
//! that signing it makes read-only, the signature records the same in a FieldMDP reference, and a
//! later change to one of them is not permitted whatever the document's DocMDP permissions allow.

use super::changes::classify_changes;
use super::listing::{Field, sig_fields, walk_fields};
use super::types::{Modification, kind};
use super::{Arc, Dict, Document, HashSet, ObjRef, Object, PdfString, SignError};

/// The read-only field flag (`/Ff` bit 1).
const READ_ONLY: i64 = 1;
/// The `/SV` flag that makes its `/Reasons` binding (`/Ff` bit 4).
const REASONS_REQUIRED: i64 = 1 << 3;
/// How far up `/Parent` an inherited `/Ff` is looked for.
const MAX_PARENTS: usize = 32;
/// Most objects one signature's locked fields may lead to; past it the lock reads as not whole.
const MAX_LOCKED_OBJECTS: usize = 1 << 16;

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

/// What a signature's locks cover ([`locked`]).
pub(super) struct Locked {
    /// Every object the covered fields are made of or lead to: each field, its parents, its
    /// widgets, and what their entries refer to (values, appearance streams and their resources),
    /// short of the pages and the rest of the form.
    pub(super) objects: HashSet<u32>,
    /// Each covered field's name, object and widgets' objects, in the order the form lists them.
    pub(super) fields: Vec<(String, u32, Vec<u32>)>,
    /// Whether the form was read whole: if not, a field not read may be one of them.
    pub(super) whole: bool,
}

impl Locked {
    /// Whether `doc` still has the covered fields the signed revision had, by name, as the same
    /// objects: a field replaced or moved under another parent leaves its old objects as they were.
    pub(super) fn same_fields_in(&self, doc: &Document) -> bool {
        let names: HashSet<&str> = self.fields.iter().map(|(n, ..)| n.as_str()).collect();
        let (fields, whole) = walk_fields(doc, false);
        let now: Option<Vec<_>> = fields.iter().filter(|f| names.contains(f.name.as_str())).map(placed).collect();
        (whole || !self.whole) && now.as_ref() == Some(&self.fields)
    }
}

/// [`classify_changes`] of `doc` against `old` under `locked`; a covered field replaced, or moved
/// under another parent, which leaves its old objects as they were, is a change to a locked field.
pub(super) fn classify_locked(doc: &Document, old: Document, p: Option<u8>, locked: Option<&Locked>) -> Modification {
    let judged = classify_changes(doc, Some(old), p, locked);
    if !locked.is_some_and(|l| !l.same_fields_in(doc)) {
        return judged;
    }
    let mut kinds = match judged {
        Modification::Disallowed(kinds) => kinds,
        _ => Vec::new(),
    };
    if !kinds.contains(&kind::LOCKED_FORM_FIELDS) {
        kinds.push(kind::LOCKED_FORM_FIELDS);
    }
    Modification::Disallowed(kinds)
}

/// How a change to object `num` reads under `locked`: as a change to a locked field when `num` is
/// one of its objects, before `classify` can pass over it as unchanged, or when it is a form fill
/// and the form was not read whole; otherwise as `classify` reads it (`None`: no change).
pub(super) fn under_lock(locked: Option<&Locked>, num: u32, classify: impl FnOnce() -> Option<&'static str>) -> Option<&'static str> {
    match locked {
        Some(l) if l.objects.contains(&num) => Some(kind::LOCKED_FORM_FIELDS),
        Some(l) if !l.whole => classify().map(|what| if what == kind::FORM_FILL { kind::LOCKED_FORM_FIELDS } else { what }),
        _ => classify(),
    }
}

/// `f`'s name, object and widgets' objects; `None` when one of them is not an object of its own.
fn placed(f: &Field) -> Option<(String, u32, Vec<u32>)> {
    let widgets = f.widgets.iter().map(|(w, _)| w.map(|w| w.num)).collect::<Option<Vec<u32>>>()?;
    Some((f.name.clone(), f.r?.num, widgets))
}

/// What the signature in signature field `field`, with value `v`, locks in `old`, the revision it
/// signed ([`policies`]), or `None` when it locks nothing.
pub(super) fn locked(old: &Document, field: &Dict, v: &Dict) -> Option<Locked> {
    let locks = policies(old, field, v);
    if locks.is_empty() {
        return None;
    }
    let (fields, mut whole) = walk_fields(old, false);
    let (mut objects, mut covered, mut left) = (HashSet::new(), Vec::new(), MAX_LOCKED_OBJECTS);
    for f in fields.iter().filter(|f| locks.iter().any(|l| l.covers(&f.name))) {
        // A field or widget written in place changes with whatever holds it, which may be no
        // field of these.
        let Some(place) = placed(f) else {
            whole = false;
            continue;
        };
        objects.insert(place.1);
        objects.extend(place.2.iter().copied());
        covered.push(place);
        whole &= reach(old, &f.dict, &mut objects, &mut left);
        for (_, w) in &f.widgets {
            whole &= reach(old, w, &mut objects, &mut left);
        }
        whole &= parents(old, &f.dict, &mut objects, &mut left);
    }
    Some(Locked { objects, fields: covered, whole })
}

/// Add `d`'s parents, and what their entries lead to ([`reach`]), to `out`; false when they go on
/// past [`MAX_PARENTS`] or [`reach`] stops.
fn parents(doc: &Document, d: &Dict, out: &mut HashSet<u32>, left: &mut usize) -> bool {
    let mut next = d.get(b"Parent").cloned();
    for _ in 0..MAX_PARENTS {
        let Some(p) = next.take() else { return true };
        if let Some(r) = p.as_ref() {
            out.insert(r.num);
        }
        let parent = doc.resolve(&p);
        let Some(parent) = parent.as_dict() else { return true };
        if !reach(doc, parent, out, left) {
            return false;
        }
        next = parent.get(b"Parent").cloned();
    }
    next.is_none()
}

/// Add to `out` every object `d`'s entries lead to, other than up or across the form and page
/// trees: `/Parent`, `/Kids` and `/P` are not followed, nor is a page, the page tree or the
/// catalog. False once that would add more than `left` objects.
fn reach(doc: &Document, d: &Dict, out: &mut HashSet<u32>, left: &mut usize) -> bool {
    let mut found = Vec::new();
    refs_in_dict(d, &mut found);
    while let Some(r) = found.pop() {
        if out.contains(&r.num) {
            continue;
        }
        let o = doc.get(r);
        let dict = match &*o {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        };
        if dict.is_some_and(|d| matches!(d.name(b"Type"), Some(b"Page" | b"Pages" | b"Catalog"))) {
            continue;
        }
        let Some(rest) = left.checked_sub(1) else { return false };
        *left = rest;
        out.insert(r.num);
        refs_in(&o, &mut found);
    }
    true
}

/// The references in `o`, written in place or nested, other than through `/Parent`, `/Kids` or `/P`.
fn refs_in(o: &Object, found: &mut Vec<ObjRef>) {
    match o {
        Object::Ref(r) => found.push(*r),
        Object::Array(a) => a.iter().for_each(|o| refs_in(o, found)),
        Object::Dict(d) => refs_in_dict(d, found),
        Object::Stream(s) => refs_in_dict(&s.dict, found),
        _ => {}
    }
}

fn refs_in_dict(d: &Dict, found: &mut Vec<ObjRef>) {
    for (k, v) in d.iter() {
        if !matches!(k.as_slice(), b"Parent" | b"Kids" | b"P") {
            refs_in(v, found);
        }
    }
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
