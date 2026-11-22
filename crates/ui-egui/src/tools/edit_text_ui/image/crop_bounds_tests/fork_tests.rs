// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

#[test]
fn crop_drag_survives_bounds_that_round() {
    // 6.1 - 0.1 rounds to 6.0, and 6.1 - 6.0 is then just below 0.1.
    let (low, high) = (0.1_f32, 6.1_f32);
    assert!(high - (high - low) < low);
    for bounds in [
        Rect::from_min_max(Pos2::new(low, 0.0), Pos2::new(high, 100.0)),
        Rect::from_min_max(Pos2::new(0.0, low), Pos2::new(100.0, high)),
        Rect::from_min_max(Pos2::new(low, low), Pos2::new(high, high)),
    ] {
        for drag in [
            Rect::from_min_max(Pos2::new(-5.0, -5.0), Pos2::new(50.0, 50.0)),
            Rect::from_min_max(Pos2::new(3.0, 3.0), Pos2::new(3.0, 3.0)),
            Rect::from_min_max(Pos2::new(50.0, 50.0), Pos2::new(60.0, 60.0)),
        ] {
            let kept = clamp_crop(bounds, drag);
            assert!(kept.min.x >= bounds.min.x && kept.min.y >= bounds.min.y, "{kept:?}");
            assert!(kept.max.x <= bounds.max.x && kept.max.y <= bounds.max.y, "{kept:?}");
            assert!(kept.min.x <= kept.max.x && kept.min.y <= kept.max.y, "{kept:?}");
        }
    }
}
