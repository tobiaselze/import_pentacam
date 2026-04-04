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
        // BAD-D scores (bottom row) are handled by template matching in
        // extract_badd_row() — not by label matching. OCR confuses the similar
        // labels (Df/Dt/Da/D:), so we match the spatial pattern instead.
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

    // BAD-D row: template-matching extraction.
    //
    // Find all D-label items (text matching "D" + optional letter + ":"),
    // cluster by cy to identify the BAD-D row, fit their cx positions against
    // two spacing templates (Group A and Group B), and use the best fit to
    // assign field names. This is robust to OCR misreading individual labels
    // and adapts to device-specific horizontal spacing.
    let badd = extract_badd_row(items);
    for (name, field) in badd {
        result.entry(name).or_insert(field);
    }

    // Phase 2: Positional fallback using Belin archetype (data table fields only)
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

// ---------------------------------------------------------------------------
// BAD-D row: template-matching extraction
// ---------------------------------------------------------------------------

/// Cumulative label offsets from Df for each layout group.
/// Slots: [Df, Db, Dp, Dt, Da, D_final]
const BADD_TEMPLATE_A: [f64; 6] = [0.0, 272.0, 498.0, 730.0, 966.0, 1304.0];
const BADD_TEMPLATE_B: [f64; 6] = [0.0, 210.0, 428.0, 646.0, 826.0, 1176.0];
const BADD_FIELD_NAMES: [&str; 6] = [
    "Belin_Df", "Belin_Db", "Belin_Dp", "Belin_Dt", "Belin_Da", "Belin_D_final",
];

/// Extract BAD-D row values using template matching.
///
/// 1. Find all D-label items on the page (text matching D + optional letter + colon)
/// 2. Cluster by cy to identify the BAD-D row
/// 3. Fit label cx positions against both spacing templates (affine: x = scale*t + offset)
/// 4. Best fit determines layout group and field assignment
/// 5. For each assigned label, find the nearest numeric value to its right
fn extract_badd_row(items: &[OcrItem]) -> HashMap<String, LocatedField> {
    let mut result = HashMap::new();
    let d_label_re = Regex::new(r"(?i)^D[a-z]?\s*:").unwrap();

    // Step 1: Find D-label candidates
    let candidates: Vec<&OcrItem> = items.iter()
        .filter(|item| d_label_re.is_match(item.text.trim()))
        .collect();

    if candidates.len() < 4 { return result; }

    // Step 2: Cluster by cy to find the BAD-D row.
    // The row with the most D-labels within a ±25px cy band.
    let mut best_cy = 0.0_f32;
    let mut best_count = 0_usize;
    for c in &candidates {
        let count = candidates.iter()
            .filter(|other| (other.cy - c.cy).abs() <= 25.0)
            .count();
        if count > best_count {
            best_count = count;
            best_cy = c.cy;
        }
    }
    if best_count < 4 { return result; }

    // Filter to BAD-D row, sorted by cx.
    // For combined label+value items (e.g., "Da: 0.25"), estimate the bare
    // label cx by shifting left. Combined items have centroid ~40-50px right
    // of the bare label, which throws off the template fit.
    let re_combined = Regex::new(r"^D[a-z]?\s*:\s*-?[0-9]").unwrap();
    let mut row: Vec<(f32, &OcrItem)> = candidates.iter()
        .filter(|item| (item.cy - best_cy).abs() <= 25.0)
        .map(|item| {
            let cx_adj = if re_combined.is_match(item.text.trim()) {
                // Estimate bare label position: shift left by ~half the value text width
                item.cx - 45.0
            } else {
                item.cx
            };
            (cx_adj, *item)
        })
        .collect();
    row.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let n = row.len();
    if n < 4 { return result; }

    // Step 3: Try both templates. For each, try all ordered subsets of
    // k labels (k=4..min(n,6)) assigned to k of the 6 template slots.
    // Fit affine (scale + offset) and track best overall fit.
    let mut best_resid = f64::MAX;
    let mut best_assignment: Vec<(usize, usize)> = Vec::new(); // (candidate_idx, slot_idx)
    let mut best_scale = 1.0_f64;
    let mut best_offset = 0.0_f64;
    let mut best_template: &[f64; 6] = &BADD_TEMPLATE_A;

    for template in [&BADD_TEMPLATE_A, &BADD_TEMPLATE_B] {
        for k in 4..=n.min(6) {
            // Enumerate ordered k-subsets of candidates (bitmask over n items)
            for cmask in 0..(1u32 << n) {
                if cmask.count_ones() as usize != k { continue; }
                let cidxs: Vec<usize> = (0..n).filter(|&i| cmask & (1 << i) != 0).collect();

                // Enumerate ordered k-subsets of 6 template slots
                for smask in 0..(1u32 << 6) {
                    if smask.count_ones() as usize != k { continue; }
                    let sidxs: Vec<usize> = (0..6).filter(|&i| smask & (1 << i) != 0).collect();

                    // Build pairs: (template_pos, adjusted_cx)
                    let pairs: Vec<(f64, f64)> = cidxs.iter().zip(sidxs.iter())
                        .map(|(&ci, &si)| (template[si], row[ci].0 as f64))
                        .collect();

                    let (scale, offset) = fit_1d(&pairs);

                    // Scale must be reasonable (0.85 - 1.15)
                    if scale < 0.85 || scale > 1.15 { continue; }

                    // Check per-point residuals — reject if any > 35px
                    let max_r = pairs.iter()
                        .map(|(t, x)| (x - (scale * t + offset)).abs())
                        .fold(0.0_f64, f64::max);
                    if max_r > 35.0 { continue; }

                    // Mean squared residual (prefer more points at similar residual)
                    let msr: f64 = pairs.iter()
                        .map(|(t, x)| (x - (scale * t + offset)).powi(2))
                        .sum::<f64>() / k as f64;

                    // Penalize fewer matches: add penalty for missing slots
                    let penalty = (6 - k) as f64 * 200.0;
                    let score = msr + penalty;

                    if score < best_resid {
                        best_resid = score;
                        best_scale = scale;
                        best_offset = offset;
                        best_template = template;
                        best_assignment = cidxs.iter().zip(sidxs.iter())
                            .map(|(&c, &s)| (c, s))
                            .collect();
                    }
                }
            }
        }
    }

    if best_assignment.is_empty() { return result; }

    // Step 4: For each of the 6 template slots, determine the predicted
    // label cx (from the fitted transform) and find the value.
    let badd_cy = best_cy;
    let max_dy = 20.0_f32;

    for slot in 0..6 {
        let field_name = BADD_FIELD_NAMES[slot];
        let template_pos = best_scale * best_template[slot] + best_offset;

        // Find the value: nearest numeric OCR item to the right of the
        // predicted label position, on the same row.
        let label_cx = template_pos as f32;
        let mut best_val: Option<(f32, f64, &OcrItem)> = None;

        for item in items {
            if (item.cy - badd_cy).abs() > max_dy { continue; }
            let dx = item.cx - label_cx;
            if dx < -20.0 || dx > 200.0 { continue; }

            // Skip D-labels
            if d_label_re.is_match(item.text.trim()) { continue; }

            if let Some(val) = parse_belin_value(&item.text) {
                if val.abs() > 25.0 { continue; } // BAD-D range check
                let dist = dx.abs();
                if best_val.as_ref().map_or(true, |b| dist < b.0) {
                    best_val = Some((dist, val, item));
                }
            }
        }

        if let Some((_, val, item)) = best_val {
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

/// Simple 1D affine fit: x = scale * t + offset (least squares).
fn fit_1d(pairs: &[(f64, f64)]) -> (f64, f64) {
    let n = pairs.len() as f64;
    if n < 2.0 { return (1.0, 0.0); }
    let st: f64 = pairs.iter().map(|p| p.0).sum();
    let sx: f64 = pairs.iter().map(|p| p.1).sum();
    let stt: f64 = pairs.iter().map(|p| p.0 * p.0).sum();
    let stx: f64 = pairs.iter().map(|p| p.0 * p.1).sum();
    let denom = n * stt - st * st;
    if denom.abs() < 1e-12 { return (1.0, sx / n); }
    let scale = (n * stx - st * sx) / denom;
    let offset = (sx - scale * st) / n;
    (scale, offset)
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
