//! Label-based field matching — Phase 1 of find_labeled_values().
//!
//! Port of pentacam_ocr_v7.py find_labeled_values() Phase 1:
//! For each OCR item, match label text against known patterns within
//! y-zone ranges, then look right on the same row for the value.

use super::field_locate::{extract_numeric, LocatedField};
use super::ocr_engine::OcrItem;
use std::collections::HashMap;

/// Y-range for a zone on the page.
type YRange = (f32, f32);

/// Fields that carry (main, x, y) coordinates on the same row.
const PACHY_COORD_FIELDS: &[&str] = &["PupilCenter", "PachyVertex", "Thinnest", "Kmax"];

/// Ambiguous back-surface fields — skipped in Phase 1, handled in Phase 2
/// (closest-to-affine-prediction selection).
const AMBIG_FIELDS: &[&str] = &["K1_back", "K2_back", "Km_back", "Astig_back"];

/// Bottom-panel fields that need the scale-bar guard (cx_cap=700) on compact devices.
const BOTTOM_FIELD_NAMES: &[&str] = &[
    "Kmax", "PupilCenter", "PachyVertex", "Thinnest",
    "CorneaVol", "HWTW", "ChamberVol", "Angle", "AC_depth", "PupilDia",
    "KPD",
];

/// Run Phase 1 label matching on sorted OCR items.
///
/// Returns a map of field_name -> LocatedField for all fields matched by label proximity.
/// Ambiguous back-surface fields (K1/K2/Km/Astig_back) are skipped — handle in Phase 2.
pub fn match_labels(items: &[OcrItem], printout_type_is_topo: bool) -> HashMap<String, LocatedField> {
    // Sort items by (rounded cy, cx) — same as Python
    let mut items_sorted: Vec<&OcrItem> = items.iter().collect();
    items_sorted.sort_by(|a, b| {
        let ay = (a.cy / 20.0).round() * 20.0;
        let by = (b.cy / 20.0).round() * 20.0;
        ay.partial_cmp(&by).unwrap().then(a.cx.partial_cmp(&b.cx).unwrap())
    });

    // Detect zone boundaries from section headers
    let (front_y, back_y, _tnp_header_y) = detect_zones(&items_sorted, printout_type_is_topo);
    let bottom_y: YRange = (1500.0, 2400.0);

    // Build label→field map
    let label_map = build_label_map_owned(front_y, back_y, bottom_y);

    let mut labeled: HashMap<String, LocatedField> = HashMap::new();

    for item in &items_sorted {
        let key_lower = item.text.to_lowercase();
        let key_lower = key_lower.trim_end_matches(':').trim();

        // Try exact match, then startswith
        let matched_field = label_map.iter()
            .find(|&&(lbl, yr, _)| key_lower == lbl && in_range(item.cy, yr))
            .or_else(|| label_map.iter()
                .find(|&&(lbl, yr, _)| key_lower.starts_with(lbl) && in_range(item.cy, yr)))
            .map(|&(_, _, field)| field);

        let matched_field = match matched_field {
            Some(f) => f,
            None => continue,
        };

        // Skip ambiguous fields — Phase 2 handles them
        if AMBIG_FIELDS.contains(&matched_field) {
            continue;
        }

        // Skip if already found
        if labeled.contains_key(matched_field) {
            continue;
        }

        let is_pachy_coord = PACHY_COORD_FIELDS.contains(&matched_field);
        let is_bottom = BOTTOM_FIELD_NAMES.contains(&matched_field);
        let max_dx: f32 = if is_pachy_coord { 800.0 } else { 500.0 };
        let main_cx_cap: f32 = if item.cy > 1490.0 && item.cy <= 2000.0 && is_bottom {
            700.0
        } else {
            1100.0
        };
        let search_cx_cap: f32 = if is_pachy_coord { 1100.0 } else { main_cx_cap };

        // Find value tokens to the right on same row
        let mut same_row: Vec<&OcrItem> = items_sorted.iter()
            .filter(|i| {
                (i.cy - item.cy).abs() < 25.0
                    && i.cx > item.cx
                    && i.cx - item.cx < max_dx
                    && i.cx < search_cx_cap
            })
            .copied()
            .collect();
        same_row.sort_by(|a, b| a.cx.partial_cmp(&b.cx).unwrap());

        let mut n_extracted = 0u32;
        for row_item in &same_row {
            let val = match extract_numeric(&row_item.text) {
                Some(v) => v,
                None => continue,
            };

            if n_extracted == 0 {
                // Main value — apply strict cap
                if row_item.cx >= main_cx_cap {
                    continue;
                }
                // Skip 0.0 as main value for pachy fields (OCR artifact)
                if is_pachy_coord && val == 0.0 {
                    continue;
                }
                labeled.insert(matched_field.to_string(), LocatedField {
                    value: val,
                    conf: row_item.confidence,
                    cx: row_item.cx,
                    cy: row_item.cy,
                    raw_text: row_item.text.clone(),
                });
                n_extracted += 1;
                if !is_pachy_coord {
                    break;
                }
            } else if n_extracted == 1 {
                let x_field = format!("{}_x", matched_field);
                if !labeled.contains_key(&x_field) {
                    labeled.insert(x_field, LocatedField {
                        value: val,
                        conf: row_item.confidence,
                        cx: row_item.cx,
                        cy: row_item.cy,
                        raw_text: row_item.text.clone(),
                    });
                }
                n_extracted += 1;
            } else if n_extracted == 2 {
                let y_field = format!("{}_y", matched_field);
                if !labeled.contains_key(&y_field) {
                    labeled.insert(y_field, LocatedField {
                        value: val,
                        conf: row_item.confidence,
                        cx: row_item.cx,
                        cy: row_item.cy,
                        raw_text: row_item.text.clone(),
                    });
                }
                break;
            }
        }
    }

    labeled
}

/// Detect "Cornea Front" / "Cornea Back" / "True Net Power" headers to set zone boundaries.
fn detect_zones(items: &[&OcrItem], is_topo: bool) -> (YRange, YRange, Option<f32>) {
    let default_front: YRange = (700.0, 1180.0);
    let default_back: YRange = (1200.0, 1720.0);

    let mut front_header_y: Option<f32> = None;
    let mut back_header_y: Option<f32> = None;
    let mut tnp_header_y: Option<f32> = None;

    for item in items {
        let tl = item.text.to_lowercase();
        if tl.contains("cornea front") && item.cy > 400.0 {
            front_header_y = Some(item.cy);
        } else if tl.contains("cornea back") && item.cy > 400.0 {
            back_header_y = Some(item.cy);
        } else if tl.contains("true net power") && item.cy > 400.0 {
            tnp_header_y = Some(item.cy);
        }
    }

    let (mut front_y, mut back_y) = (default_front, default_back);

    if let (Some(fh), Some(bh)) = (front_header_y, back_header_y) {
        if bh > fh && bh < default_back.0 {
            front_y = ((fh - 100.0).max(200.0), bh - 5.0);
            back_y = (bh + 5.0, default_back.1);
        }
    }

    // Cap BACK_Y at TNP header on Topometric pages
    if let Some(tnp) = tnp_header_y {
        if back_y.0 < tnp && tnp < back_y.1 {
            back_y = (back_y.0, tnp - 10.0);
        }
    }

    (front_y, back_y, tnp_header_y)
}

fn in_range(y: f32, range: YRange) -> bool {
    y >= range.0 && y <= range.1
}

/// Label mapping: (label_text, y_range, field_name)
pub fn build_label_map_owned(front_y: YRange, back_y: YRange, bottom_y: YRange)
    -> Vec<(&'static str, YRange, &'static str)>
{
    vec![
        // Cornea front
        ("rf",     front_y, "Rf_front"),
        ("rs",     front_y, "Rs_front"),
        ("rm",     front_y, "Rm_front"),
        ("k1",     front_y, "K1_front"),
        ("ki",     front_y, "K1_front"),   // OCR: K1→Ki
        ("k2",     front_y, "K2_front"),
        ("kz",     front_y, "K2_front"),   // OCR: K2→Kz
        ("km",     front_y, "Km_front"),
        ("astig",  front_y, "Astig_front"),
        ("rper",   front_y, "Rper_front"),
        ("rmin",   front_y, "Rmin_front"),
        // Cornea back
        ("rf",     back_y,  "Rf_back"),
        ("rs",     back_y,  "Rs_back"),
        ("rm",     back_y,  "Rm_back"),
        ("k1",     back_y,  "K1_back"),
        ("ki",     back_y,  "K1_back"),
        ("k2",     back_y,  "K2_back"),
        ("kz",     back_y,  "K2_back"),
        ("km",     back_y,  "Km_back"),
        ("astig",  back_y,  "Astig_back"),
        ("rper",   back_y,  "Rper_back"),
        ("rmin",   back_y,  "Rmin_back"),
        // Q-val
        ("q-val",  front_y, "Qval_front"),
        ("q-val",  back_y,  "Qval_back"),
        // Bottom panel
        ("k max (front)",  bottom_y, "Kmax"),
        ("k max. (front)", bottom_y, "Kmax"),
        ("k max (front}",  bottom_y, "Kmax"),
        ("pupil center",   bottom_y, "PupilCenter"),
        ("pachy vertex",   bottom_y, "PachyVertex"),
        ("pachy apex",     bottom_y, "PachyVertex"),
        ("thinnest",       bottom_y, "Thinnest"),
        ("cornea volume",  bottom_y, "CorneaVol"),
        ("chamber volume", bottom_y, "ChamberVol"),
        ("hwtw",           bottom_y, "HWTW"),
        ("kpd",            bottom_y, "KPD"),
        ("angle",          bottom_y, "Angle"),
        ("pupil dia",      bottom_y, "PupilDia"),
        ("a. c. depth",    bottom_y, "AC_depth"),
        ("a.c. depth",     bottom_y, "AC_depth"),
        ("a c. depth",     bottom_y, "AC_depth"),
    ]
}
