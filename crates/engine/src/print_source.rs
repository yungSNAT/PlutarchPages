// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A document's snapshot for printing, owned so a print can be built off the UI thread: pages
//! going as pictures can take minutes to render.

use std::sync::Arc;

use pdfcraft_sign::{SignatureInfo, StatusCaptions};

use crate::export::ExportSource;
use crate::{EditError, LOW_RESOLUTION_PRINT_DPI, guard, print, print_layer_states, print_raster, set_default_layer_state};

/// What [`crate::Session::print_source`] took of a document.
pub struct PrintSource {
    /// What Print as image draws from, and the renderer's configuration.
    pub(crate) export: ExportSource,
    /// The working copy of a document with an editor.
    pub(crate) working: Option<pdfcraft_cos::Document>,
    pub(crate) bytes: Arc<Vec<u8>>,
    pub(crate) password: Option<String>,
    /// The layers as shown.
    pub(crate) layers: Vec<pdfcraft_render::Layer>,
    /// The Layers panel or an action has changed one of `layers`.
    pub(crate) layers_changed: bool,
    /// Table 22, bit 12 clear: only a low-level representation may be printed.
    pub(crate) low_resolution_only: bool,
    /// The signatures, to print their states as the screen shows them.
    pub(crate) signatures: Arc<Vec<SignatureInfo>>,
    pub(crate) captions: Arc<StatusCaptions>,
}

/// A print-ready PDF.
pub struct Printed {
    pub bytes: Vec<u8>,
    /// The pages (0-based) that went as pictures for [`print::Settings::type3_raster`].
    pub type3_pages: Vec<usize>,
}

impl PrintSource {
    fn cos(&self) -> Result<pdfcraft_cos::Document, EditError> {
        match &self.working {
            Some(cos) => Ok(cos.clone()),
            None => {
                pdfcraft_cos::Document::open_with_password(self.bytes.clone(), self.password.as_deref()).map_err(|e| EditError::Write(e.to_string()))
            }
        }
    }

    /// The print-ready PDF for `settings` (sheets laid out for the paper; see `pdfcraft-print`).
    pub fn print(&self, settings: &print::Settings) -> Result<Printed, EditError> {
        // Table 22, bit 12 clear: the pages always go as pictures, at no more than the low
        // resolution.
        let low_res = self.low_resolution_only.then(|| print::Raster {
            dpi: settings.raster.map_or(LOW_RESOLUTION_PRINT_DPI, |r| r.dpi.min(LOW_RESOLUTION_PRINT_DPI)),
            grayscale: settings.raster.is_some_and(|r| r.grayscale),
        });
        // Print as image: the pages used become pictures first, then the picture document is
        // imposed in their place (its page k is the k-th distinct source page used).
        if let Some(raster) = low_res.as_ref().or(settings.raster.as_ref()) {
            let mut distinct: Vec<usize> = Vec::new();
            for &p in &settings.pages {
                if !distinct.contains(&p) {
                    distinct.push(p);
                }
            }
            let bytes = print_raster::rasterize(&self.export, &self.layers, &distinct, raster, settings.content).map_err(EditError::Print)?;
            let pictures = pdfcraft_cos::Document::open(Arc::new(bytes)).map_err(|e| EditError::Print(e.to_string()))?;
            let remapped = print::Settings {
                pages: settings.pages.iter().map(|p| distinct.iter().position(|d| d == p).unwrap_or(0)).collect(),
                content: print::Content::Document,
                raster: None,
                type3_raster: None,
                ..settings.clone()
            };
            let bytes = pdfcraft_print::impose(&pictures, &remapped).map_err(|e| EditError::Print(e.to_string()))?;
            return Ok(Printed { bytes, type3_pages: Vec::new() });
        }
        let mut cos = self.cos()?;
        // Comments and fields without an appearance print as the page shows them (and as Print
        // as Image already printed them): the vector print drew only appearance streams, so a
        // text box without one was on screen but not on paper. ISO 32000-2 §12.5.3: an
        // annotation with the Print flag is printed.
        if let Some(displayed) = guard(|| crate::document::as_displayed(&cos)).ok().flatten() {
            cos = displayed;
        }
        // #3046: the layers print as they are shown. Once the Layers panel (or a set-layer
        // action) has changed one, the print copy's default configuration says which are on, so a
        // hidden layer stays off on paper too; untouched, the document's own configuration prints.
        // On top of either, the configuration's auto state for the Print event sets the layers it
        // names (`print_layer_states`), as printing does in a viewer (ISO 32000-2 §8.11.4.4).
        if let Some(printed) = print_layer_states(&cos, &self.layers) {
            set_default_layer_state(&mut cos, &printed);
        } else if self.layers_changed {
            set_default_layer_state(&mut cos, &self.layers);
        }
        // Signature states as the screen shows them, so not when its display copy could not be
        // written. This copy is only printed, never saved.
        if self.signatures.iter().any(|s| s.verdict_shown) {
            let _ = pdfcraft_sign::show_status_layers(&mut cos, &mut self.signatures.to_vec(), &self.captions);
        }
        // The pages that draw with Type 3 fonts go as pictures when asked (Windows printers,
        // #2684: its print component can crash the app on them); the others stay vector.
        let (type3_pages, pictures) = match &settings.type3_raster {
            Some(raster) => {
                let pages = pdfcraft_print::type3_pages(&cos, &settings.pages, settings.content);
                let pictures = if pages.is_empty() {
                    Default::default()
                } else {
                    // Drawn from a copy holding what the vector print holds.
                    let mut printed = cos.clone();
                    pdfcraft_print::keep_printed(&mut printed, settings.content);
                    print_raster::pictures(&self.export, &self.layers, &printed, &pages, raster).map_err(EditError::Print)?
                };
                (pages, pictures)
            }
            None => Default::default(),
        };
        let bytes = pdfcraft_print::impose_with_pictures(&cos, settings, &pictures).map_err(|e| EditError::Print(e.to_string()))?;
        Ok(Printed { bytes, type3_pages })
    }

    /// The pages among `pages` (0-based) that [`print::Settings::type3_raster`] prints as pictures
    /// under `content` ([`pdfcraft_print::type3_pages`]). Empty when the document can't be read,
    /// or when every page is a picture anyway because only a low-resolution print is allowed.
    pub fn type3_pages(&self, pages: &[usize], content: print::Content) -> Vec<usize> {
        if self.low_resolution_only {
            return Vec::new();
        }
        self.cos().map_or_else(|_| Vec::new(), |cos| pdfcraft_print::type3_pages(&cos, pages, content))
    }
}
