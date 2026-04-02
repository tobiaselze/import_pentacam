//! Field location via label matching and affine archetype fitting.
//!
//! Port of pentacam_ocr_v7.py find_labeled_values() + affine fit logic.

use super::ocr_engine::OcrItem;
use pentacam_types::PrintoutType;
use regex::Regex;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Archetype coordinates — Group A reference at 300 DPI
// ---------------------------------------------------------------------------

/// (cy_A, cx_A) for each field on 4-Maps Refractive/Selectable pages.
/// Measured on device 6317010, software 1.27r13.
pub const ARCHETYPE_4MAPS: &[(&str, f32, f32)] = &[
    // field,           cy_A,   cx_A
    ("Rf_front",        710.0,  494.0),
    ("Rs_front",        798.0,  494.0),
    ("Rm_front",        886.0,  493.0),
    ("K1_front",        710.0,  821.0),
    ("K2_front",        798.0,  821.0),
    ("Km_front",        886.0,  820.0),
    ("Astig_front",     976.0,  810.0),
    ("Axis_front",      976.0,  477.0),
    ("Rmin_front",     1064.0,  832.0),
    ("Rper_front",     1064.0,  493.0),
    ("Rf_back",        1240.0,  494.0),
    ("Rs_back",        1330.0,  494.0),
    ("Rm_back",        1419.0,  492.0),
    ("K1_back",        1240.0,  818.0),
    ("K2_back",        1330.0,  817.0),
    ("Km_back",        1420.0,  817.0),
    ("Astig_back",     1510.0,  810.0),
    ("Axis_back",      1510.0,  468.0),
    ("Rmin_back",      1597.0,  832.0),
    ("Rper_back",      1597.0,  493.0),
    ("PupilCenter",    1777.0,  486.0),
    ("PachyVertex",    1854.0,  485.0),
    ("Thinnest",       1932.0,  486.0),
    ("Kmax",           2007.0,  480.0),
    ("PupilCenter_x",  1776.0,  741.0),
    ("PupilCenter_y",  1776.0,  906.0),
    ("PachyVertex_x",  1852.0,  732.0),
    ("PachyVertex_y",  1852.0,  898.0),
    ("Thinnest_x",     1930.0,  742.0),
    ("Thinnest_y",     1930.0,  907.0),
    ("Kmax_x",         2008.0,  736.0),
    ("Kmax_y",         2008.0,  901.0),
    ("CorneaVol",      2101.0,  504.0),
    ("HWTW",           2102.0,  868.0),
    ("ChamberVol",     2187.0,  499.0),
    ("Angle",          2188.0,  851.0),
    ("AC_depth",       2272.0,  494.0),
    ("PupilDia",       2272.0,  867.0),
    ("Qval_front",     1140.0,  310.0),
    ("Qval_back",      1597.0,  310.0),
];

/// (cy_A, cx_A) for each field on Topometric/KC-Staging pages.
pub const ARCHETYPE_TOPO: &[(&str, f32, f32)] = &[
    ("Rf_front",        710.0,  518.0),
    ("Rs_front",        779.0,  518.0),
    ("Rm_front",        847.0,  516.0),
    ("K1_front",        710.0,  867.0),
    ("K2_front",        778.0,  867.0),
    ("Km_front",        848.0,  866.0),
    ("Astig_front",     917.0,  857.0),
    ("Axis_front",      917.0,  500.0),
    ("Rmin_front",      985.0,  878.0),
    ("Rper_front",      985.0,  518.0),
    ("Qval_front",      984.0,  246.0),
    ("Rf_back",        1122.0,  517.0),
    ("Rs_back",        1189.0,  517.0),
    ("Rm_back",        1259.0,  516.0),
    ("K1_back",        1122.0,  864.0),
    ("K2_back",        1189.0,  862.0),
    ("Km_back",        1259.0,  863.0),
    ("Astig_back",     1328.0,  858.0),
    ("Axis_back",      1328.0,  500.0),
    ("Rmin_back",      1396.0,  878.0),
    ("Rper_back",      1396.0,  518.0),
    ("Qval_back",      1396.0,  246.0),
    ("TNP_Astig",      1533.0,  500.0),
    ("TNP_K1",         1532.0,  868.0),
    ("TNP_Axis",       1600.0,  505.0),
    ("TNP_K2",         1601.0,  866.0),
    ("TNP_PMax",       1670.0,  504.0),
    ("TNP_Km",         1670.0,  867.0),
    ("PupilCenter",    1851.0,  508.0),
    ("PachyVertex",    1929.0,  508.0),
    ("Thinnest",       2005.0,  508.0),
    ("Kmax",           2082.0,  504.0),
    ("PupilCenter_x",  1850.0,  774.0),
    ("PupilCenter_y",  1851.0,  942.0),
    ("PachyVertex_x",  1928.0,  770.0),
    ("PachyVertex_y",  1928.0,  934.0),
    ("Thinnest_x",     2004.0,  776.0),
    ("Thinnest_y",     2005.0,  940.0),
    ("Kmax_x",         2082.0,  774.0),
    ("Kmax_y",         2082.0,  938.0),
    ("CorneaVol",      2168.0,  517.0),
    ("HWTW",           2168.0,  915.0),
    ("ChamberVol",     2236.0,  522.0),
    ("Angle",          2238.0,  900.0),
    ("AC_depth",       2305.0,  516.0),
    ("PupilDia",       2304.0,  914.0),
];

/// (cy_A, cx_A) for each field on Belin/Ambrosio Enhanced Ectasia pages.
pub const ARCHETYPE_BELIN: &[(&str, f32, f32)] = &[
    // Keratometry (data table, left column)
    ("Belin_K1",           552.0, 2012.0),
    ("Belin_K2",           604.0, 2012.0),
    ("Belin_KMax",         656.0, 2012.0),
    // Keratometry (data table, right column)
    ("Belin_Axis",         552.0, 2376.0),
    ("Belin_Qval",         604.0, 2374.0),
    ("Belin_QS",           656.0, 2362.0),
    // Pachymetry
    ("Belin_PachyThin",    716.0, 2385.0),
    ("Belin_DistVertex",   767.0, 2392.0),
    // Elevation thickness
    ("Belin_F_Ele_Th",     828.0, 2008.0),
    ("Belin_B_Ele_Th",     828.0, 2375.0),
    // Progression Index
    ("Belin_Prog_Min",     933.0, 2001.0),
    ("Belin_Prog_Max",     933.0, 2369.0),
    ("Belin_Prog_Avg",     986.0, 2002.0),
    ("Belin_ARTmax",       985.0, 2365.0),
    // BAD-D scores (bottom row)
    ("Belin_Df",          2370.0, 1947.0),
    ("Belin_Db",          2370.0, 2178.0),
    ("Belin_Dp",          2370.0, 2410.0),
    ("Belin_Dt",          2370.0, 2646.0),
    ("Belin_Da",          2370.0, 2873.0),
    ("Belin_D_final",     2368.0, 3218.0),
];

// ---------------------------------------------------------------------------
// Located field result
// ---------------------------------------------------------------------------

/// A located field: its value, confidence, and position on the page.
#[derive(Debug, Clone)]
pub struct LocatedField {
    pub value: f64,
    pub conf: f32,
    pub cx: f32,
    pub cy: f32,
    pub raw_text: String,
}

// ---------------------------------------------------------------------------
// Affine fit result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AffineFit {
    pub alpha: f64,
    pub beta: f64,
    pub delta_cx: f64,
    pub resid_std: f64,
    pub n_inliers: usize,
    pub n_pairs: usize,
}

// ---------------------------------------------------------------------------
// Number extraction from OCR text
// ---------------------------------------------------------------------------

/// Extract the first numeric value from an OCR text token.
/// Handles common OCR mistakes (comma→period, o→0, Z→7, ~→-).
pub fn extract_numeric(text: &str) -> Option<f64> {
    let mut text = text.replace(',', ".").replace('o', "0").replace('O', "0");

    // Z→7 when adjacent to digits (no lookbehind in Rust regex — use manual replacement)
    // Replace Z followed by digit/dot, but only if not preceded by a letter
    let chars: Vec<char> = text.chars().collect();
    let mut new_text = String::with_capacity(text.len());
    for (i, &ch) in chars.iter().enumerate() {
        if ch == 'Z' {
            let preceded_by_letter = i > 0 && chars[i - 1].is_ascii_alphabetic();
            let followed_by_digit = i + 1 < chars.len()
                && (chars[i + 1].is_ascii_digit() || chars[i + 1] == '.');
            if !preceded_by_letter && followed_by_digit {
                new_text.push('7');
                continue;
            }
        }
        new_text.push(ch);
    }
    text = new_text;

    // ~→- (misread of minus sign)
    let re_tilde = Regex::new(r"~-?([0-9])").unwrap();
    text = re_tilde.replace_all(&text, "-$1").to_string();

    // Collapse duplicate periods
    let re_dots = Regex::new(r"\.{2,}").unwrap();
    text = re_dots.replace_all(&text, ".").to_string();

    // Strip unit suffixes
    let text = text.trim();
    let re_um = Regex::new(r"[pµu]m$").unwrap();
    let text = re_um.replace(text, "").to_string();
    let re_mm = Regex::new(r"\s*mm[23³]?$").unwrap();
    let text = re_mm.replace(&text, "").to_string();
    let re_d = Regex::new(r"\s*[dD]$").unwrap();
    let text = re_d.replace(&text, "").to_string();
    let text = text.trim();

    // Leading-decimal fragment: '.7' → 0.7
    let re_ld = Regex::new(r"^\.([0-9]+)$").unwrap();
    if let Some(caps) = re_ld.captures(text) {
        return format!("0.{}", &caps[1]).parse().ok();
    }

    // Leading-zero integer: '09' → 0.9
    let stripped = text.trim_start_matches(&['(', '['][..]);
    let re_lz = Regex::new(r"^0([0-9]+)$").unwrap();
    if let Some(caps) = re_lz.captures(stripped) {
        return format!("0.{}", &caps[1]).parse().ok();
    }

    // Split decimal: '38 .1' → 38.1
    let re_split = Regex::new(r"(-?[0-9]+)\s+\.([0-9]+)").unwrap();
    if let Some(caps) = re_split.captures(text) {
        return format!("{}.{}", &caps[1], &caps[2]).parse().ok();
    }

    // Prefer float with decimal point over bare integer
    let re_float = Regex::new(r"-?[0-9]+\.[0-9]+").unwrap();
    if let Some(m) = re_float.find(text) {
        return m.as_str().parse().ok();
    }

    // Fall back to any number
    let re_num = Regex::new(r"-?[0-9]+\.?[0-9]*").unwrap();
    re_num.find(text).and_then(|m| m.as_str().parse().ok())
}

// ---------------------------------------------------------------------------
// Affine fitting
// ---------------------------------------------------------------------------

/// Fit cy_actual = alpha * cy_ref + beta using least-squares, with outlier rejection.
pub fn fit_affine(
    labeled: &HashMap<String, LocatedField>,
    archetype: &[(&str, f32, f32)],
) -> AffineFit {
    let arch_map: HashMap<&str, (f32, f32)> = archetype.iter()
        .map(|&(name, cy, cx)| (name, (cy, cx)))
        .collect();

    let pairs: Vec<(f64, f64)> = labeled.iter()
        .filter_map(|(name, field)| {
            arch_map.get(name.as_str()).map(|&(cy_ref, _)| (cy_ref as f64, field.cy as f64))
        })
        .collect();

    let n_pairs = pairs.len();
    if n_pairs < 5 {
        return AffineFit {
            alpha: 1.0, beta: 0.0, delta_cx: 0.0,
            resid_std: 0.0, n_inliers: 0, n_pairs,
        };
    }

    // Initial least-squares fit
    let (alpha, beta) = polyfit1(&pairs);

    // Outlier rejection: drop residuals > 40 px, refit
    let inliers: Vec<(f64, f64)> = pairs.iter()
        .filter(|&&(ref_cy, act_cy)| (act_cy - (alpha * ref_cy + beta)).abs() < 40.0)
        .copied()
        .collect();

    let (alpha, beta) = if inliers.len() >= 5 {
        polyfit1(&inliers)
    } else {
        (alpha, beta)
    };

    let resid_std = {
        let residuals: Vec<f64> = inliers.iter()
            .map(|&(r, a)| a - (alpha * r + beta))
            .collect();
        let mean = residuals.iter().sum::<f64>() / residuals.len() as f64;
        let var = residuals.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / residuals.len() as f64;
        var.sqrt()
    };

    // Horizontal shift: median of cx_actual - cx_ref
    let cx_deltas: Vec<f64> = labeled.iter()
        .filter_map(|(name, field)| {
            arch_map.get(name.as_str()).map(|&(_, cx_ref)| field.cx as f64 - cx_ref as f64)
        })
        .collect();
    let delta_cx = median(&cx_deltas);

    AffineFit {
        alpha, beta, delta_cx, resid_std,
        n_inliers: inliers.len(),
        n_pairs,
    }
}

/// Simple least-squares linear fit: y = a*x + b
fn polyfit1(pairs: &[(f64, f64)]) -> (f64, f64) {
    let n = pairs.len() as f64;
    let sx: f64 = pairs.iter().map(|p| p.0).sum();
    let sy: f64 = pairs.iter().map(|p| p.1).sum();
    let sxx: f64 = pairs.iter().map(|p| p.0 * p.0).sum();
    let sxy: f64 = pairs.iter().map(|p| p.0 * p.1).sum();
    let denom = n * sxx - sx * sx;
    if denom.abs() < 1e-12 {
        return (1.0, 0.0);
    }
    let a = (n * sxy - sx * sy) / denom;
    let b = (sy - a * sx) / n;
    (a, b)
}

fn median(vals: &[f64]) -> f64 {
    if vals.is_empty() { return 0.0; }
    let mut sorted = vals.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

// ---------------------------------------------------------------------------
// Select archetype for printout type
// ---------------------------------------------------------------------------

pub fn archetype_for(printout_type: &PrintoutType) -> &'static [(&'static str, f32, f32)] {
    match printout_type {
        PrintoutType::TopometricKcStaging => ARCHETYPE_TOPO,
        PrintoutType::BelinAmbrosio => ARCHETYPE_BELIN,
        _ => ARCHETYPE_4MAPS,
    }
}

/// Select archetype based on whether the printout is Topometric.
/// Used by label_match.rs where we don't have the full PrintoutType enum.
pub fn archetype_for_type(is_topo: bool) -> &'static [(&'static str, f32, f32)] {
    if is_topo { ARCHETYPE_TOPO } else { ARCHETYPE_4MAPS }
}
