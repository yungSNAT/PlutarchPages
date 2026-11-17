//! Office documents (Word, Excel, PowerPoint, OpenDocument, RTF): recognising them. PlutarchPages
//! converts them through `pdfcraft-external`, which runs an installed LibreOffice only when the
//! user turns external converters on; PdfCraft never parses these formats itself.

/// File extensions of Office documents. [`crate::OFFICE_FILES`] are those converted here.
pub const OFFICE_EXTENSIONS: &[&str] = &[
    // Word and Writer
    "doc", "docx", "docm", "dot", "dotx", "dotm", "odt", "ott", "rtf", //
    // Excel and Calc
    "xls", "xlsx", "xlsm", "xlt", "xltx", "xltm", "ods", "ots", //
    // PowerPoint and Impress
    "ppt", "pptx", "pptm", "pot", "potx", "potm", "pps", "ppsx", "ppsm", "odp", "otp",
];

/// The OLE compound-file signature of Office 97–2003 files (`.doc`, `.xls`, `.ppt`).
const OLE: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// `name`'s extension as it appears in [`OFFICE_EXTENSIONS`], if it is one.
fn office_extension(name: &str) -> Option<&'static str> {
    let (_, ext) = name.rsplit_once('.')?;
    OFFICE_EXTENSIONS.iter().copied().find(|e| ext.eq_ignore_ascii_case(e))
}

/// Whether `bytes` named `name` is an Office document: an Office extension, and the bytes start
/// like one (an OLE compound file, a ZIP package as OOXML and OpenDocument are, or RTF).
pub fn is_office(name: &str, bytes: &[u8]) -> bool {
    office_extension(name).is_some() && (bytes.starts_with(OLE) || bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"{\\rtf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCX_HEAD: &[u8] = b"PK\x03\x04\x14\x00\x06\x00";

    #[test]
    fn office_files_are_recognised_by_extension_and_signature() {
        assert!(is_office("Report.docx", DOCX_HEAD));
        assert!(is_office("SHEET.XLSX", DOCX_HEAD));
        assert!(is_office("deck.odp", DOCX_HEAD));
        assert!(is_office("old.doc", OLE));
        assert!(is_office("old.ppt", &[OLE, b"rest"].concat()));
        assert!(is_office("letter.rtf", b"{\\rtf1\\ansi"));
        // Word saves RTF under .doc too.
        assert!(is_office("letter.doc", b"{\\rtf1\\ansi"));
    }

    #[test]
    fn other_files_are_not_office_documents() {
        // The right signature with another extension, or an Office name with other bytes.
        assert!(!is_office("archive.zip", DOCX_HEAD));
        assert!(!is_office("Report.docx", b"%PDF-1.7"));
        assert!(!is_office("Report.docx", b"plain text"));
        assert!(!is_office("Report.docx", b""));
        assert!(!is_office("docx", DOCX_HEAD));
        assert!(!is_office("", DOCX_HEAD));
        // A multi-byte name never splits a character.
        assert!(!is_office("résumé.dócx", DOCX_HEAD));
        assert!(is_office("résumé.docx", DOCX_HEAD));
    }
}
