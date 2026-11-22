// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// The dark theme is true black for OLED screens; both themes take their accent and
/// selection from the Mermaid scheme and keep text on them readable.
#[test]
fn dark_is_true_black_and_the_scheme_colours_stay_readable() {
    let dark = Tokens::for_kind(ThemeKind::Dark);
    for c in [dark.titlebar, dark.chrome, dark.panel, dark.pasteboard] {
        assert_eq!(c, Color32::BLACK);
    }
    assert_eq!((dark.text, dark.selected), (IVORY, TYRIAN_PURPLE));
    for t in [Tokens::for_kind(ThemeKind::Light), dark] {
        assert_eq!(t.accent, CRIMSON);
        for (name, fg, bg) in [("text on selected", t.text, t.selected), ("links", t.accent_text, t.panel), ("ivory on accent", IVORY, t.accent)] {
            let r = contrast(fg, bg);
            assert!(r >= 4.5, "{:?} {name}: {r:.2}:1", t.kind);
        }
    }
}

/// Status colours read as text on every surface, and a passed result is Rhodesian green or its
/// tint, never another green.
#[test]
fn status_colours_stay_readable_on_every_surface() {
    for kind in [ThemeKind::Light, ThemeKind::Dark] {
        let t = Tokens::for_kind(kind);
        for (name, fg) in [("success", t.success), ("warning", t.warning), ("danger", t.danger), ("info", t.info)] {
            for bg in [t.chrome, t.panel, t.card] {
                let r = contrast(fg, bg);
                assert!(r >= 4.5, "{kind:?} {name}: {r:.2}:1");
            }
        }
    }
    assert_eq!(Tokens::for_kind(ThemeKind::Light).success, RHODESIAN_GREEN);
}

/// Overlays sit on page content, so they need 3:1 against white paper and a black photograph.
#[test]
fn on_page_colours_read_on_white_and_black_pages() {
    for (name, c) in [("select", ON_PAGE_SELECT), ("hint", ON_PAGE_HINT), ("alert", ON_PAGE_ALERT)] {
        for page in [Color32::WHITE, Color32::BLACK] {
            let r = contrast(c, page);
            assert!(r >= 3.0, "{name} on {page:?}: {r:.2}:1");
        }
    }
}

/// Interface colours live in this file, so an upstream merge can't bring its blue back
/// unnoticed. Colours a document or the user chose arrive as variables, not hex literals.
#[test]
fn chrome_literals_stay_in_the_theme() {
    fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("theme.rs") && !path.to_string_lossy().contains("tests") {
                let source = std::fs::read_to_string(&path).unwrap();
                let code = source.split("#[cfg(test)]").next().unwrap_or_default();
                for (n, line) in code.lines().enumerate() {
                    if ["from_rgb(0x", "from_rgba_unmultiplied(0x", "from_rgba_premultiplied(0x"].iter().any(|p| line.contains(p)) {
                        found.push(format!("{}:{}", path.display(), n + 1));
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src")), &mut found);
    assert!(found.is_empty(), "colour literals outside theme.rs: {found:#?}");
}
