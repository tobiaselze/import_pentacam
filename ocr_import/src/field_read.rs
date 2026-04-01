//! Read individual field values from image crops with preprocessing.
//!
//! Port of pentacam_ocr_v7.py: whiten_colored_backgrounds, fill_hollow_digits,
//! get_tight_crop, paddle_read_crop.

use image::{DynamicImage, GenericImageView, ImageBuffer, Luma, Rgb, RgbImage};

/// Right boundary of the numerical data panel. Value boxes live left of this;
/// the 4 topography maps start further right and must not be whitened.
const VALUE_PANEL_X2: u32 = 600;

/// Upscale factor applied before OCR reads a crop.
const CROP_UPSCALE: u32 = 3;

// ---------------------------------------------------------------------------
// whiten_colored_backgrounds
// ---------------------------------------------------------------------------

/// Normalize colored pixels in the left data panel to black or white.
///
/// Two cases:
/// 1. Colored backgrounds (highlighted cells): high saturation + bright → white
/// 2. Colored text strokes (LCD-style outlines): high saturation + dark → black
pub fn whiten_colored_backgrounds(img: &mut RgbImage) {
    let panel_x2 = VALUE_PANEL_X2.min(img.width());

    for y in 0..img.height() {
        for x in 0..panel_x2 {
            let px = img.get_pixel(x, y);
            let r = px[0] as f32;
            let g = px[1] as f32;
            let b = px[2] as f32;

            let max_c = r.max(g).max(b);
            let min_c = r.min(g).min(b);
            let saturation = if max_c > 10.0 { (max_c - min_c) / max_c } else { 0.0 };

            // Case 1: colored background → white
            if saturation > 0.25 && min_c > 60.0 {
                img.put_pixel(x, y, Rgb([255, 255, 255]));
            }
            // Case 2: colored text stroke → black
            else if saturation > 0.40 && max_c > 60.0 && min_c <= 100.0 {
                img.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// fill_hollow_digits
// ---------------------------------------------------------------------------

/// Morphological closing on dark pixels to fill hollow outlined digits.
///
/// Some Pentacam cells render values as outlined characters (colored stroke
/// with white fill). After whitening, the interior is white, giving OCR
/// hollow outlines it can't decode.
///
/// Closing = dilate dark (MinFilter) then erode (MaxFilter).
/// Only applied when dark pixel density < 10% (hollow indicator).
pub fn fill_hollow_digits(img: &DynamicImage, n_dilate: u32) -> DynamicImage {
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();

    // Check dark pixel density — skip if already solid text
    let dark_count = gray.pixels().filter(|p| p[0] < 80).count();
    let density = dark_count as f32 / (w * h) as f32;
    if density > 0.10 {
        return img.clone();
    }

    // Dilate dark pixels (min filter 3x3)
    let mut current = gray;
    for _ in 0..n_dilate {
        current = min_filter_3x3(&current);
    }
    // Erode back (max filter 3x3)
    for _ in 0..n_dilate {
        current = max_filter_3x3(&current);
    }

    // Merge: only blacken pixels that became dark during closing
    let original_gray = img.to_luma8();
    let mut rgb = img.to_rgb8();
    for y in 0..h {
        for x in 0..w {
            let orig = original_gray.get_pixel(x, y)[0];
            let closed = current.get_pixel(x, y)[0];
            if closed < 80 && orig >= 80 {
                rgb.put_pixel(x, y, Rgb([0, 0, 0]));
            }
        }
    }

    DynamicImage::ImageRgb8(rgb)
}

/// 3x3 min filter (dilates dark pixels).
fn min_filter_3x3(img: &ImageBuffer<Luma<u8>, Vec<u8>>) -> ImageBuffer<Luma<u8>, Vec<u8>> {
    let (w, h) = img.dimensions();
    let mut out = ImageBuffer::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut min_val = 255u8;
            for dy in 0..3i32 {
                for dx in 0..3i32 {
                    let nx = (x as i32 + dx - 1).clamp(0, w as i32 - 1) as u32;
                    let ny = (y as i32 + dy - 1).clamp(0, h as i32 - 1) as u32;
                    min_val = min_val.min(img.get_pixel(nx, ny)[0]);
                }
            }
            out.put_pixel(x, y, Luma([min_val]));
        }
    }
    out
}

/// 3x3 max filter (erodes dark pixels / dilates light).
fn max_filter_3x3(img: &ImageBuffer<Luma<u8>, Vec<u8>>) -> ImageBuffer<Luma<u8>, Vec<u8>> {
    let (w, h) = img.dimensions();
    let mut out = ImageBuffer::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut max_val = 0u8;
            for dy in 0..3i32 {
                for dx in 0..3i32 {
                    let nx = (x as i32 + dx - 1).clamp(0, w as i32 - 1) as u32;
                    let ny = (y as i32 + dy - 1).clamp(0, h as i32 - 1) as u32;
                    max_val = max_val.max(img.get_pixel(nx, ny)[0]);
                }
            }
            out.put_pixel(x, y, Luma([max_val]));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// get_tight_crop
// ---------------------------------------------------------------------------

/// Bounding box for a detected OCR region.
pub struct OcrBox {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

/// Extract a tight crop around a field value.
///
/// Snaps to the nearest OCR bounding box within 60px, then adds padding.
/// Falls back to a fixed-size crop if no nearby box found.
pub fn get_tight_crop(
    img: &DynamicImage,
    cx: f32,
    cy: f32,
    ocr_boxes: &[OcrBox],
    pad: u32,
) -> DynamicImage {
    let mut best_box: Option<(u32, u32, u32, u32)> = None;
    let mut best_dist = f32::MAX;

    for b in ocr_boxes {
        let bcx = (b.x1 + b.x2) / 2.0;
        let bcy = (b.y1 + b.y2) / 2.0;
        let dist = (bcx - cx).abs() + (bcy - cy).abs();
        if dist < best_dist {
            best_dist = dist;
            best_box = Some((b.x1 as u32, b.y1 as u32, b.x2 as u32, b.y2 as u32));
        }
    }

    let (x1, y1, x2, y2) = if best_dist < 60.0 {
        best_box.unwrap()
    } else {
        (
            (cx as u32).saturating_sub(100),
            (cy as u32).saturating_sub(35),
            (cx as u32) + 100,
            (cy as u32) + 35,
        )
    };

    let (iw, ih) = img.dimensions();
    img.crop_imm(
        x1.saturating_sub(pad),
        y1.saturating_sub(pad),
        (x2 + pad).min(iw) - x1.saturating_sub(pad),
        (y2 + pad).min(ih) - y1.saturating_sub(pad),
    )
}

// ---------------------------------------------------------------------------
// Crop reading pipeline
// ---------------------------------------------------------------------------

/// Preprocess a crop for OCR: upscale 3x, fill hollow digits.
pub fn preprocess_crop(crop: &DynamicImage) -> DynamicImage {
    let (w, h) = crop.dimensions();
    let big = crop.resize_exact(
        w * CROP_UPSCALE,
        h * CROP_UPSCALE,
        image::imageops::FilterType::Lanczos3,
    );
    fill_hollow_digits(&big, 4)
}

/// Extract a numeric value from OCR text, handling signs, decimals, etc.
/// Same as field_locate::extract_numeric but also filters for best candidate.
pub fn extract_best_numeric(items: &[super::ocr_engine::OcrItem]) -> Option<(f64, f32)> {
    let mut best_val: Option<f64> = None;
    let mut best_score: f64 = -1.0;
    let mut best_conf: f32 = 0.0;

    for item in items {
        let stripped = item.text.trim().trim_start_matches(&['(', '['][..]);
        if stripped.is_empty() { continue; }
        let first = match stripped.chars().next() {
            Some(c) => c,
            None => continue,
        };
        if !first.is_ascii_digit() && first != '+' && first != '-' { continue; }
        if stripped.ends_with(':') { continue; }

        if let Some(val) = super::field_locate::extract_numeric(&item.text) {
            let score = item.confidence as f64 * stripped.len().max(3) as f64;
            if score > best_score {
                best_val = Some(val);
                best_score = score;
                best_conf = item.confidence;
            }
        }
    }

    best_val.map(|v| (v, best_conf))
}
