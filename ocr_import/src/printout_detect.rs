//! Detect printout type from full-page OCR results.

use pentacam_types::PrintoutType;
use super::ocr_engine::OcrItem;

/// Detect the printout type by matching OCR text against known title patterns.
pub fn detect_printout_type(_items: &[OcrItem]) -> Option<PrintoutType> {
    todo!("Regex match on title bar text regions")
}
