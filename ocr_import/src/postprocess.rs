//! Post-processing corrections for OCR-extracted field values.
//!
//! Rule-based fixes using domain knowledge about Pentacam measurement ranges.
//! Port of pentacam_ocr_v7.py post-processing passes.

use regex::Regex;
use std::collections::HashMap;
use super::field_locate::LocatedField;

/// Apply all post-processing corrections to the extracted fields.
pub fn apply_corrections(labeled: &mut HashMap<String, LocatedField>) {
    fix_coordinate_fields(labeled);
    fix_k_back_sign(labeled);
    fix_k_front_range(labeled);
    fix_astigmatism(labeled);
    fix_qval(labeled);
    fix_radius_range(labeled);
    fix_pupil_dia(labeled);
    fix_ac_depth(labeled);
    fix_pachy_range(labeled);
}

/// Coordinate fields (PupilCenter_x/y, PachyVertex_x/y, etc.) are in ±4.5 mm range.
/// OCR reads '-' as '1', or drops decimal points.
fn fix_coordinate_fields(labeled: &mut HashMap<String, LocatedField>) {
    let coord_fields = [
        "PupilCenter_x", "PupilCenter_y",
        "PachyVertex_x", "PachyVertex_y",
        "Thinnest_x", "Thinnest_y",
        "Kmax_x", "Kmax_y",
    ];

    let re_case0 = Regex::new(r"^1\+([0-9]+\.[0-9]+)$").unwrap();
    let re_case1 = Regex::new(r"^1([0-9]\.[0-9]+)$").unwrap();
    let re_case2 = Regex::new(r"^[0-9]([0-9]\.[0-9]+)$").unwrap();

    for &field in &coord_fields {
        let entry = match labeled.get(field) {
            Some(e) => e.clone(),
            None => continue,
        };

        let tok = entry.raw_text.split_whitespace().next().unwrap_or("")
            .trim_start_matches(&['(', '['][..])
            .trim_end_matches(')');

        // Case 0: '1+1.21' → '-1.21'
        if let Some(caps) = re_case0.captures(tok) {
            let v: f64 = caps[1].parse().unwrap_or(0.0);
            if v.abs() <= 5.0 {
                let mut e = entry.clone();
                e.value = -v;
                labeled.insert(field.to_string(), e);
                continue;
            }
        }

        if entry.value.abs() <= 3.0 { continue; }

        let mut corrected: Option<f64> = None;

        // Case 1: '10.57' → '-0.57' (leading 1 = misread minus)
        if let Some(caps) = re_case1.captures(tok) {
            let v: f64 = caps[1].parse().unwrap_or(0.0);
            if v.abs() <= 5.0 {
                corrected = Some(-v);
            }
        }

        // Case 2: '71.57' → '-1.57' (leading digit = misread bracket)
        if corrected.is_none() {
            if let Some(caps) = re_case2.captures(tok) {
                let v: f64 = caps[1].parse().unwrap_or(0.0);
                if v.abs() <= 5.0 {
                    corrected = Some(-v);
                }
            }
        }

        // Case 3: '294' (integer) → '2.94' → '-2.94'
        if corrected.is_none() {
            let tok_nopm = tok.trim_start_matches(&['+', '-'][..]);
            if tok_nopm.chars().all(|c| c.is_ascii_digit()) && tok_nopm.len() >= 2 {
                let reconstructed: f64 = format!(
                    "{}.{}", &tok_nopm[..1], &tok_nopm[1..]
                ).parse().unwrap_or(99.0);
                if reconstructed.abs() <= 5.0 {
                    corrected = Some(if tok.starts_with('+') { reconstructed } else { -reconstructed });
                }
            }
        }

        // Case 4: '222.0' → '2.22'
        if corrected.is_none() && tok.contains('.') {
            let tok_abs = tok.trim_start_matches(&['+', '-'][..]);
            let parts: Vec<&str> = tok_abs.splitn(2, '.').collect();
            if parts.len() == 2 {
                let combined = format!("{}{}", parts[0], parts[1]);
                if combined.len() >= 2 && combined.chars().all(|c| c.is_ascii_digit()) {
                    let reconstructed: f64 = format!(
                        "{}.{}", &combined[..1], &combined[1..]
                    ).parse().unwrap_or(99.0);
                    if reconstructed.abs() <= 5.0 {
                        let sign = if tok.starts_with('+') { 1.0 } else { -1.0 };
                        corrected = Some(sign * reconstructed);
                    }
                }
            }
        }

        if let Some(c) = corrected {
            let mut e = entry.clone();
            e.value = c;
            labeled.insert(field.to_string(), e);
        }
    }
}

/// K_back values are always negative on Pentacam.
/// OCR reads '-' as '1', turning '-6.6' into '16.6'.
fn fix_k_back_sign(labeled: &mut HashMap<String, LocatedField>) {
    let re_neg = Regex::new(r"^1([0-9]+\.[0-9]+)$").unwrap();
    let re_int2 = Regex::new(r"^([0-9])([0-9])$").unwrap();
    let re_int3 = Regex::new(r"^1([0-9])([0-9])$").unwrap();

    for field in &["K1_back", "K2_back", "Km_back"] {
        let entry = match labeled.get(*field) {
            Some(e) if e.value > 0.0 => e.clone(),
            _ => continue,
        };
        let tok = entry.raw_text.split_whitespace().next().unwrap_or("")
            .trim_end_matches(&['d', 'D'][..]);

        let mut corrected: Option<f64> = None;

        // '16.6' → '-6.6'
        if let Some(caps) = re_neg.captures(tok) {
            corrected = Some(-caps[1].parse::<f64>().unwrap_or(0.0));
        }
        // '56' → '-5.6'
        if corrected.is_none() {
            if let Some(caps) = re_int2.captures(tok) {
                corrected = Some(-format!("{}.{}", &caps[1], &caps[2]).parse::<f64>().unwrap_or(0.0));
            }
        }
        // '165' → '-6.5'
        if corrected.is_none() {
            if let Some(caps) = re_int3.captures(tok) {
                corrected = Some(-format!("{}.{}", &caps[1], &caps[2]).parse::<f64>().unwrap_or(0.0));
            }
        }
        // Fallback: K_back is always negative
        if corrected.is_none() {
            corrected = Some(-entry.value.abs());
        }

        if let Some(c) = corrected {
            let mut e = entry;
            e.value = c;
            labeled.insert(field.to_string(), e);
        }
    }
}

/// K_front values are typically 30–55 D.
/// Spurious leading '1': '141.6' → 41.6. Missing decimal: '426' → 42.6.
fn fix_k_front_range(labeled: &mut HashMap<String, LocatedField>) {
    let re_spur = Regex::new(r"^1([3-9][0-9]\.[0-9]+)$").unwrap();
    let re_dec = Regex::new(r"^([3-9][0-9])([0-9]{1,2})$").unwrap();

    for field in &["K1_front", "K2_front", "Km_front", "Kmax"] {
        let entry = match labeled.get(*field) {
            Some(e) if e.value > 100.0 => e.clone(),
            _ => continue,
        };
        let tok = entry.raw_text.split_whitespace().next().unwrap_or("")
            .trim_end_matches(&['d', 'D'][..]);

        let mut corrected: Option<f64> = None;

        if let Some(caps) = re_spur.captures(tok) {
            corrected = Some(caps[1].parse().unwrap_or(0.0));
        }
        if corrected.is_none() {
            if let Some(caps) = re_dec.captures(tok) {
                corrected = Some(format!("{}.{}", &caps[1], &caps[2]).parse().unwrap_or(0.0));
            }
        }

        if let Some(c) = corrected {
            let mut e = entry;
            e.value = c;
            labeled.insert(field.to_string(), e);
        }
    }
}

/// Astigmatism: spurious leading '1' and sign fixes.
fn fix_astigmatism(labeled: &mut HashMap<String, LocatedField>) {
    let re_spur = Regex::new(r"^1([0-9]\.[0-9]+)$").unwrap();
    let re_nodec = Regex::new(r"^1([1-9])$").unwrap();
    let re_lz = Regex::new(r"^1(0[0-9])(?:\.[0-9]*)?$").unwrap();

    for field in &["Astig_front", "Astig_back"] {
        let entry = match labeled.get(*field) {
            Some(e) => e.clone(),
            None => continue,
        };
        let tok = entry.raw_text.split_whitespace().next().unwrap_or("")
            .trim_end_matches(&['d', 'D'][..]);
        let val = entry.value;

        let mut corrected: Option<f64> = None;

        if val > 10.0 && val < 11.0 {
            corrected = Some(val - 10.0);
        } else if val >= 11.0 && val <= 20.0 {
            if let Some(caps) = re_spur.captures(tok) {
                corrected = Some(caps[1].parse().unwrap_or(0.0));
            } else if let Some(caps) = re_nodec.captures(tok) {
                corrected = Some(format!("1.{}", &caps[1]).parse().unwrap_or(0.0));
            }
        } else if val >= 100.0 && val < 120.0 {
            if let Some(caps) = re_lz.captures(tok) {
                corrected = Some(format!("0.{}", &caps[1].chars().last().unwrap()).parse().unwrap_or(0.0));
            }
        } else if val < 0.0 {
            let tok0 = entry.raw_text.split_whitespace().next().unwrap_or("");
            if tok0.starts_with("1-") {
                corrected = Some(val.abs());
            }
        }

        if let Some(c) = corrected {
            let mut e = entry;
            e.value = c;
            labeled.insert(field.to_string(), e);
        }
    }
}

/// Qval (asphericity) is in range -2.0 to +2.0.
/// OCR misreads sign: 10.26 → -0.26.
fn fix_qval(labeled: &mut HashMap<String, LocatedField>) {
    for field in &["Qval_front", "Qval_back"] {
        let entry = match labeled.get(*field) {
            Some(e) if e.value > 5.0 => e.clone(),
            _ => continue,
        };
        let mod_val = entry.value % 10.0;
        let corrected = if mod_val <= 2.5 { -mod_val } else { -(entry.value - 10.0) };
        let mut e = entry;
        e.value = corrected;
        labeled.insert(field.to_string(), e);
    }
}

/// Radius fields are typically 4–12 mm.
/// Spurious leading digit: '18.18' → 8.18.
fn fix_radius_range(labeled: &mut HashMap<String, LocatedField>) {
    let re_spur = Regex::new(r"^[0-9]([4-9]\.[0-9]+)$").unwrap();
    let r_fields = [
        "Rf_front", "Rs_front", "Rm_front", "Rmin_front", "Rper_front",
        "Rf_back", "Rs_back", "Rm_back", "Rmin_back", "Rper_back",
    ];

    for field in &r_fields {
        let entry = match labeled.get(*field) {
            Some(e) if e.value > 12.0 => e.clone(),
            _ => continue,
        };
        let tok = entry.raw_text.split_whitespace().next().unwrap_or("")
            .trim_end_matches(&['m', 'M'][..]);

        if let Some(caps) = re_spur.captures(tok) {
            let mut e = entry;
            e.value = caps[1].parse().unwrap_or(e.value);
            labeled.insert(field.to_string(), e);
        }
    }
}

/// PupilDia is 1–8 mm. Spurious leading '1': '13.32' → 3.32.
fn fix_pupil_dia(labeled: &mut HashMap<String, LocatedField>) {
    let (val, raw) = match labeled.get("PupilDia") {
        Some(e) if e.value > 10.0 => (e.value, e.raw_text.clone()),
        _ => return,
    };
    let re_spur = Regex::new(r"^1([0-9]\.[0-9]+)$").unwrap();
    let tok = raw.split_whitespace().next().unwrap_or("")
        .trim_end_matches(&['d', 'D', 'm', 'M'][..]);
    if let Some(caps) = re_spur.captures(tok) {
        if let Ok(corrected) = caps[1].parse::<f64>() {
            labeled.get_mut("PupilDia").unwrap().value = corrected;
        }
    }
}

/// AC_depth is 2–5 mm. Spurious leading '1': '13.24' → 3.24.
fn fix_ac_depth(labeled: &mut HashMap<String, LocatedField>) {
    let (val, raw) = match labeled.get("AC_depth") {
        Some(e) if e.value > 10.0 => (e.value, e.raw_text.clone()),
        _ => return,
    };
    let re_spur = Regex::new(r"^1([0-9]\.[0-9]+)$").unwrap();
    let tok = raw.split_whitespace().next().unwrap_or("")
        .trim_end_matches(&['m', 'M'][..]);
    if let Some(caps) = re_spur.captures(tok) {
        if let Ok(corrected) = caps[1].parse::<f64>() {
            labeled.get_mut("AC_depth").unwrap().value = corrected;
        }
    }
}

/// Pachymetry values (Thinnest, PachyVertex, PupilCenter) are 300–700 µm.
/// Extra digit: '5555' → 555.
fn fix_pachy_range(labeled: &mut HashMap<String, LocatedField>) {
    for field in &["Thinnest", "PachyVertex", "PupilCenter"] {
        let val = match labeled.get(*field) {
            Some(e) => e.value,
            None => continue,
        };

        if val >= 1000.0 && val < 10000.0 {
            let s = format!("{:.0}", val);
            if s.len() == 4 {
                let corrected: f64 = s[1..].parse().unwrap_or(val);
                if (300.0..=700.0).contains(&corrected) {
                    labeled.get_mut(*field).unwrap().value = corrected;
                }
            }
        }
    }
}
