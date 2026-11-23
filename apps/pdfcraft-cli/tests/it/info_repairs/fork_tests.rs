// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! PlutarchPages' own tests of the parent module, kept apart from upstream's so upstream merges
//! leave its file alone (PATCHES.md, 'Merging upstream').

use super::*;

/// The document information is what the app shows in Document Properties: a title, author,
/// producer or creator stated only in XMP counts (#2373).
#[test]
fn info_reads_document_information_stated_only_in_xmp() {
    let xmp = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
               <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
               <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Quarterly report</rdf:li></rdf:Alt></dc:title>\
               <dc:creator><rdf:Seq><rdf:li>Ada</rdf:li></rdf:Seq></dc:creator></rdf:Description>\
               <rdf:Description rdf:about=\"\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\
               <pdf:Producer>Press 2</pdf:Producer><xmp:CreatorTool>Writer 9</xmp:CreatorTool></rdf:Description></rdf:RDF></x:xmpmeta>";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /Metadata 4 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 120 60] >>".to_string(),
        format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len()),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for off in &offsets {
        bytes.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let json = info_of(&bytes, "xmp");
    let got = ["title", "author", "producer", "creator"].map(|k| json[k].as_str().map(str::to_owned));
    assert_eq!(got, [Some("Quarterly report"), Some("Ada"), Some("Press 2"), Some("Writer 9")].map(|v| v.map(str::to_owned)), "{json}");
    assert_eq!(json["file_size"], serde_json::json!(bytes.len()), "{json}");
}
