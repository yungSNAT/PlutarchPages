//! The fork's tests of FormCalc built-in functions.

use crate::Limits;
use crate::formcalc::run_formcalc;
use crate::xfa::{XfaDoc, XfaEvent, XfaNode};

fn eval(script: &str) -> crate::xfa::XfaOutcome {
    run_formcalc(script, &XfaEvent::default(), &XfaDoc::default(), &XfaNode::default(), Limits::default())
}

fn ok(script: &str) -> String {
    let out = eval(script);
    assert_eq!(out.error, None, "{script} -> {:?}", out.error);
    out.result.unwrap_or_default()
}

#[test]
fn arithmetic_string_and_date_builtins_cover_all_arms() {
    assert!(eval("Mod(5, 0)").error.is_some_and(|e| e.contains("division by zero")));
    assert_eq!(ok("Avg()"), "");
    assert_eq!(ok("Choose(0, \"a\", \"b\")"), "");
    assert_eq!(ok("Replace(\"abc\", \"\", \"x\")"), "abc");
    assert_eq!(ok("Uuid()").len(), 32);
    assert_eq!(ok("Encode(\"<a&\"\"b\"\">\", \"html\")"), "&lt;a&amp;&quot;b&quot;&gt;");
    assert_eq!(ok("Encode(\"<a&\"\"b\"\">\", \"xml\")"), "&lt;a&amp;&quot;b&quot;&gt;");
    assert_eq!(ok("Ref()"), "");
    assert_eq!(ok("Ref(42)"), "42");

    // String length cap in `builtin()` and `Stuff()`
    assert!(eval("Encode(Space(400000), \"url\")").error.is_some_and(|e| e.contains("string is too long")));
    assert!(eval("Stuff(Space(600000), 1, 0, Space(600000))").error.is_some_and(|e| e.contains("string is too long")));

    // Date and time builtins
    // Today and now: whatever the clock reads, a whole day number and a time of day in ms.
    let today: f64 = ok("Date()").parse().unwrap_or(0.0);
    assert!(today >= 1.0 && today.fract() == 0.0, "today={today}");
    let ms: f64 = ok("Time()").parse().unwrap_or(-1.0);
    assert!((0.0..86_400_000.0).contains(&ms) && ms.fract() == 0.0, "ms={ms}");
    assert_eq!(ok("DateFmt(1)"), "M/D/YY");
    assert_eq!(ok("DateFmt(2)"), "MMM D, YYYY");
    assert_eq!(ok("DateFmt(3)"), "MMMM D, YYYY");
    assert_eq!(ok("DateFmt(0)"), "MMM D, YYYY");
    assert_eq!(ok("LocalDateFmt(9)"), "MMM D, YYYY");
    assert_eq!(ok("Num2Time(-1)"), "");
}

#[test]
fn financial_and_unit_builtins_cover_all_arms() {
    // Financial builtins: valid + validation error branches + early payoff break
    assert!(eval("Pmt(1000, 0.05, 0)").error.is_some_and(|e| e.contains("positive")));
    assert!(eval("Term(0, 0.05, 1000)").error.is_some_and(|e| e.contains("positive")));
    let term: f64 = ok("Term(100, 0.05, 1000)").parse().unwrap_or(0.0);
    assert!(term > 8.0 && term < 9.0, "term={term}");
    assert!(eval("CTerm(0, 2000, 1000)").error.is_some_and(|e| e.contains("positive")));
    let cterm: f64 = ok("CTerm(0.05, 2000, 1000)").parse().unwrap_or(0.0);
    assert!(cterm > 14.0 && cterm < 15.0, "cterm={cterm}");
    assert!(eval("Rate(2000, 1000, 0)").error.is_some_and(|e| e.contains("positive")));
    let rate: f64 = ok("Rate(2000, 1000, 10)").parse().unwrap_or(0.0);
    assert!(rate > 0.07 && rate < 0.08, "rate={rate}");
    let npv: f64 = ok("NPV(0.1, 100, 200, 300)").parse().unwrap_or(0.0);
    assert!(npv > 480.0 && npv < 485.0, "npv={npv}");
    let ppmt: f64 = ok("PPmt(100, 0.05, 60, 1, 5)").parse().unwrap_or(0.0);
    assert!((ppmt - 100.0).abs() < 1e-6, "ppmt={ppmt}");
    assert!(eval("Apr(0, 100, 12)").error.is_some_and(|e| e.contains("positive")));
    let apr: f64 = ok("Apr(1000, 100, 12)").parse().unwrap_or(0.0);
    assert!(apr > 0.3 && apr < 0.4, "apr={apr}");

    // Unit builtins
    assert_eq!(ok("UnitType(\"2in\")"), "in");
    assert_eq!(ok("UnitType(\"2mm\")"), "mm");
    assert_eq!(ok("UnitType(\"2pt\")"), "pt");
    assert_eq!(ok("UnitType(\"2pc\")"), "pc");
    assert_eq!(ok("UnitType(\"2foo\")"), "in");
    let uv: f64 = ok("UnitValue(\"2.54cm\", \"pc\")").parse().unwrap_or(0.0);
    assert!((uv - 6.0).abs() < 1e-6, "uv={uv}");
}

#[test]
fn time_formats_follow_the_en_us_styles() {
    // XFA 3.3 FormCalc TimeFmt and Designer's en_US time patterns: default and medium h:MM:SS A,
    // short h:MM A, long and full h:MM:SS A Z; an invalid style gives the default.
    assert_eq!(ok("TimeFmt()"), "h:MM:SS A");
    assert_eq!(ok("TimeFmt(0)"), "h:MM:SS A");
    assert_eq!(ok("TimeFmt(1)"), "h:MM A");
    assert_eq!(ok("TimeFmt(2)"), "h:MM:SS A");
    assert_eq!(ok("TimeFmt(3)"), "h:MM:SS A Z");
    assert_eq!(ok("TimeFmt(4)"), "h:MM:SS A Z");
    assert_eq!(ok("TimeFmt(9)"), "h:MM:SS A");
    assert_eq!(ok("LocalTimeFmt(1)"), "h:MM A");
}
