//! Wrapper around oar-ocr for full-page OCR and field-crop reading.

/// A detected text region: text content, confidence, and centroid position.
pub struct OcrItem {
    pub text: String,
    pub confidence: f32,
    pub cx: f32,
    pub cy: f32,
}

/// Run full-page OCR on a rendered page image.
/// Returns all detected text regions with positions.
pub fn run_full_page(_img_path: &std::path::Path) -> Result<Vec<OcrItem>, String> {
    todo!("Initialize oar-ocr (lazy singleton), run predict, extract items")
}

/// Run OCR on a small crop image (e.g. a single field value).
/// Upscales, applies preprocessing (fill_hollow_digits), then reads.
/// Returns (numeric_value, confidence) or None.
pub fn read_crop(_crop: &image::DynamicImage) -> Option<(f64, f32)> {
    todo!("Upscale 3x, preprocess, run oar-ocr predict, extract_numeric")
}
