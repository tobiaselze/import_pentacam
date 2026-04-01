//! Field location via label matching and affine archetype fitting.

use super::ocr_engine::OcrItem;
use pentacam_types::PrintoutType;
use std::collections::HashMap;

/// A located field: its centroid position on the page.
pub struct LocatedField {
    pub cx: f32,
    pub cy: f32,
    pub label_conf: f32,
    pub raw_label_text: String,
}

/// Match known field labels to OCR items, then apply adaptive affine fit
/// from the Group A reference archetype for positional fallback.
/// Returns map of field_name -> located position.
pub fn find_labeled_values(
    _items: &[OcrItem],
    _printout_type: &PrintoutType,
) -> Result<HashMap<String, LocatedField>, String> {
    todo!("Label regex matching + affine fit from archetype coordinates")
}
