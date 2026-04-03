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
        BelinLabel { pattern: Regex::new(r"(?i)^Df\s*:?$").unwrap(), field_name: "Belin_Df", max_dx: 200.0, zone: BelinZone::BadD },
        BelinLabel { pattern: Regex::new(r"(?i)^Db\s*:?$").unwrap(), field_name: "Belin_Db", max_dx: 200.0, zone: BelinZone::BadD },
        BelinLabel { pattern: Regex::new(r"(?i)^Dp\s*:?$").unwrap(), field_name: "Belin_Dp", max_dx: 200.0, zone: BelinZone::BadD },
        BelinLabel { pattern: Regex::new(r"(?i)^Dt\s*:?$").unwrap(), field_name: "Belin_Dt", max_dx: 200.0, zone: BelinZone::BadD },
        BelinLabel { pattern: Regex::new(r"(?i)^Da\s*:?$").unwrap(), field_name: "Belin_Da", max_dx: 200.0, zone: BelinZone::BadD },
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

        // Find label
        let label_match = items.iter().find(|item| {
            label_def.pattern.is_match(item.text.trim())
                && in_zone(item.cy, &label_def.zone)
        });

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

    // Phase 1b: Validate Dt is right of Dp (catches OCR splitting "Dt:" into "D"+"t:")
    if let (Some(dt), Some(dp)) = (result.get("Belin_Dt"), result.get("Belin_Dp")) {
        if dt.cx < dp.cx + 50.0 {
            result.remove("Belin_Dt");
        }
    }

    // Phase 1c: D_final — rightmost numeric on the BAD-D row.
    // The "D:" label is a single char that OCR often misses. Instead of relying
    // on label matching, find the BAD-D row cy from any found BAD-D field,
    // then take the rightmost value not already assigned to another field.
    if !result.contains_key("Belin_D_final") {
        let badd_cy = ["Belin_Df", "Belin_Db", "Belin_Dp", "Belin_Dt", "Belin_Da"]
            .iter()
            .filter_map(|&f| result.get(f))
            .map(|loc| loc.cy)
            .next();

        if let Some(row_cy) = badd_cy {
            let assigned_cx: Vec<f32> = ["Belin_Df","Belin_Db","Belin_Dp","Belin_Dt","Belin_Da"]
                .iter()
                .filter_map(|&f| result.get(f))
                .map(|loc| loc.cx)
                .collect();

            let mut best: Option<(f64, &OcrItem)> = None;
            let mut best_cx: f32 = 0.0;

            for item in items {
                if (item.cy - row_cy).abs() > 20.0 { continue; }
                if assigned_cx.iter().any(|&acx| (item.cx - acx).abs() < 50.0) { continue; }
                if let Some(val) = parse_belin_value(&item.text) {
                    if item.cx > best_cx {
                        best = Some((val, item));
                        best_cx = item.cx;
                    }
                }
            }

            if let Some((val, item)) = best {
                result.insert("Belin_D_final".to_string(), LocatedField {
                    value: val,
                    conf: item.confidence,
                    cx: item.cx,
                    cy: item.cy,
                    raw_text: item.text.clone(),
                });
            }
        }
    }

    // Phase 2: Positional fallback using Belin archetype
    let fit = fit_affine(&result, ARCHETYPE_BELIN);
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
