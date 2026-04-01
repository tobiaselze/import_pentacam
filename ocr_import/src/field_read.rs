//! Read individual field values from image crops.

/// Preprocessing: whiten colored backgrounds for contrast enhancement.
pub fn whiten_colored_backgrounds(_img: &mut image::DynamicImage) {
    todo!()
}

/// Preprocessing: fill hollow digit glyphs (common LCD-style fonts).
pub fn fill_hollow_digits(_img: &mut image::DynamicImage) {
    todo!()
}

/// Extract a tight crop around a field value at (cx, cy).
pub fn get_tight_crop(
    _page_img: &image::DynamicImage,
    _cx: f32,
    _cy: f32,
) -> image::DynamicImage {
    todo!()
}

/// Extract a numeric value from OCR text, handling signs, decimals, etc.
pub fn extract_numeric(_text: &str) -> Option<f64> {
    todo!()
}
