// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' rows that craft-i18n doesn't have (`crates/ui-egui/i18n-fork`, embedded by
//! `build_i18n_fork.rs`; PATCHES.md).

use super::*;

/// Every overlay locale is one PlutarchPages offers and embeds, each overlay row is what a lookup
/// in its language gives, and no overlay row is malformed or also in craft-i18n's catalog (defined
/// twice). Built without craft-i18n there is nothing to check, which a build with
/// `CRAFT_I18N_REQUIRED=1` (CI and releases) refuses.
#[test]
fn the_fork_overlay_is_embedded_and_served() {
    if !needs_catalogs() {
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n-fork");
    let unescape = |s: &str| s.replace("\\\\", "\u{0}").replace("\\n", "\n").replace("\\t", "\t").replace('\u{0}', "\\");
    let mut locales = 0;
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("the overlay directory") {
        let path = entry.expect("an overlay file").path();
        let code = path.file_stem().and_then(|s| s.to_str()).expect("a locale code").to_owned();
        let lang = Lang::from_code(&code).unwrap_or_else(|| panic!("{code}: an overlay for a language PlutarchPages doesn't offer or embed"));
        let text = std::fs::read_to_string(&path).expect("an overlay");
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        for line in text.lines().map(|l| l.strip_suffix('\r').unwrap_or(l)).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let [context, source, translation] =
                line.split('\t').map(unescape).collect::<Vec<_>>().try_into().unwrap_or_else(|_| panic!("{code}: not 3 columns: {line:?}"));
            let served = if context.is_empty() { Some(tr(lang, &source)) } else { lang.i18n().get(lang.lang, &context, &source) };
            assert_eq!(served, Some(translation.as_str()), "{code}: ({context:?}, {source:?})");
            checked += 1;
        }
        // The joined catalog's first lines are the overlay's, so a problem on one of them is the
        // overlay's; a row craft-i18n has too is reported where craft-i18n's copy is.
        let overlay_lines = text.lines().count();
        let problems: Vec<String> = I18N
            .check_all()
            .into_iter()
            .filter(|p| {
                let line = p.strip_prefix(&format!("{code}/app: line ")).and_then(|r| r.split(':').next()).and_then(|n| n.parse::<usize>().ok());
                line.is_some_and(|n| n <= overlay_lines || p.contains("defined twice"))
            })
            .collect();
        assert!(problems.is_empty(), "{code}: {problems:#?}");
        locales += 1;
    }
    assert!(locales > 0 && checked > 0, "the overlay directory holds no rows");
}
