// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

#[test]
fn gray_conversion_visits_each_unchanged_form_once() {
    // Twenty nested forms, each drawing the next eight times, with an already-gray leaf: re-walking
    // unchanged forms per draw would mean about 8^12 visits (to the depth limit) from a few kilobytes.
    let mut doc = Document::new_empty();
    let mut next = doc.add(Object::Stream(Stream::from_raw(form_dict(None), b"0 g".to_vec())));
    for _ in 0..20 {
        next = doc.add(Object::Stream(Stream::from_raw(form_dict(Some(next)), b"/X Do\n".repeat(8))));
    }
    page(&mut doc, &[("X", next)], "/X Do /X Do");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || sender.send(pdfcraft_optimize::convert_to_gray(&mut doc, None).unwrap()));
    let report = receiver.recv_timeout(std::time::Duration::from_secs(20)).expect("conversion finishes");
    assert_eq!((report.pages, report.forms, report.operators), (1, 0, 0));
}

#[test]
fn gray_conversion_follows_each_context_of_a_form_without_resources() {
    // A form with no /Resources draws /Im from its caller's: gray on one page, colour on the other.
    // Either order, each page must end up drawing its own image, and the colour one in gray. A
    // /Resources that is not a dictionary (here a missing object) counts as none.
    for (colour_first, dangling) in [(false, false), (true, false), (false, true)] {
        let mut doc = Document::new_empty();
        let mut dict = form_dict(None);
        if dangling {
            dict.set(b"Resources".to_vec(), Object::Ref(ObjRef::new(999, 0)));
        }
        let form = doc.add(Object::Stream(Stream::from_raw(dict, b"/Im Do".to_vec())));
        let gray = image(&mut doc, 4, 4, 1, false, None);
        let colour = image(&mut doc, 6, 6, 3, false, None);
        let order = if colour_first { [colour, gray] } else { [gray, colour] };
        let pages = order.map(|im| page(&mut doc, &[("X", form), ("Im", im)], "/X Do"));
        let report = pdfcraft_optimize::convert_to_gray(&mut doc, None).unwrap();
        assert_eq!(report.images, 1, "colour first: {colour_first}, dangling: {dangling}");
        for (p, original) in pages.into_iter().zip(order) {
            let res = |d: &Dict| d.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned());
            let page_res = res(doc.get(p).as_dict().unwrap()).unwrap();
            let xo =
                |r: &Dict, name: &[u8]| r.get(b"XObject").and_then(|x| doc.resolve(x).as_dict().cloned()).and_then(|x| x.reference(name)).unwrap();
            let drawn = stream(&doc, xo(&page_res, b"X"));
            let im = stream(&doc, xo(&res(&drawn.dict).unwrap_or(page_res), b"Im"));
            assert_eq!(im.dict.int(b"Width"), stream(&doc, original).dict.int(b"Width"), "colour first: {colour_first}, dangling: {dangling}");
            assert_eq!(im.dict.name(b"ColorSpace"), Some(&b"DeviceGray"[..]), "colour first: {colour_first}, dangling: {dangling}");
        }
    }
}

#[test]
fn gray_conversion_revisits_a_form_first_reached_at_the_depth_limit() {
    // The page reaches S first through ten nested forms, where S's own child T (red) is past the
    // depth limit, then draws S directly: that draw must still convert T.
    let mut doc = Document::new_empty();
    let t = doc.add(Object::Stream(Stream::from_raw(form_dict(None), b"1 0 0 rg".to_vec())));
    let s = doc.add(Object::Stream(Stream::from_raw(form_dict(Some(t)), b"/X Do".to_vec())));
    let mut chain = s;
    for _ in 0..11 {
        chain = doc.add(Object::Stream(Stream::from_raw(form_dict(Some(chain)), b"/X Do".to_vec())));
    }
    page(&mut doc, &[("A", chain), ("S", s)], "/A Do /S Do");
    let report = pdfcraft_optimize::convert_to_gray(&mut doc, None).unwrap();
    assert_eq!((report.forms, report.operators), (2, 1));
}

/// A page whose content stream is cut short is left as it is: writing back the part that decoded
/// would drop the rest of the page.
#[test]
fn gray_conversion_leaves_a_page_whose_content_is_cut_short() {
    let mut doc = Document::new_empty();
    let p = page(&mut doc, &[], "");
    let content = "1 0 0 rg 0 0 10 10 re f\n".repeat(400);
    let whole = Stream::flate(Dict::new(), content.as_bytes());
    let mut cut = whole.dict.clone();
    cut.set(b"Length".to_vec(), Object::Int((whole.raw.len() / 2) as i64));
    let cut = Stream::from_raw(cut, whole.raw[..whole.raw.len() / 2].to_vec());
    let c = doc.get(p).as_dict().and_then(|d| d.reference(b"Contents")).unwrap();
    doc.set(c, Object::Stream(cut.clone()));
    let report = pdfcraft_optimize::convert_to_gray(&mut doc, None).unwrap();
    assert_eq!((report.operators, report.left), (0, 1));
    assert_eq!(doc.get(p).as_dict().and_then(|d| d.reference(b"Contents")), Some(c));
    let Object::Stream(s) = &*doc.get(c) else { panic!("the content stream is gone") };
    assert_eq!(s.raw, cut.raw);
}

fn form_dict(child: Option<ObjRef>) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    d.set(b"BBox".to_vec(), Object::Array(vec![0.into(), 0.into(), 10.into(), 10.into()]));
    if let Some(c) = child {
        let mut x = Dict::new();
        x.set(b"X".to_vec(), Object::Ref(c));
        let mut res = Dict::new();
        res.set(b"XObject".to_vec(), Object::Dict(x));
        d.set(b"Resources".to_vec(), Object::Dict(res));
    }
    d
}

#[test]
fn gray_conversion_converts_a_form_once_for_pages_sharing_its_resources() {
    // Pages sharing one indirect resource dictionary give a form without /Resources the same
    // context, so it is converted once rather than once per page.
    let mut doc = Document::new_empty();
    let form = doc.add(Object::Stream(Stream::from_raw(form_dict(None), b"/Im Do".to_vec())));
    let colour = image(&mut doc, 6, 6, 3, false, None);
    let mut x = Dict::new();
    x.set(b"X".to_vec(), Object::Ref(form));
    x.set(b"Im".to_vec(), Object::Ref(colour));
    let mut res = Dict::new();
    res.set(b"XObject".to_vec(), Object::Dict(x));
    let shared = doc.add(Object::Dict(res));
    for _ in 0..3 {
        let p = page(&mut doc, &[], "/X Do");
        doc.update_dict(p, |d| d.set(b"Resources".to_vec(), Object::Ref(shared))).unwrap();
    }
    let report = pdfcraft_optimize::convert_to_gray(&mut doc, None).unwrap();
    assert_eq!((report.pages, report.forms, report.images), (3, 1, 1));
}

#[test]
fn cleanup_can_cancel_while_scanning_decoded_content() {
    use pdfcraft_optimize::{Stage, optimize_with_progress};

    let settings = Settings { merge_fragments: false, flate_unencoded: false, lzw_to_flate: false, ..Settings::default() };
    // The page reaches the clean-up progress interval without naming its unused resource.
    let content = " ".repeat(64 << 20);
    for continue_cleanup in [false, true] {
        let mut doc = Document::new_empty();
        let unused = form(&mut doc, None, "");
        let page = page(&mut doc, &[("Unused", unused)], &content);
        let mut cleanup_checks = 0;
        let result = optimize_with_progress(&mut doc, &settings, &mut |stage| {
            if stage != Stage::CleanUp {
                return true;
            }
            cleanup_checks += 1;
            cleanup_checks == 1 || continue_cleanup
        });

        assert_eq!(cleanup_checks, 2);
        if continue_cleanup {
            assert_eq!(result.unwrap().unused_xobjects, 1);
            assert!(page_xobjects(&doc, page).is_empty());
        } else {
            assert!(matches!(result, Err(OptimizeError::Cancelled)));
            assert_eq!(page_xobjects(&doc, page), ["Unused"]);
        }
    }
}
