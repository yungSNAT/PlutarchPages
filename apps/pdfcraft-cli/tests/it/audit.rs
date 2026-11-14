//! `audit` reports the rectangles of faux redactions where `doc_audit_redactions` does: in
//! points from the top-left of the page as displayed, after `/Rotate`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_pdfcraft-cli");

/// Two US Letter pages with the same content. The second page is turned a quarter. `catalog` and
/// `trailer` are extra entries for the catalogue and the trailer; object 7 is a small /Info.
fn fixture(body: &str, catalog: &str, trailer: &str) -> Vec<u8> {
    let objs: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R{catalog} >>"),
        "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /MediaBox [0 0 612 792] >>".into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /Rotate 90 /Contents 6 0 R /Resources << /Font << /F1 3 0 R >> >> >>".into(),
        format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()),
        "<< /Author (Synthetic Author) >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R{trailer} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// The `rect` of each finding, in page order.
fn rects(report: &serde_json::Value) -> Vec<serde_json::Value> {
    report["findings"].as_array().expect("findings").iter().map(|f| f["rect"].clone()).collect()
}

#[test]
fn audit_rectangles_match_the_tool_in_top_left_points() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let body = "BT /F1 24 Tf 100 700 Td (TOP SECRET) Tj ET\n0 g\n90 688 220 32 re f";
    std::fs::write(dir.join("faux.pdf"), fixture(body, "", "")).unwrap();

    let out = Command::new(BIN).arg("audit").arg(dir.join("faux.pdf")).output().unwrap();
    assert!(!out.status.success(), "findings must fail the audit");
    let cli: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(cli["count"], 2, "{cli}");
    // Page 1 upright: y measured down from the top of the 792 pt page. Page 2 turned a quarter
    // clockwise: 792 pt wide, and the cover's user-space y becomes its x.
    let expected = [serde_json::json!([90.0, 72.0, 310.0, 104.0]), serde_json::json!([688.0, 90.0, 720.0, 310.0])];
    assert_eq!(rects(&cli), expected, "{cli}");

    // The tool reports the same rectangles.
    let steps = serde_json::json!([
        { "tool": "doc_open", "args": { "path": "faux.pdf" } },
        { "tool": "doc_audit_redactions", "args": { "doc": 1 } },
    ]);
    std::fs::write(dir.join("steps.json"), steps.to_string()).unwrap();
    let out = Command::new(BIN).arg("run").arg("--script").arg(dir.join("steps.json")).arg("--root").arg(&dir).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let tool = serde_json::Deserializer::from_str(&stdout).into_iter::<serde_json::Value>().last().unwrap().unwrap();
    assert_eq!(rects(&tool), rects(&cli), "{tool}");
}

#[test]
fn advisory_findings_do_not_fail_the_cli_audit() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("advisory.pdf");
    let body = "BT /F1 20 Tf 72 692 Td [-1000] TJ ET\n0 g 71 688 22 21 re f";
    std::fs::write(&path, fixture(body, "", " /Info 7 0 R")).unwrap();

    let out = Command::new(BIN).arg("audit").arg(&path).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["count"], 2, "{report}");
    assert_eq!(report["findings"][0]["kind"], "retained-advance");
    assert_eq!(report["document_findings"][0]["kind"], "document-info");
    std::fs::remove_file(path).unwrap();
}

/// A /Info or /Metadata of the wrong type, as a catalogue entry and a trailer entry, with the
/// document finding each one gives.
const MALFORMED: [(&str, &str, &str); 2] =
    [("", " /Info (not a dictionary)", "malformed-document-info"), (" /Metadata (not a stream)", "", "malformed-xmp-metadata")];

#[test]
fn malformed_metadata_alone_does_not_fail_the_cli_audit() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("malformed-only.pdf");
    for (catalog, trailer, kind) in MALFORMED {
        std::fs::write(&path, fixture("BT /F1 24 Tf 100 700 Td (PUBLIC) Tj ET", catalog, trailer)).unwrap();

        let out = Command::new(BIN).arg("audit").arg(&path).output().unwrap();
        assert!(out.status.success(), "{kind}: {}", String::from_utf8_lossy(&out.stderr));
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["count"], 0, "{report}");
        assert_eq!(report["document_findings"], serde_json::json!([{ "kind": kind, "count": 1 }]), "{report}");
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn malformed_metadata_beside_covered_text_still_fails_the_cli_audit() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("malformed-leak.pdf");
    for (catalog, trailer, kind) in MALFORMED {
        std::fs::write(&path, fixture("BT /F1 24 Tf 100 700 Td (TOP SECRET) Tj ET\n0 g\n90 688 220 32 re f", catalog, trailer)).unwrap();

        let out = Command::new(BIN).arg("audit").arg(&path).output().unwrap();
        assert!(!out.status.success(), "{kind}: covered text must fail the audit");
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["count"], 2, "{report}");
        assert!(report["findings"].as_array().unwrap().iter().all(|f| f["kind"] == "covered-text"), "{report}");
        assert_eq!(report["document_findings"], serde_json::json!([{ "kind": kind, "count": 1 }]), "{report}");
    }
    std::fs::remove_file(path).unwrap();
}

/// A page the audit can't read fails it, and the error names the page as a reader counts: here
/// the first page's text runs past the text reader's budget.
#[test]
fn an_unreadable_page_fails_the_cli_audit_and_is_numbered_from_1() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-cli-audit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("unreadable.pdf");
    std::fs::write(&path, fixture("/PCSoft << /N 20000000 >> BDC EMC", "", "")).unwrap();

    let out = Command::new(BIN).arg("audit").arg(&path).output().unwrap();
    assert!(!out.status.success(), "an audit error must fail the audit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unreadable.pdf: page 1: "), "{stderr}");
    std::fs::remove_file(path).unwrap();
}
