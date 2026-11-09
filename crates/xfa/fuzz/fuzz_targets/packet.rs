//! Decode an XFA packet, then parse and lay out its template with any datasets it carries.
#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfcraft_xfa::{decode_packet, layout_xml};

fuzz_target!(|data: &[u8]| {
    if data.len() > 64 * 1024 {
        return;
    }
    if let Some((xml, _)) = decode_packet(data) {
        let _ = layout_xml(&xml);
    }
});
