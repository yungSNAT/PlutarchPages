// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' rows that craft-i18n doesn't have (`crates/ui-egui/i18n-fork`, embedded by
//! `build_i18n_fork.rs`; PATCHES.md).

use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn overlays() -> BTreeMap<String, String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("i18n-fork");
    std::fs::read_dir(dir)
        .expect("the overlay directory")
        .map(|entry| {
            let path = entry.expect("an overlay file").path();
            let code = path.file_stem().and_then(|s| s.to_str()).expect("a locale code").to_owned();
            let text = std::fs::read_to_string(path).expect("an overlay");
            (code, text)
        })
        .collect()
}

fn overlay_rows<'a>(code: &'a str, text: &'a str) -> impl Iterator<Item = [String; 3]> + 'a {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let unescape = |s: &str| s.replace("\\\\", "\u{0}").replace("\\n", "\n").replace("\\t", "\t").replace('\u{0}', "\\");
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(move |line| line.split('\t').map(unescape).collect::<Vec<_>>().try_into().unwrap_or_else(|_| panic!("{code}: not 3 columns: {line:?}")))
}

/// A key belongs to the fork while at least one overlay carries it. Count actual lookups,
/// including a translation already present in the pinned upstream catalogue for another locale.
#[test]
fn fork_translation_gaps_report() {
    const START: &str = "=== translation gaps ===";
    const END: &str = "=== end of translation gaps ===";
    if !catalogs_embedded() {
        println!("{START}\nFork translation gaps: skipped, built without craft-i18n (set CRAFT_I18N_DIR)\n{END}");
        return;
    }
    let overlays = overlays();
    let keys: BTreeSet<(String, String)> =
        overlays.iter().flat_map(|(code, text)| overlay_rows(code, text).map(|[context, source, _]| (context, source))).collect();
    println!("{START}\nFork translation gaps: {} overlay keys", keys.len());
    for lang in Lang::all().filter(|lang| *lang != Lang::EN) {
        let mut kept = 0;
        let missing: Vec<_> = keys
            .iter()
            .filter(|(context, source)| match lang.i18n().get(lang.lang, context, source) {
                Some(translation) => {
                    kept += usize::from(translation == source);
                    false
                }
                None => true,
            })
            .collect();
        println!("  {:<8} {:>3} missing, {kept:>3} kept in English", lang.code(), missing.len());
        for (context, source) in missing {
            if context.is_empty() {
                println!("    {source:?}");
            } else {
                println!("    {context:?} / {source:?}");
            }
        }
    }
    println!("{END}");
}

/// Every overlay locale is one PlutarchPages offers and embeds, each overlay row is what a lookup
/// in its language gives, and no overlay row is malformed or also in craft-i18n's catalog (defined
/// twice). Built without craft-i18n there is nothing to check, which a build with
/// `CRAFT_I18N_REQUIRED=1` (CI and releases) refuses.
#[test]
fn the_fork_overlay_is_embedded_and_served() {
    if !needs_catalogs() {
        return;
    }
    let mut locales = 0;
    let mut checked = 0;
    for (code, text) in overlays() {
        let lang = Lang::from_code(&code).unwrap_or_else(|| panic!("{code}: an overlay for a language PlutarchPages doesn't offer or embed"));
        for [context, source, translation] in overlay_rows(&code, &text) {
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
