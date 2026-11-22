// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

#[test]
fn page_tree_counts_do_not_size_the_page_list() {
    // Vendored lopdf patch: the page iterator's size hint summed each Pages node's `/Count`,
    // and `get_pages` preallocated from it: a fuzzed `/Count 4294967295` reserved 48 GiB.
    use lopdf::{Object, dictionary};
    let mut doc = lopdf::Document::with_version("1.7");
    let pages = doc.new_object_id();
    let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages });
    let huge = |doc: &mut lopdf::Document| {
        doc.add_object(dictionary! { "Type" => "Pages", "Parent" => pages, "Kids" => Vec::<Object>::new(), "Count" => i64::MAX })
    };
    let (a, b) = (huge(&mut doc), huge(&mut doc));
    let kids = vec![Object::Reference(page), Object::Reference(a), Object::Reference(b)];
    doc.objects.insert(pages, dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => 4_294_967_295i64 }.into());
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
    doc.trailer.set("Root", catalog);

    let (low, high) = doc.page_iter().size_hint();
    assert!(low <= 1 && high.is_some_and(|h| h <= doc.objects.len()), "{low} {high:?}");
    assert_eq!(doc.get_pages(), [(1, page)].into());
}
