// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// A book bound on the right reads across each spread from the right: the earlier page on the
/// right, a lone cover on the left. Rects stay in page order, and a page alone on its row in place.
#[test]
fn right_bound_spreads_put_the_earlier_page_on_the_right() {
    let page = || pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 };
    let info = DocInfo { pages: vec![page(), page(), page()], ..Default::default() };
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    (v.layout, v.zoom) = (PageLayout::TwoUp, 1.0);
    let w = v.content_width(&info, 1200.0);
    let left_bound = v.layout(&info, w, 1.0);
    assert!(left_bound[0].max.x < left_bound[1].min.x, "the earlier page on the left: {left_bound:?}");
    v.right_to_left = true;
    let right_bound = v.layout(&info, w, 1.0);
    assert!(right_bound[1].max.x < right_bound[0].min.x, "the earlier page on the right: {right_bound:?}");
    assert!(
        (right_bound[0].min.x - left_bound[1].min.x).abs() < 1e-3 && (right_bound[1].min.x - left_bound[0].min.x).abs() < 1e-3,
        "the same spread"
    );
    assert_eq!(right_bound[0].min.y, right_bound[1].min.y, "one row");
    assert!((right_bound[2].min.x - left_bound[2].min.x).abs() < 1e-3, "the last page, alone on its row");
    v.cover = true;
    let covered = v.layout(&info, w, 1.0);
    assert!(covered[0].max.x <= w / 2.0, "the cover on the left: {:?} in {w}", covered[0]);
    assert!(covered[2].max.x < covered[1].min.x && covered[1].min.y > covered[0].max.y, "{covered:?}");
}

#[test]
fn an_earlier_zoom_tile_counts_as_painted_only_inside_the_viewport() {
    let view = [0.0, 0.5, 1.0, 1.0];
    // The top half of the page, off screen: painted and clipped, but nothing shows.
    assert!(!super::super::uv_overlap([0.0, 0.0, 1.0, 0.5], view));
    // Reaching into the viewport.
    assert!(super::super::uv_overlap([0.0, 0.25, 0.5, 0.75], view));
    // Touching the edge only.
    assert!(!super::super::uv_overlap([0.0, 0.0, 0.5, 0.5], [0.5, 0.5, 1.0, 1.0]));
}
