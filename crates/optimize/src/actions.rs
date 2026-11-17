// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Discard external cross-references outside links and bookmarks: actions that open another file
//! or launch a program, wherever they are (ISO 32000-2 §12.6.4). Upstream's `external_refs` takes
//! them out of links and bookmarks first; form actions are upstream's `form_actions`.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::OptimizeError;

/// How deep `/Next` chains are followed.
const MAX_CHAIN: usize = 32;

/// External cross-references: go to another file, or launch an application or file.
pub(crate) const EXTERNAL_ACTIONS: &[&[u8]] = &[b"GoToR", b"Launch"];

fn kind_of(doc: &Document, action: &Object) -> Option<Vec<u8>> {
    doc.resolve(action).as_dict().and_then(|d| d.name(b"S")).map(<[u8]>::to_vec)
}

fn is_target(doc: &Document, action: &Object, kinds: &[&[u8]]) -> bool {
    kind_of(doc, action).is_some_and(|k| kinds.contains(&k.as_slice()))
}

/// An action value with the targeted actions taken out: `None` when the action itself goes; an
/// inline action keeps its `/Next` chain minus the targets (followed `MAX_CHAIN` deep). A
/// referenced action is left for the sweep to clean where it is stored.
fn clean_action(doc: &Document, action: &Object, kinds: &[&[u8]], removed: &mut usize, depth: usize) -> Option<Object> {
    if is_target(doc, action, kinds) {
        *removed += 1;
        return None;
    }
    match action {
        Object::Dict(d) if depth < MAX_CHAIN => {
            let mut d = d.clone();
            if let Some(next) = d.get(b"Next").cloned() {
                match clean_next(doc, &next, kinds, removed, depth + 1) {
                    Some(n) => d.set(b"Next".to_vec(), n),
                    None => {
                        d.remove(b"Next");
                    }
                }
            }
            Some(Object::Dict(d))
        }
        other => Some(other.clone()),
    }
}

/// A `/Next` value (one action or an array of them) without the targets; `None` when nothing is left.
fn clean_next(doc: &Document, next: &Object, kinds: &[&[u8]], removed: &mut usize, depth: usize) -> Option<Object> {
    match next {
        Object::Array(items) => {
            let kept: Vec<Object> = items.iter().filter_map(|a| clean_action(doc, a, kinds, removed, depth)).collect();
            (!kept.is_empty()).then_some(Object::Array(kept))
        }
        other => clean_action(doc, other, kinds, removed, depth),
    }
}

/// Clean one dictionary in place: its `/A`, `/OpenAction`, `/AA` entries and, if it is itself an
/// action, its `/Next`. Returns whether it changed.
fn clean_dict(doc: &Document, d: &mut Dict, kinds: &[&[u8]], removed: &mut usize) -> bool {
    let before = *removed;
    for key in [&b"A"[..], b"OpenAction"] {
        // An OpenAction can also be a destination array, which is not an action.
        if let Some(a) = d.get(key).cloned().filter(|a| doc.resolve(a).as_dict().is_some()) {
            match clean_action(doc, &a, kinds, removed, 0) {
                Some(kept) => d.set(key.to_vec(), kept),
                None => {
                    d.remove(key);
                }
            }
        }
    }
    if let Some(Object::Dict(aa)) = d.get(b"AA").cloned() {
        let mut out = Dict::new();
        for (k, a) in aa.iter() {
            if let Some(kept) = clean_action(doc, a, kinds, removed, 0) {
                out.set(k.to_vec(), kept);
            }
        }
        if out.is_empty() {
            d.remove(b"AA");
        } else {
            d.set(b"AA".to_vec(), Object::Dict(out));
        }
    }
    if d.name(b"Type").is_none_or(|t| t == b"Action")
        && d.contains(b"S")
        && let Some(next) = d.get(b"Next").cloned()
    {
        match clean_next(doc, &next, kinds, removed, 1) {
            Some(n) => d.set(b"Next".to_vec(), n),
            None => {
                d.remove(b"Next");
            }
        }
    }
    *removed != before
}

/// Remove every action of `kinds` from the document (annotations, form fields, pages, the
/// catalog's open action and triggers, bookmarks, and chained `/Next` actions). Returns how
/// many were removed.
pub(crate) fn discard(doc: &mut Document, kinds: &[&[u8]]) -> Result<usize, OptimizeError> {
    let mut removed = 0;
    let mut seen = HashSet::new();
    for num in doc.object_numbers() {
        let r = ObjRef { num, generation: doc.generation(num) };
        if !seen.insert(r) {
            continue;
        }
        let Some(mut d) = doc.get(r).as_dict().cloned() else { continue };
        let mut n = 0;
        if clean_dict(doc, &mut d, kinds, &mut n) {
            removed += n;
            doc.update_dict(r, |target| *target = d)?;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::PdfString;

    /// Form actions: send, load or clear field values. The sweep takes any list of kinds.
    const FORM_ACTIONS: &[&[u8]] = &[b"SubmitForm", b"ImportData", b"ResetForm"];

    fn action(kind: &str) -> Dict {
        let mut a = Dict::new();
        a.set(b"S".to_vec(), Object::name(kind));
        a
    }

    #[test]
    fn form_and_external_actions_go_and_others_stay() {
        let mut doc = Document::new_empty();
        let mut uri = action("URI");
        uri.set(b"URI".to_vec(), PdfString::literal(b"https://example.org".to_vec()));
        let mut gotor = action("GoToR");
        gotor.set(b"F".to_vec(), PdfString::literal(b"other.pdf".to_vec()));
        // A submit button with a reset chained after a JavaScript action.
        let mut js = action("JavaScript");
        js.set(b"Next".to_vec(), Object::Array(vec![Object::Dict(action("ResetForm")), Object::Dict(action("URI"))]));
        let mut button = Dict::new();
        button.set(b"Subtype".to_vec(), Object::name("Widget"));
        button.set(b"A".to_vec(), Object::Dict(action("SubmitForm")));
        let mut aa = Dict::new();
        aa.set(b"D".to_vec(), Object::Dict(js));
        aa.set(b"U".to_vec(), Object::Dict(action("ImportData")));
        button.set(b"AA".to_vec(), Object::Dict(aa));
        let button = doc.add(button);
        let mut link = Dict::new();
        link.set(b"Subtype".to_vec(), Object::name("Link"));
        link.set(b"A".to_vec(), Object::Ref(doc.add(gotor)));
        let link = doc.add(link);
        let mut web = Dict::new();
        web.set(b"A".to_vec(), Object::Dict(uri));
        let web = doc.add(web);

        assert_eq!(discard(&mut doc, FORM_ACTIONS).unwrap(), 3, "submit, import and the chained reset");
        let b = doc.get(button).as_dict().cloned().unwrap();
        assert!(!b.contains(b"A"));
        let aa = b.get(b"AA").and_then(Object::as_dict).cloned().unwrap();
        assert!(!aa.contains(b"U"));
        let next = aa.get(b"D").and_then(Object::as_dict).and_then(|d| d.get(b"Next")).and_then(Object::as_array).cloned().unwrap();
        assert_eq!(next.len(), 1, "the URI after the reset stays");
        assert!(doc.get(link).as_dict().unwrap().contains(b"A"), "links stay for form actions");

        assert_eq!(discard(&mut doc, EXTERNAL_ACTIONS).unwrap(), 1);
        assert!(!doc.get(link).as_dict().unwrap().contains(b"A"), "the link to another file goes");
        assert!(doc.get(web).as_dict().unwrap().contains(b"A"), "web links are not cross-references");
    }

    #[test]
    fn long_and_deep_chains_are_bounded() {
        let mut doc = Document::new_empty();
        // A JavaScript action whose /Next chain nests 200 deep, each level resetting the form.
        let mut chain = action("JavaScript");
        for _ in 0..200 {
            let mut outer = action("JavaScript");
            outer.set(b"Next".to_vec(), Object::Array(vec![Object::Dict(action("ResetForm")), Object::Dict(chain)]));
            chain = outer;
        }
        let mut holder = Dict::new();
        holder.set(b"OpenAction".to_vec(), Object::Dict(chain));
        holder.set(b"AA".to_vec(), Object::Int(7));
        doc.add(holder);
        let n = discard(&mut doc, FORM_ACTIONS).unwrap();
        assert!(n > 0 && n <= MAX_CHAIN + 1, "{n}");
        // A wide chain is cleaned entirely (no recursion).
        let mut wide = action("JavaScript");
        wide.set(b"Next".to_vec(), Object::Array((0..10_000).map(|_| Object::Dict(action("ResetForm"))).collect()));
        let mut holder = Dict::new();
        holder.set(b"OpenAction".to_vec(), Object::Dict(wide));
        let r = doc.add(holder);
        assert_eq!(discard(&mut doc, FORM_ACTIONS).unwrap(), 10_000);
        assert!(!doc.get(r).as_dict().and_then(|d| d.get(b"OpenAction")).and_then(Object::as_dict).unwrap().contains(b"Next"));
    }
}
