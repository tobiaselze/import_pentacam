//! Map region extraction from Pentacam printout pages.
//!
//! Extracts color map images from 4-Maps Refractive and Belin Ectasia pages
//! using anchor label detection + pixel scanning. Port of extract_maps.py.

use image::{DynamicImage, GenericImageView, RgbImage};
use regex::Regex;
use std::collections::HashMap;

use super::ocr_engine::OcrItem;

/// Extracted map regions from a printout page.
pub struct ExtractedMaps {
    /// Named map regions: "4mr_all", "belin_left", "belin_thickness", "belin_charts"
    pub maps: HashMap<String, DynamicImage>,
}

/// Extract map regions from a rendered printout page.
pub fn extract_maps(
    img: &DynamicImage,
    items: &[OcrItem],
    printout_type: &str,
) -> ExtractedMaps {
    let ptype = printout_type.to_lowercase();
    let maps = if ptype.contains("fourmaps") || ptype.contains("refractive") || ptype.contains("selectable") || ptype.contains("4maps") {
        extract_4mr_maps(img, items)
    } else if ptype.contains("belin") && (ptype.contains("ectasia") || ptype.contains("ambrosio") || ptype.contains("belinambrosio")) {
        extract_belin_maps(img, items)
    } else {
        HashMap::new()
    };
    ExtractedMaps { maps }
}

// ---------------------------------------------------------------------------
// 4 Maps Refractive: 2x2 map grid
// ---------------------------------------------------------------------------

fn extract_4mr_maps(img: &DynamicImage, items: &[OcrItem]) -> HashMap<String, DynamicImage> {
    let mut result = HashMap::new();
    let (w, h) = img.dimensions();

    // Find anchor labels
    // Match any curvature header (Refractive: "Axial/Sagittal Curvature", Selectable: "Tangential Curvature" etc.)
    let re_curv = Regex::new(r"(?i)(Axial|Sagittal|Tangential)\s*Curvature.*Front").unwrap();
    let re_elev_f = Regex::new(r"(?i)Elevation.*Front").unwrap();
    let re_thick = Regex::new(r"(?i)Corneal\s*Thickness").unwrap();
    let re_elev_b = Regex::new(r"(?i)Elevation.*Back").unwrap();

    let max_cy = (h as f32 * 0.7) as f32;
    let curv = find_anchor(items, &re_curv, Some(max_cy));
    let elev_f = find_anchor(items, &re_elev_f, Some(max_cy));
    let thick = find_anchor(items, &re_thick, Some(max_cy));
    let elev_b = find_anchor(items, &re_elev_b, Some(max_cy));

    // Need at least one from each row
    let top_anchors: Vec<(f32, f32)> = [curv, elev_f].into_iter().flatten().collect();
    let bot_anchors: Vec<(f32, f32)> = [thick, elev_b].into_iter().flatten().collect();

    if top_anchors.is_empty() && bot_anchors.is_empty() {
        return result;
    }

    let top_cy = top_anchors.iter().map(|a| a.1).fold(f32::MAX, f32::min);
    let bot_cy = if !bot_anchors.is_empty() {
        bot_anchors.iter().map(|a| a.1).fold(f32::MAX, f32::min)
    } else {
        top_cy + h as f32 * 0.4
    };
    let row_gap = bot_cy - top_cy;

    // Left edge: find "Curvature" standalone label
    let re_curv_label = Regex::new(r"(?i)^Curvature$").unwrap();
    let curv_label = find_anchor(items, &re_curv_label, Some(max_cy));
    let left_edge = if let Some((cx, _)) = curv_label {
        (cx - 90.0).max(0.0) as u32
    } else {
        let left_cx = top_anchors.iter().chain(bot_anchors.iter())
            .map(|a| a.0).fold(f32::MAX, f32::min);
        (left_cx - 680.0).max(0.0) as u32
    };

    // Right edge: scan for rightmost non-white pixel in right half
    let rgb = img.to_rgb8();
    let scan_y = (top_cy + row_gap * 0.4) as u32;
    let mid_x = w / 2;
    let right_edge = if scan_y < h {
        let mut rightmost = w;
        for x in (mid_x..w).rev() {
            let p = rgb.get_pixel(x, scan_y);
            if p[0].min(p[1]).min(p[2]) < 240 {
                rightmost = (x + 15).min(w);
                break;
            }
        }
        rightmost
    } else {
        w
    };

    let top_start = (top_cy - 15.0).max(0.0) as u32;
    let bot_end = (bot_cy + row_gap - 30.0).min(h as f32 - 10.0) as u32;

    if right_edge > left_edge && bot_end > top_start {
        result.insert(
            "4mr_all".to_string(),
            img.crop_imm(left_edge, top_start, right_edge - left_edge, bot_end - top_start),
        );
    }

    result
}

// ---------------------------------------------------------------------------
// Belin Ectasia: 3x2 maps + thickness + charts
// ---------------------------------------------------------------------------

fn extract_belin_maps(img: &DynamicImage, items: &[OcrItem]) -> HashMap<String, DynamicImage> {
    let mut result = HashMap::new();
    let (w, h) = img.dimensions();

    // Find anchors
    let re_elev_f = Regex::new(r"(?i)^Elevation\s*[\(\[]?\s*Front\s*[\)\]]?").unwrap();
    let re_elev_b = Regex::new(r"(?i)^Elevation\s*[\(\[]?\s*Back\s*[\)\]]?").unwrap();
    let re_fixed = Regex::new(r"(?i)^Fixed$").unwrap();
    let re_thick = Regex::new(r"(?i)Corneal\s*Thickness").unwrap();
    let re_ref = Regex::new(r"(?i)^Reference\s*Database").unwrap();
    let re_mean = Regex::new(r"(?i)Mean corneal").unwrap();

    let ef = find_anchor(items, &re_elev_f, Some(h as f32 * 0.25));
    let _eb = find_anchor(items, &re_elev_b, Some(h as f32 * 0.25));
    let fixed = find_label_below(items, &re_fixed, h as f32 * 0.7);
    let ref_db = find_label_below(items, &re_ref, h as f32 * 0.85);
    let thick = find_anchor(items, &re_thick, Some(h as f32 * 0.3));
    let mean = find_anchor(items, &re_mean, Some(h as f32 * 0.55));

    let top_anchor = ef.or(_eb);
    if top_anchor.is_none() { return result; }
    let (ta_cx, ta_cy) = top_anchor.unwrap();

    let rgb = img.to_rgb8();

    // LEFT PANEL (3×2 elevation/difference maps)
    let left_top = (ta_cy - 25.0).max(0.0) as u32;
    let left_bottom = if let Some((_, fy)) = fixed {
        (fy + 70.0).min(h as f32 - 10.0) as u32
    } else {
        (h as f32 * 0.96) as u32
    };

    // Left edge: pixel scan
    let scan_y = (ta_cy + 300.0) as u32;
    let left_edge = if scan_y < h {
        scan_left_edge(&rgb, scan_y, 0, w / 2)
    } else {
        0
    };

    // Right edge of left panel: midpoint of page or just before thickness map
    let left_right = if let Some((tx, _)) = thick {
        (tx - 30.0) as u32
    } else {
        (w as f32 * 0.52) as u32
    };

    if left_right > left_edge && left_bottom > left_top {
        result.insert(
            "belin_left".to_string(),
            img.crop_imm(left_edge, left_top, left_right - left_edge, left_bottom - left_top),
        );
    }

    // THICKNESS MAP (right side, upper)
    if let Some((tx, ty)) = thick {
        let thick_top = (ty - 15.0).max(0.0) as u32;
        let thick_left = left_right;
        let thick_right = w.min(thick_left + (w - thick_left));
        let thick_bottom = if let Some((_, my)) = mean {
            (my - 10.0) as u32
        } else {
            (h as f32 * 0.52) as u32
        };

        if thick_right > thick_left && thick_bottom > thick_top {
            result.insert(
                "belin_thickness".to_string(),
                img.crop_imm(thick_left, thick_top, thick_right - thick_left, thick_bottom - thick_top),
            );
        }
    }

    // CHARTS (CTSP + PTI, below thickness)
    if let Some((mx, my)) = mean {
        let chart_top = (my - 10.0) as u32;
        let chart_left = left_right;
        let chart_right = w;
        let chart_bottom = if let Some((_, ry)) = ref_db {
            (ry + 25.0).min(h as f32) as u32
        } else {
            (h as f32 * 0.92) as u32
        };

        if chart_right > chart_left && chart_bottom > chart_top {
            result.insert(
                "belin_charts".to_string(),
                img.crop_imm(chart_left, chart_top, chart_right - chart_left, chart_bottom - chart_top),
            );
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_anchor(items: &[OcrItem], pattern: &Regex, max_cy: Option<f32>) -> Option<(f32, f32)> {
    for item in items {
        if let Some(max) = max_cy {
            if item.cy > max { continue; }
        }
        if pattern.is_match(item.text.trim()) {
            return Some((item.cx, item.cy));
        }
    }
    None
}

fn find_label_below(items: &[OcrItem], pattern: &Regex, min_cy: f32) -> Option<(f32, f32)> {
    for item in items {
        if item.cy <= min_cy { continue; }
        if pattern.is_match(item.text.trim()) {
            return Some((item.cx, item.cy));
        }
    }
    None
}

/// Scan leftward from a starting region to find the leftmost non-white pixel.
fn scan_left_edge(rgb: &RgbImage, y: u32, x_start: u32, x_end: u32) -> u32 {
    let (w, h) = rgb.dimensions();
    if y >= h { return x_start; }
    for x in x_start..x_end.min(w) {
        let p = rgb.get_pixel(x, y);
        if p[0].min(p[1]).min(p[2]) < 240 {
            return x.saturating_sub(10);
        }
    }
    x_start
}
