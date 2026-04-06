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
    let wf = w as f32;
    let hf = h as f32;

    // ── Find all anchors (matching Python exactly) ───────────────────────
    let re_elev_f = Regex::new(r"(?i)^Elevation\s*[\(\[]?\s*Front\s*[\)\]]?").unwrap();
    let re_elev_b = Regex::new(r"(?i)^Elevation\s*[\(\[]?\s*Back\s*[\)\]]?").unwrap();
    let re_fixed = Regex::new(r"(?i)^Fixed$").unwrap();
    let re_ref = Regex::new(r"(?i)^Reference\s*Database").unwrap();
    let re_footer = Regex::new(r"(?i)Oculus|Münch|Arlington").unwrap();
    let re_mean = Regex::new(r"(?i)Mean corneal").unwrap();
    let re_pachy = Regex::new(r"(?i)^Pachy\.?$").unwrap();
    let re_abs = Regex::new(r"(?i)^Abs\.?$").unwrap();
    let re_ctsp = Regex::new(r"(?i)Corneal Thickness Spatial Profile|\bCTSP\b").unwrap();
    let re_pti = Regex::new(r"(?i)Percentage Thickness Increase|\bPTI\b").unwrap();
    let re_diameter = Regex::new(r"(?i)^Diameter$").unwrap();

    let ef = find_anchor(items, &re_elev_f, Some(hf * 0.25));
    let eb = find_anchor(items, &re_elev_b, Some(hf * 0.25));
    let fixed = find_label_below(items, &re_fixed, hf * 0.7);
    let ref_db = find_label_below(items, &re_ref, hf * 0.85);
    let mean_label = find_anchor(items, &re_mean, Some(hf * 0.55));

    // Pachy/Abs color bar labels — far right only (cx > 80% of page width)
    let mut pachy_label: Option<(f32, f32)> = None;
    let mut abs_label: Option<(f32, f32)> = None;
    for item in items {
        if item.cx > wf * 0.8 {
            if pachy_label.is_none() && re_pachy.is_match(item.text.trim()) {
                pachy_label = Some((item.cx, item.cy));
            }
            if abs_label.is_none() && re_abs.is_match(item.text.trim()) {
                abs_label = Some((item.cx, item.cy));
            }
        }
    }

    // CTSP / PTI labels
    let ctsp = find_anchor(items, &re_ctsp, Some(hf * 0.6));
    let pti = find_anchor(items, &re_pti, Some(hf * 0.8));

    let top_anchor = ef.or(eb);
    if top_anchor.is_none() { return result; }
    let (_ta_cx, ta_cy) = top_anchor.unwrap();

    let rgb = img.to_rgb8();

    // ── LEFT PANEL (3×2 elevation/difference maps) ───────────────────────
    let left_top = (ta_cy - 25.0).max(0.0) as u32;

    // Bottom: "Fixed" label + 70px, but capped by footer detection
    let left_bottom = if let Some((_, fy)) = fixed {
        let mut bottom = (fy + 70.0).min(hf - 10.0);
        // Detect footer to avoid including it
        for item in items {
            if re_footer.is_match(item.text.trim()) && item.cy > fy {
                bottom = bottom.min(item.cy - 10.0);
            }
        }
        bottom as u32
    } else {
        (hf * 0.96).min(hf - 10.0) as u32
    };

    // Left edge: pixel scan at mid-height of top map row (left half only)
    let scan_y_left = (ta_cy + 300.0) as u32;
    let left_left = if scan_y_left < h {
        scan_left_edge(&rgb, scan_y_left, 0, w / 2)
    } else {
        0
    };

    // Right edge: geometric — midpoint between Elevation Front and Back headers
    // Each panel extends half_gap from its header. Right = eb_cx + half_gap.
    let left_right = if let (Some((ef_cx, _)), Some((eb_cx, _))) = (ef, eb) {
        let half_gap = (eb_cx - ef_cx) / 2.0;
        (eb_cx + half_gap) as u32
    } else if let Some((eb_cx, _)) = eb {
        (eb_cx + 400.0) as u32
    } else if let Some((ef_cx, _)) = ef {
        (ef_cx + 1200.0) as u32
    } else {
        (wf * 0.52) as u32
    };

    if left_right > left_left && left_bottom > left_top {
        result.insert(
            "belin_left".to_string(),
            img.crop_imm(left_left, left_top, left_right - left_left, left_bottom - left_top),
        );
    }

    // ── THICKNESS MAP (top right) ────────────────────────────────────────
    // Find "Corneal Thickness" header — must be in RIGHT HALF, top quarter
    let re_ct = Regex::new(r"(?i)Corneal\s*Thickness").unwrap();
    let re_corneal = Regex::new(r"(?i)^Corneal$").unwrap();
    let re_thickness_word = Regex::new(r"(?i)^Thickness$").unwrap();

    let mut ct_header: Option<(f32, f32)> = None;
    for item in items {
        if re_ct.is_match(item.text.trim()) && item.cx > wf * 0.6 && item.cy < hf * 0.25 {
            ct_header = Some((item.cx, item.cy));
            break;
        }
    }
    // Try split tokens: "Corneal" + nearby "Thickness"
    if ct_header.is_none() {
        'outer: for i1 in items {
            if re_corneal.is_match(i1.text.trim()) && i1.cx > wf * 0.6 && i1.cy < hf * 0.25 {
                for i2 in items {
                    if re_thickness_word.is_match(i2.text.trim())
                        && (i2.cy - i1.cy).abs() < 15.0
                        && (i2.cx - i1.cx).abs() < 300.0
                    {
                        ct_header = Some(((i1.cx + i2.cx) / 2.0, i1.cy));
                        break 'outer;
                    }
                }
            }
        }
    }

    let thick_top = if let Some((_, cty)) = ct_header {
        (cty - 25.0).max(0.0) as u32
    } else {
        left_top
    };

    // Bottom: "Abs." label + 40px, or "Pachy." + 100px
    let thick_bottom = if let Some((_, ay)) = abs_label {
        (ay + 40.0).min(hf - 10.0) as u32
    } else if let Some((_, py)) = pachy_label {
        (py + 100.0).min(hf - 10.0) as u32
    } else {
        (ta_cy + 830.0) as u32
    };

    // Right edge: rightmost non-white pixel in right portion
    let scan_y_thick = ((thick_top + thick_bottom) / 2) as u32;
    let thick_right = if scan_y_thick < h {
        scan_right_edge(&rgb, scan_y_thick, w / 2, w)
    } else {
        w
    };

    // Left edge: use Abs. label to measure color bar width, then subtract
    let thick_left = if let (Some((ct_cx, _)), Some((abs_cx, _))) = (ct_header, abs_label) {
        let color_bar_w = 2.0 * (thick_right as f32 - abs_cx);
        let map_half_w = (thick_right as f32 - ct_cx) - color_bar_w;
        (ct_cx - map_half_w).max(0.0) as u32
    } else if let Some((ct_cx, _)) = ct_header {
        (2.0 * ct_cx - thick_right as f32).max(0.0) as u32
    } else if let Some((px, _)) = pachy_label {
        (px - 550.0).max(0.0) as u32
    } else {
        (wf * 0.7) as u32
    };

    if thick_right > thick_left && thick_bottom > thick_top {
        result.insert(
            "belin_thickness".to_string(),
            img.crop_imm(thick_left, thick_top, thick_right - thick_left, thick_bottom - thick_top),
        );
    }

    // ── CHARTS (CTSP + PTI + BAD-D) ─────────────────────────────────────
    let (chart_top, chart_bottom, chart_left, chart_right);

    if let (Some((ctsp_cx, ctsp_cy)), Some((pti_cx, pti_cy))) = (ctsp, pti) {
        let panel_height = pti_cy - ctsp_cy;

        // Top: include "Mean corneal..." title if present above CTSP
        chart_top = if let Some((_, my)) = mean_label {
            if my < ctsp_cy { (my - 10.0).max(0.0) as u32 }
            else { (ctsp_cy - 15.0).max(0.0) as u32 }
        } else {
            (ctsp_cy - 15.0).max(0.0) as u32
        };

        // Bottom: Reference Database + 60px, or one panel_height below PTI
        let mut bot = if let Some((_, ry)) = ref_db {
            (ry + 60.0).min(hf - 10.0)
        } else {
            (pti_cy + panel_height + 60.0).min(hf - 10.0)
        };
        // Detect footer to avoid including it
        for item in items {
            if re_footer.is_match(item.text.trim()) && item.cy > pti_cy {
                bot = bot.min(item.cy - 10.0);
            }
        }
        chart_bottom = bot as u32;

        // Left: find y-axis number labels (e.g. "400", "500", "µm", "%")
        let re_yaxis = Regex::new(r"(?i)^\d{2,3}$|^[µμu]m$|^%$").unwrap();
        let mut yaxis_cx_values: Vec<f32> = Vec::new();
        for item in items {
            if re_yaxis.is_match(item.text.trim())
                && item.cy > ctsp_cy && item.cy < ctsp_cy + panel_height
                && item.cx < ctsp_cx && item.cx > ctsp_cx - 400.0
            {
                yaxis_cx_values.push(item.cx);
            }
        }

        chart_left = if !yaxis_cx_values.is_empty() {
            let min_cx = yaxis_cx_values.iter().cloned().fold(f32::MAX, f32::min);
            (min_cx - 60.0).max(0.0) as u32
        } else {
            (ctsp_cx.min(pti_cx) - 400.0).max(0.0) as u32
        };

        // Right: "Diameter" label cx + 80px
        let mut diameter_cx: Option<f32> = None;
        for item in items {
            if re_diameter.is_match(item.text.trim()) {
                if (item.cy - ctsp_cy).abs() < 20.0 || (item.cy - pti_cy).abs() < 20.0 {
                    diameter_cx = Some(item.cx);
                    break;
                }
            }
        }
        chart_right = if let Some(dx) = diameter_cx {
            (dx + 80.0).min(wf) as u32
        } else {
            thick_right
        };
    } else {
        // Fallback when CTSP/PTI not found
        chart_top = thick_bottom + 10;
        chart_bottom = if let Some((_, ry)) = ref_db {
            (ry + 60.0).min(hf - 10.0) as u32
        } else {
            (hf * 0.95) as u32
        };
        chart_left = thick_left;
        chart_right = thick_right;
    };

    if chart_right > chart_left && chart_bottom > chart_top {
        result.insert(
            "belin_charts".to_string(),
            img.crop_imm(chart_left, chart_top, chart_right - chart_left, chart_bottom - chart_top),
        );
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

/// Scan rightward to find the leftmost non-white pixel.
fn scan_left_edge(rgb: &RgbImage, y: u32, x_start: u32, x_end: u32) -> u32 {
    let (w, h) = rgb.dimensions();
    if y >= h { return x_start; }
    for x in x_start..x_end.min(w) {
        let p = rgb.get_pixel(x, y);
        if p[0].min(p[1]).min(p[2]) < 240 {
            return x.saturating_sub(5);
        }
    }
    x_start
}

/// Scan leftward from right to find the rightmost non-white pixel.
fn scan_right_edge(rgb: &RgbImage, y: u32, x_start: u32, x_end: u32) -> u32 {
    let (w, h) = rgb.dimensions();
    if y >= h { return x_end; }
    let end = x_end.min(w);
    for x in (x_start..end).rev() {
        let p = rgb.get_pixel(x, y);
        if p[0].min(p[1]).min(p[2]) < 240 {
            return (x + 15).min(w);
        }
    }
    x_end
}
