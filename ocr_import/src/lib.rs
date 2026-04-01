//! OCR-based extraction from Pentacam PDF printouts and images.
//!
//! Supports three input modes:
//! - Embedded PDF bytes (from DICOM via `dicom_import`)
//! - Standalone PDF files
//! - Standalone image files (PNG/JPG — treated as single rendered page)
//!
//! Pipeline per page:
//! 1. Render PDF page to 300 DPI PNG (or use image directly)
//! 2. Full-page OCR via oar-ocr
//! 3. Detect printout type from OCR text
//! 4. Locate fields via label matching + affine archetype fit
//! 5. Read each field crop with preprocessing
//! 6. Post-processing passes (digit confusion, sign detection)
//! 7. Quality gate (min 25 located fields)

pub mod ocr_engine;
pub mod printout_detect;
pub mod field_locate;
pub mod field_read;
pub mod four_maps;
pub mod topometric;
pub mod belin;
pub mod demographics;

use pentacam_types::{PrintoutResult, PdfDemographics};
use std::path::Path;

/// Process a PDF (from bytes) and return results for each recognized page.
pub fn process_pdf_bytes(
    pdf_bytes: &[u8],
    _need_demographics: bool,
) -> Result<(Vec<PrintoutResult>, Option<PdfDemographics>), String> {
    let _ = pdf_bytes;
    todo!("Render each page, run OCR pipeline, collect PrintoutResults")
}

/// Process a standalone PDF file.
pub fn process_pdf_file(
    path: &Path,
    need_demographics: bool,
) -> Result<(Vec<PrintoutResult>, Option<PdfDemographics>), String> {
    let _ = (path, need_demographics);
    todo!("Read PDF bytes from file, delegate to process_pdf_bytes")
}

/// Process a standalone image file (single page).
pub fn process_image_file(
    path: &Path,
    need_demographics: bool,
) -> Result<(Vec<PrintoutResult>, Option<PdfDemographics>), String> {
    let _ = (path, need_demographics);
    todo!("Load image, run OCR pipeline for one page")
}
