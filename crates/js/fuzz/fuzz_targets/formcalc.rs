//! Exercise the FormCalc parser and interpreter against a small synthetic form.
#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfcraft_js::Limits;
use pdfcraft_js::formcalc::run_formcalc;
use pdfcraft_js::xfa::{XfaDoc, XfaEvent, XfaKind, XfaNode};

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    let Ok(script) = std::str::from_utf8(data) else { return };
    let field = XfaNode {
        name: "total".into(),
        som: "form[0].total[0]".into(),
        kind: Some(XfaKind::Field),
        value: "12".into(),
        numeric: true,
        ..XfaNode::default()
    };
    let event = XfaEvent { activity: "calculate".into(), target: field.som.clone(), ..XfaEvent::default() };
    let root = XfaNode { name: "form".into(), som: "form[0]".into(), kind: Some(XfaKind::Form), children: vec![field], ..XfaNode::default() };
    // The viewer adds a deadline this harness lacks, so keep loops short enough to finish
    // inside libFuzzer's timeout under the sanitiser.
    let limits = Limits { loop_iterations: 10_000, recursion: 32 };
    let _ = run_formcalc(script, &event, &XfaDoc::default(), &root, limits);
});
