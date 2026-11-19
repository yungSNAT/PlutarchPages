//! This fork's Type 3 printing (PATCHES.md, 'Windows printing with Type 3 fonts'): the pages that
//! draw with Type 3 fonts go as pictures. It lives here, not in upstream's `printing.rs`, so an
//! upstream merge meets 2 calls there.

use pdfcraft_engine::print::{self, spool};
use serde_json::{Value, json};

use crate::{Args, Result};

/// The pictures `doc_print` draws the pages with Type 3 fonts as, when it prints the others as
/// they are (`type3_as_image`): by default when printing to a Windows printer, whose print
/// component can crash on them (#2684). None when every page already goes as a picture (`raster`).
pub(super) fn raster(a: &Args, to_printer: bool, raster: Option<&print::Raster>) -> Result<Option<print::Raster>> {
    if !a.opt_bool("type3_as_image")?.unwrap_or(spool::TYPE3_AS_IMAGE && to_printer) || raster.is_some() {
        return Ok(None);
    }
    let dpi = a.opt_num("dpi")?.unwrap_or(300.0);
    if !(72.0..=600.0).contains(&dpi) {
        return Err(super::bad("dpi must be between 72 and 600"));
    }
    Ok(Some(print::Raster { dpi, grayscale: a.opt_bool("grayscale")?.unwrap_or(false), colour: None }))
}

/// Lists in `doc_print`'s result the pages (1-based) that went as pictures.
pub(super) fn report(out: &mut Value, pages: &[usize]) {
    if !pages.is_empty() {
        out["type3_as_image"] = json!(pages.iter().map(|p| p.saturating_add(1)).collect::<Vec<_>>());
    }
}
