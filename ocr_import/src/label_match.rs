//! Label-based field matching — Phase 1 of find_labeled_values().
//!
//! Port of pentacam_ocr_v7.py find_labeled_values() Phase 1:
//! For each OCR item, match label text against known patterns within
//! y-zone ranges, then look right on the same row for the value.

use super::field_locate::{self, extract_numeric, LocatedField};
use super::ocr_engine::OcrItem;
use regex::Regex;
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
    let bottom_y: YRange = (1500.0, 2700.0);

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

    // ── Phase 2: Ambiguous back-surface fields ──────────────────────────
    // K1/K2/Km/Astig labels appear in BOTH "Cornea Back" and "True Net Power"
    // sections on Topometric pages. After the affine fit, pick the candidate
    // closest to the affine-predicted position.
    let archetype = field_locate::archetype_for_type(printout_type_is_topo);
    let fit = field_locate::fit_affine(&labeled, archetype);

    let ambig_fields: &[(&str, &[&str])] = &[
        ("K1_back",    &["k1", "ki", "kl"]),
        ("K2_back",    &["k2", "kz"]),
        ("Km_back",    &["km"]),
        ("Astig_back", &["astig"]),
    ];

    for &(field_name, label_keys) in ambig_fields {
        if labeled.contains_key(field_name) {
            continue;
        }
        let arch_entry = archetype.iter().find(|&&(n, _, _)| n == field_name);
        let (cy_ref, _cx_ref) = match arch_entry {
            Some(&(_, cy, cx)) => (cy, cx),
            None => continue,
        };
        let cy_pred = fit.alpha * cy_ref as f64 + fit.beta;

        let mut best: Option<(f64, LocatedField)> = None;

        for item in &items_sorted {
            let k = item.text.to_lowercase();
            let k = k.trim_end_matches(':').trim();
            if !label_keys.iter().any(|&lk| k == lk || k.starts_with(lk)) {
                continue;
            }
            // Only consider labels in the left portion of the page (cx < 500)
            // to avoid matching annotations/scale text
            if item.cx > 500.0 { continue; }

            // Find first numeric token to the right on same row
            for item2 in &items_sorted {
                if (item2.cy - item.cy).abs() >= 25.0 { continue; }
                if item2.cx <= item.cx { continue; }
                if item2.cx - item.cx >= 500.0 { continue; }
                if item2.cx >= 1100.0 { continue; }
                let val = match extract_numeric(&item2.text) {
                    Some(v) => v,
                    None => continue,
                };
                let dist = (item.cy as f64 - cy_pred).abs();
                let is_better = match &best {
                    Some((best_dist, _)) => dist < *best_dist,
                    None => true,
                };
                if is_better {
                    best = Some((dist, LocatedField {
                        value: val,
                        conf: item2.confidence,
                        cx: item2.cx,
                        cy: item.cy,
                        raw_text: item2.text.clone(),
                    }));
                }
                break; // first numeric per label candidate
            }
        }

        if let Some((_, field)) = best {
            labeled.insert(field_name.to_string(), field);
        }
    }

    // ── Phase 3: Affine positional fallback (with token merging) ────────
    // For fields still missing, use the affine-predicted position and search
    // within a tight window. Merge adjacent tokens left-to-right so split
    // numbers ('38' + '.1') reassemble into '38 .1' → 38.1.
    let win_y: f32 = 35.0;
    let win_x: f32 = 65.0;

    for &(field_name, cy_ref, cx_ref) in archetype {
        if labeled.contains_key(field_name) {
            continue;
        }
        let cy_pred = (fit.alpha * cy_ref as f64 + fit.beta) as f32;
        let cx_pred = (fit.alpha_cx * cx_ref as f64 + fit.beta_cx) as f32;

        // Collect all tokens in the window, sorted left-to-right
        let mut candidates: Vec<&OcrItem> = items_sorted.iter()
            .filter(|item| {
                (item.cy - cy_pred).abs() <= win_y
                    && (item.cx - cx_pred).abs() <= win_x
            })
            .copied()
            .collect();
        candidates.sort_by(|a, b| a.cx.partial_cmp(&b.cx).unwrap());

        if candidates.is_empty() { continue; }

        // Merge all token texts left-to-right
        let merged: String = candidates.iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let best_conf = candidates.iter()
            .map(|c| c.confidence)
            .fold(0.0f32, f32::max);
        let cx0 = candidates[0].cx;
        let cy0 = candidates[0].cy;

        if let Some(val) = extract_numeric(&merged) {
            labeled.insert(field_name.to_string(), LocatedField {
                value: val,
                conf: best_conf,
                cx: cx0,
                cy: cy0,
                raw_text: merged,
            });
        }
    }

    // ── Axis extraction from "(steep)" / "(flat)" tokens ─────────────
    // Runs AFTER Phase 3 so it can fill in Axis fields that the numeric
    // fallback missed (because the token starts with "steep"/"flat").
    let (front_y, back_y, _) = detect_zones(&items_sorted, printout_type_is_topo);
    let steep_re = Regex::new(r"(?i)(?:steep|flat)[^0-9-]*(-?[0-9]+\.?[0-9]*)").unwrap();
    let digits_re = Regex::new(r"^([0-9]+)").unwrap();

    for item in &items_sorted {
        let text_lower = item.text.to_lowercase();

        // Pattern 1: "(steep) 92.2" or "(flat)88.5" — number embedded in same token
        if let Some(caps) = steep_re.captures(&item.text) {
            let mut val_str = caps[1].to_string();
            // Handle truncated decimal: "97." → look right for continuation digits
            if val_str.ends_with('.') {
                if let Some(next) = items_sorted.iter()
                    .filter(|i| (i.cy - item.cy).abs() < 25.0 && i.cx > item.cx && i.cx - item.cx < 150.0)
                    .min_by(|a, b| a.cx.partial_cmp(&b.cx).unwrap())
                {
                    if let Some(dm) = digits_re.captures(next.text.trim()) {
                        val_str.push_str(&dm[1]);
                    }
                }
            }
            if let Ok(val) = val_str.parse::<f64>() {
                if in_range(item.cy, front_y) && !labeled.contains_key("Axis_front") {
                    labeled.insert("Axis_front".to_string(), LocatedField {
                        value: val, conf: item.confidence, cx: item.cx, cy: item.cy,
                        raw_text: item.text.clone(),
                    });
                } else if in_range(item.cy, back_y) && !labeled.contains_key("Axis_back") {
                    labeled.insert("Axis_back".to_string(), LocatedField {
                        value: val, conf: item.confidence, cx: item.cx, cy: item.cy,
                        raw_text: item.text.clone(),
                    });
                }
            }
            continue;
        }

        // Pattern 2: "(steep)" alone — number is in a separate token to the right
        if text_lower.contains("steep") || text_lower.contains("flat") {
            let mut right: Vec<&OcrItem> = items_sorted.iter()
                .filter(|i| (i.cy - item.cy).abs() < 40.0 && i.cx > item.cx && i.cx - item.cx < 300.0)
                .copied()
                .collect();
            right.sort_by(|a, b| a.cx.partial_cmp(&b.cx).unwrap());

            for r_item in &right {
                if let Some(val) = extract_numeric(&r_item.text) {
                    if val > 0.0 && val < 180.0 {
                        if in_range(item.cy, front_y) && !labeled.contains_key("Axis_front") {
                            labeled.insert("Axis_front".to_string(), LocatedField {
                                value: val, conf: r_item.confidence, cx: r_item.cx, cy: r_item.cy,
                                raw_text: format!("{} {}", item.text.trim(), r_item.text),
                            });
                        } else if in_range(item.cy, back_y) && !labeled.contains_key("Axis_back") {
                            labeled.insert("Axis_back".to_string(), LocatedField {
                                value: val, conf: r_item.confidence, cx: r_item.cx, cy: r_item.cy,
                                raw_text: format!("{} {}", item.text.trim(), r_item.text),
                            });
                        }
                        break;
                    }
                }
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
