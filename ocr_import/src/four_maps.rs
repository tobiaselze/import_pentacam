//! 4 Maps Refractive / Selectable printout extraction.
//!
//! 40 fields: keratometry (front/back), pachymetry, volumes, coordinates, etc.
//! Includes layout detection (full vs compact) and sign detection for Qval, K_back.

use pentacam_types::PrintoutResult;

/// Archetype coordinates for 4-Maps printout at 300 DPI (Group A reference).
/// Format: (field_name, label_cy, value_cx)
pub const ARCHETYPE_4MAPS: &[(&str, f32, f32)] = &[
    // TODO: populate from Python pentacam_ocr_v7.py archetype data
];

/// Extract all fields from a 4-Maps Refractive or Selectable page.
pub fn extract(_page_img: &image::DynamicImage, _ocr_items: &[super::ocr_engine::OcrItem]) -> Result<PrintoutResult, String> {
    todo!("Label matching, affine fit, field reading, digit confusion passes, quality gate")
}
