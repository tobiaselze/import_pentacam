//! Belin/Ambrosio Enhanced Ectasia printout extraction.
//!
//! Port of extract_belin_fields.py: label-based matching for 20 fields
//! in the Belin data table and BAD-D score row.
//!
//! The Belin layout differs from 4Maps/Topometric:
//! - Data table is in the right half of the page (cx ~1800-2500)
//! - Labels and values are close together (max_dx varies 200-700)
//! - BAD-D scores are in a bottom row (cy > 1900)
//! - QS field is categorical (OK/Suspect/Abnormal), not numeric

use regex::Regex;
use std::collections::HashMap;
use super::field_locate::{extract_numeric, LocatedField, AffineFit, fit_affine, ARCHETYPE_BELIN};
use super::ocr_engine::OcrItem;

/// Label definition: (regex pattern, field_name, max_dx to value, zone)
struct BelinLabel {
    pattern: Regex,
    field_name: &'static str,
    max_dx: f32,
    zone: BelinZone,
}

enum BelinZone {
    DataTable,  // cy 250-1200
    BadD,       // cy > 1900
}

fn belin_labels() -> Vec<BelinLabel> {
    vec![
        // Keratometry (data table, left column)
        BelinLabel { pattern: Regex::new(r"(?i)^K1\s*:?$").unwrap(), field_name: "Belin_K1", max_dx: 250.0, zone: BelinZone::DataTable },
        BelinLabel { pattern: Regex::new(r"(?i)^K2\s*:?$").unwrap(), field_name: "Belin_K2", max_dx: 250.0, zone: BelinZone::DataTable },
        BelinLabel { pattern: Regex::new(r"(?i)^KMax\s*:?$").unwrap(), field_name: "Belin_KMax", max_dx: 250.0, zone: BelinZone::DataTable },
        // Keratometry (data table, right column)
        BelinLabel { pattern: Regex::new(r"(?i)^Axis\s*:?$").unwrap(), field_name: "Belin_Axis", max_dx: 250.0, zone: BelinZone::DataTable },
        BelinLabel { pattern: Regex::new(r"(?i)^Q-?val\.?\s*:?$").unwrap(), field_name: "Belin_Qval", max_dx: 250.0, zone: BelinZone::DataTable },
        BelinLabel { pattern: Regex::new(r"(?i)^QS\s*:?$").unwrap(), field_name: "Belin_QS", max_dx: 250.0, zone: BelinZone::DataTable },
        // Pachymetry
        BelinLabel { pattern: Regex::new(r"(?i)Pachy\s*Thin").unwrap(), field_name: "Belin_PachyThin", max_dx: 700.0, zone: BelinZone::DataTable },
        BelinLabel { pattern: Regex::new(r"(?i)Dist\.?\s*Vertex").unwrap(), field_name: "Belin_DistVertex", max_dx: 700.0, zone: BelinZone::DataTable },
        // Elevation thickness
        BelinLabel { pattern: Regex::new(r"(?i)^F\.?Ele\.?Th\s*:?$").unwrap(), field_name: "Belin_F_Ele_Th", max_dx: 200.0, zone: BelinZone::DataTable },
        BelinLabel { pattern: Regex::new(r"(?i)^B\.?Ele\.?Th\s*:?$").unwrap(), field_name: "Belin_B_Ele_Th", max_dx: 200.0, zone: BelinZone::DataTable },
        // Progression Index
        BelinLabel { pattern: Regex::new(r"(?i)^Min\s*:?$").unwrap(), field_name: "Belin_Prog_Min", max_dx: 250.0, zone: BelinZone::DataTable },
        BelinLabel { pattern: Regex::new(r"(?i)^Max\s*:?$").unwrap(), field_name: "Belin_Prog_Max", max_dx: 250.0, zone: BelinZone::DataTable },
        BelinLabel { pattern: Regex::new(r"(?i)^Avg\s*:?$").unwrap(), field_name: "Belin_Prog_Avg", max_dx: 250.0, zone: BelinZone::DataTable },
        BelinLabel { pattern: Regex::new(r"(?i)^ART\s*max\s*:?$").unwrap(), field_name: "Belin_ARTmax", max_dx: 200.0, zone: BelinZone::DataTable },
        // BAD-D scores (bottom row)
        // Df, Db, Dp, Dt, Da are handled ONLY by Phase 2 positional fallback.
        // Label matching for these is unreliable because OCR frequently confuses
        // the similar labels (Df→Dt, Dt→D:, etc.), causing values to be assigned
        // to the wrong fields.
        // D_final uses the rightmost "D:" label to avoid confusion with misread Dt.
        BelinLabel { pattern: Regex::new(r"^D\s*:$").unwrap(), field_name: "Belin_D_final", max_dx: 200.0, zone: BelinZone::BadD },
    ]
}

fn in_zone(cy: f32, zone: &BelinZone) -> bool {
    match zone {
        BelinZone::DataTable => cy >= 250.0 && cy <= 1200.0,
        BelinZone::BadD => cy > 1900.0,
    }
}

/// Extract Belin fields from OCR items using label matching + positional fallback.
pub fn extract(items: &[OcrItem]) -> HashMap<String, LocatedField> {
    let labels = belin_labels();
    let max_dy: f32 = 20.0;
    let mut result: HashMap<String, LocatedField> = HashMap::new();

    // Phase 1: Label-based matching
    for label_def in &labels {
        if result.contains_key(label_def.field_name) {
            continue;
        }

        // Find label.
        // For D_final, use the RIGHTMOST matching "D:" label on the BAD-D row.
        // OCR sometimes misreads "Dt:" or "Df:" as "D:", and the real D_final
        // label is always the rightmost one.
        let label_match = if label_def.field_name == "Belin_D_final" {
            items.iter()
                .filter(|item| {
                    label_def.pattern.is_match(item.text.trim())
                        && in_zone(item.cy, &label_def.zone)
                })
                .max_by(|a, b| a.cx.partial_cmp(&b.cx).unwrap())
        } else {
            items.iter().find(|item| {
                label_def.pattern.is_match(item.text.trim())
                    && in_zone(item.cy, &label_def.zone)
            })
        };

        let label_item = match label_match {
            Some(l) => l,
            None => continue,
        };

        // Find value: nearest numeric to the right, same row
        let mut best_val: Option<(f64, &OcrItem)> = None;
        let mut best_dx = label_def.max_dx + 1.0;

        for item in items {
            if item.cx <= label_item.cx { continue; }
            let dx = item.cx - label_item.cx;
            let dy = (item.cy - label_item.cy).abs();
            if dx > label_def.max_dx || dy > max_dy { continue; }

            // Skip if looks like another label
            let is_label = labels.iter().any(|l| l.pattern.is_match(item.text.trim()));
            if is_label { continue; }

            // QS is categorical
            if label_def.field_name == "Belin_QS" {
                let t = item.text.trim().to_uppercase();
                if matches!(t.as_str(), "OK" | "SUSPECT" | "ABNORMAL" | "A") && dx < best_dx {
                    // Store categorical as a special value (0=OK, 1=Suspect, 2=Abnormal)
                    let cat_val = match t.as_str() {
                        "OK" => 0.0,
                        "SUSPECT" | "A" => 1.0,
                        "ABNORMAL" => 2.0,
                        _ => continue,
                    };
                    best_val = Some((cat_val, item));
                    best_dx = dx;
                }
                continue;
            }

            if let Some(val) = parse_belin_value(&item.text) {
                if dx < best_dx {
                    best_val = Some((val, item));
                    best_dx = dx;
                }
            }
        }

        if let Some((val, item)) = best_val {
            result.insert(label_def.field_name.to_string(), LocatedField {
                value: val,
                conf: item.confidence,
                cx: item.cx,
                cy: item.cy,
                raw_text: item.text.clone(),
            });
        }
    }

    // Phase 1b: D_final Group B fallback.
    // If D_final wasn't found by label matching and the affine fit suggests
    // Group B (beta > 100), try a Group-B-specific position for D_final.
    // Group B has D_final ~95px left of Group A (cx≈3123 vs 3218).
    let fit = fit_affine(&result, ARCHETYPE_BELIN);

    if !result.contains_key("Belin_D_final") && fit.beta > 100.0 {
        // Group B: D_final at cx≈3123 instead of 3218
        let cy_ref = 2368.0_f32; // same cy as Group A
        let cx_group_b = 3123.0_f32;
        let cy_pred = (fit.alpha * cy_ref as f64 + fit.beta) as f32;
        let cx_pred = cx_group_b + fit.delta_cx as f32;
        let win = 65.0_f32;

        let best = items.iter()
            .filter(|item| {
                (item.cy - cy_pred).abs() <= 35.0
                    && (item.cx - cx_pred).abs() <= win
            })
            .filter_map(|item| {
                let val = parse_belin_value(&item.text)?;
                let dist = ((item.cy - cy_pred).powi(2) + (item.cx - cx_pred).powi(2)).sqrt();
                Some((dist, val, item))
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        if let Some((_, val, item)) = best {
            result.insert("Belin_D_final".to_string(), LocatedField {
                value: val,
                conf: item.confidence,
                cx: item.cx,
                cy: item.cy,
                raw_text: item.text.clone(),
            });
        }
    }

    // Phase 1c: BAD-D row extraction anchored on D_final.
    //
    // Once D_final is found (by label or fallback), use its position to find
    // the other 5 BAD-D values. Collect numeric OCR items on the same row,
    // left of D_final, sort by cx, pick 5 rightmost with regular spacing.
    //
    // Sanity check: the 5 BAD-D values span ~1100-1300px total and are roughly
    // evenly spaced (~200-270px apart). Reject candidates that don't fit this
    // pattern (e.g., stray "+4" annotations from color bars).
    if let Some(d_final) = result.get("Belin_D_final") {
        let badd_names = ["Belin_Da", "Belin_Dt", "Belin_Dp", "Belin_Db", "Belin_Df"];
        let d_cy = d_final.cy;
        let d_cx = d_final.cx;

        // Collect all numeric items on the BAD-D row, left of D_final.
        // Filter: must be within 1400px left (Df is ~1300px from D),
        // must not be a label (e.g., "Db:", "Dp:"), and value must be
        // in plausible BAD-D range (-20 to 20).
        let mut row_values: Vec<(f32, f64, &OcrItem)> = items.iter()
            .filter(|item| {
                (item.cy - d_cy).abs() <= 20.0
                    && item.cx < d_cx - 100.0 // must be clearly left of D_final
                    && item.cx > d_cx - 1400.0
            })
            .filter(|item| {
                // Skip items that look like labels (contain ':' or letters other than e/E)
                let t = item.text.trim();
                !t.contains(':') && !t.chars().any(|c| c.is_ascii_alphabetic()
                    && c != 'e' && c != 'E' && c != '-')
            })
            .filter_map(|item| {
                let val = parse_belin_value(&item.text)?;
                // BAD-D values are typically in range -20 to +20
                if val.abs() > 25.0 { return None; }
                Some((item.cx, val, item))
            })
            .collect();

        // Sort by cx descending (rightmost first = Da, Dt, Dp, Db, Df)
        row_values.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

        // Deduplicate: keep only one item per ~80px horizontal band
        let mut deduped: Vec<(f32, f64, &OcrItem)> = Vec::new();
        for entry in &row_values {
            if deduped.last().map_or(true, |prev: &(f32, f64, &OcrItem)| (prev.0 - entry.0).abs() >= 80.0) {
                deduped.push(*entry);
            }
        }

        // Sanity check: we need exactly 5 values, and the total span should be
        // between 900 and 1400px (Df to Da spans ~1000-1300px across devices).
        if deduped.len() >= 5 {
            let candidates = &deduped[..5];
            let span = candidates[0].0 - candidates[4].0; // rightmost - leftmost
            if span >= 900.0 && span <= 1400.0 {
                for (i, (_, val, item)) in candidates.iter().enumerate() {
                    let name = badd_names[i];
                    if !result.contains_key(name) {
                        result.insert(name.to_string(), LocatedField {
                            value: *val,
                            conf: item.confidence,
                            cx: item.cx,
                            cy: item.cy,
                            raw_text: item.text.clone(),
                        });
                    }
                }
            }
        }
    }

    // Phase 2: Positional fallback using Belin archetype
    let win_y: f32 = 35.0;
    let win_x: f32 = 65.0;

    for &(field_name, cy_ref, cx_ref) in ARCHETYPE_BELIN {
        if result.contains_key(field_name) { continue; }
        // Skip QS for positional fallback — categorical
        if field_name == "Belin_QS" { continue; }

        let cy_pred = (fit.alpha * cy_ref as f64 + fit.beta) as f32;
        let cx_pred = cx_ref + fit.delta_cx as f32;

        let best = items.iter()
            .filter(|item| {
                (item.cy - cy_pred).abs() <= win_y
                    && (item.cx - cx_pred).abs() <= win_x
            })
            .filter_map(|item| {
                let val = parse_belin_value(&item.text)?;
                let dist = ((item.cy - cy_pred).powi(2) + (item.cx - cx_pred).powi(2)).sqrt();
                Some((dist, val, item))
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        if let Some((_, val, item)) = best {
            result.insert(field_name.to_string(), LocatedField {
                value: val,
                conf: item.confidence,
                cx: item.cx,
                cy: item.cy,
                raw_text: item.text.clone(),
            });
        }
    }

    result
}

/// Parse a numeric value from Belin field text, stripping units.
fn parse_belin_value(text: &str) -> Option<f64> {
    let mut raw = text.replace('\u{2212}', "-").replace(',', ".");
    // Strip units
    let re_um = Regex::new(r"(?i)\s*[µμu]m$").unwrap();
    raw = re_um.replace(&raw, "").to_string();
    let re_mm = Regex::new(r"(?i)\s*mm$").unwrap();
    raw = re_mm.replace(&raw, "").to_string();
    let re_d = Regex::new(r"\s*[dD]$").unwrap();
    raw = re_d.replace(&raw, "").to_string();
    let re_deg = Regex::new(r"[°*]$").unwrap();
    raw = re_deg.replace(&raw, "").to_string();

    let re_num = Regex::new(r"[-+]?\d+\.?\d*").unwrap();
    re_num.find(raw.trim()).and_then(|m| m.as_str().parse().ok())
}
