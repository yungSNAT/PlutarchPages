//! Field locks (FieldMDP, ISO 32000-2 §12.8.2.4): a signature field's `/Lock` names the fields
//! that signing it makes read-only, the signature records the same in a FieldMDP reference, and a
//! later change to one of them is not permitted whatever the document's DocMDP permissions allow.

use super::listing::{Field, walk_fields};
use super::{Dict, Document, HashSet, ObjRef, Object, PdfString, SignError};

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

/// `d`'s field flags, its own `/Ff` or the nearest parent's.
fn flags(doc: &Document, d: &Dict) -> i64 {
    let mut d = d.clone();
    for _ in 0..MAX_PARENTS {
        if let Some(ff) = d.get(b"Ff").and_then(|f| doc.resolve(f).as_int()) {
            return ff;
        }
        let Some(parent) = d.get(b"Parent").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()) else { break };
        d = parent;
    }
    0
}

/// Refuse to sign `field` when it is read-only (an earlier signature's lock, or its author, made it
/// so), or when its `/SV` requires one of its `/Reasons` and `reason` isn't one of them. A single
/// reason of `.` means none may be given.
pub(super) fn check_signable(doc: &Document, field: &Field, reason: Option<&str>) -> Result<(), SignError> {
    if flags(doc, &field.dict) & READ_ONLY != 0 {
        return Err(SignError::Pdf(format!("the field {:?} is read-only, locked by an earlier signature or by the form's author", field.name)));
    }
    let Some(sv) = field.dict.get(b"SV").map(|sv| doc.resolve(sv)).and_then(|sv| sv.as_dict().cloned()) else { return Ok(()) };
    if sv.get(b"Ff").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0) & REASONS_REQUIRED == 0 {
        return Ok(());
    }
    let reasons: Vec<String> = sv
        .get(b"Reasons")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_array().map(|a| a.iter().filter_map(|r| doc.resolve(r).as_string().map(|s| s.to_text())).collect()))
        .unwrap_or_default();
    let reason = reason.map(str::trim).filter(|r| !r.is_empty());
    let ok = match (reasons.as_slice(), reason) {
        ([], _) => true,
        ([only], given) if only == "." => given.is_none(),
        (_, Some(given)) => reasons.iter().any(|r| r == given),
        (_, None) => false,
    };
    if ok {
        Ok(())
    } else {
        Err(SignError::Pdf(format!("the field {:?} must be signed for one of these reasons: {}", field.name, reasons.join("; "))))
    }
}

/// Make every field `lock` covers read-only. One that already is stays as it was, so a signature
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
    for f in fields.iter().filter(|f| lock.covers(&f.name)) {
        let ff = flags(doc, &f.dict);
        if ff & READ_ONLY != 0 || f.dict.get(b"V").is_some_and(|v| doc.resolve(v).as_dict().is_some()) {
            continue;
        }
        let r = f.r.ok_or_else(|| SignError::Pdf(format!("the field {:?} can't be locked: it is not an object of its own", f.name)))?;
        doc.update_dict(r, |d| d.set(b"Ff".to_vec(), Object::Int(ff | READ_ONLY)))?;
    }
    Ok(())
}

/// What a signature's FieldMDP references lock ([`locked`]).
pub(super) struct Locked {
    /// The objects of the fields they cover: each field, its widgets and their appearance
    /// dictionaries and streams.
    pub(super) objects: HashSet<u32>,
    /// Whether the form was read whole: if not, a field not read may be one of them.
    pub(super) whole: bool,
}

/// What signature value `v`'s FieldMDP references lock in `old`, the revision it signed, or `None`
/// when it has none.
pub(super) fn locked(old: &Document, v: &Dict) -> Option<Locked> {
    let references = v.get(b"Reference").map(|r| old.resolve(r))?;
    let mut locks = references
        .as_array()?
        .iter()
        .filter_map(|r| old.resolve(r).as_dict().cloned())
        .filter(|r| r.name(b"TransformMethod") == Some(b"FieldMDP"))
        // A FieldMDP reference whose parameters can't be read locks everything.
        .map(|r| r.get(b"TransformParams").and_then(|p| old.resolve(p).as_dict().and_then(|p| FieldLock::read(old, p))).unwrap_or(FieldLock::All))
        .peekable();
    locks.peek()?;
    let locks: Vec<FieldLock> = locks.collect();
    let (fields, whole) = walk_fields(old, false);
    let mut objects = HashSet::new();
    for f in fields.iter().filter(|f| locks.iter().any(|l| l.covers(&f.name))) {
        objects.extend(f.r.map(|r| r.num));
        for (w, d) in &f.widgets {
            objects.extend(w.map(|r| r.num));
            appearance_objects(old, d, &mut objects);
        }
        appearance_objects(old, &f.dict, &mut objects);
    }
    Some(Locked { objects, whole })
}

/// The objects of `widget`'s `/AP`: the dictionary, each appearance and each state's appearance.
fn appearance_objects(doc: &Document, widget: &Dict, out: &mut HashSet<u32>) {
    let Some(ap) = widget.get(b"AP") else { return };
    out.extend(ap.as_ref().map(|r| r.num));
    let ap = doc.resolve(ap);
    for (_, entry) in ap.as_dict().into_iter().flat_map(Dict::iter) {
        out.extend(entry.as_ref().map(|r| r.num));
        if let Object::Dict(states) = &*doc.resolve(entry) {
            out.extend(states.iter().filter_map(|(_, s)| s.as_ref()).map(|r| r.num));
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
