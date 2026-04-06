//! Compact CSV generator: pools raw CSV rows into one row per eye-visit.
//!
//! For each field, selects the best value across all sources (SR > SPR > OCR),
//! runs cross-validation between SR and OCR, and flags suspicious values.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use crate::field_map::{self, ALL_FIELDS};

// ---------------------------------------------------------------------------
// Compact row
// ---------------------------------------------------------------------------

/// Best value for a single field, with provenance.
struct BestValue {
    value: f64,
    source: String,
    confidence: f32,
    reliability: f32,
    flag: String,
}

// ---------------------------------------------------------------------------
// Generation
// ---------------------------------------------------------------------------

/// Generate the compact CSV from the raw CSV.
/// Groups by scan_hash, picks best value per field, cross-validates SR vs OCR.
pub fn generate_compact(raw_csv_path: &Path, output_path: &Path) -> Result<u32, String> {
    let file = File::open(raw_csv_path)
        .map_err(|e| format!("Open raw CSV: {}", e))?;
    let reader = BufReader::new(file);

    // Parse header
    let mut lines = reader.lines();
    let header_line = lines.next()
        .ok_or("Empty raw CSV")?
        .map_err(|e| format!("Read header: {}", e))?;
    let headers: Vec<&str> = header_line.split(',').collect();
    let col_index: HashMap<&str, usize> = headers.iter().enumerate()
        .map(|(i, &h)| (h, i))
        .collect();

    // Read all rows, group by scan_hash
    let mut groups: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    for line in lines {
        let line = line.map_err(|e| format!("Read line: {}", e))?;
        if line.trim().is_empty() { continue; }
        let cols: Vec<String> = parse_csv_line(&line);
        let hash_idx = *col_index.get("scan_hash").ok_or("No scan_hash column")?;
        let hash = cols.get(hash_idx).cloned().unwrap_or_default();
        if hash.is_empty() { continue; }
        groups.entry(hash).or_default().push(cols);
    }

    // Write compact CSV
    let out = File::create(output_path)
        .map_err(|e| format!("Create compact CSV: {}", e))?;
    let mut writer = BufWriter::new(out);

    // Header
    let mut header = "patient_id,patient_name,dob,sex,eye,exam_date,exam_time,\
        device_serial,software_version,scan_hash,n_source_rows,source_types"
        .to_string();
    for &field in ALL_FIELDS {
        header.push_str(&format!(",{},{}_source,{}_conf,{}_reliability,{}_flag",
            field, field, field, field, field));
    }
    writeln!(writer, "{}", header)
        .map_err(|e| format!("Write header: {}", e))?;

    let mut n_visits = 0u32;

    for (hash, rows) in &groups {
        if rows.is_empty() { continue; }

        // Use first row for demographics
        let first = &rows[0];
        let get = |col: &str| -> String {
            col_index.get(col)
                .and_then(|&i| first.get(i))
                .cloned()
                .unwrap_or_default()
        };

        // Collect source types
        let source_types: Vec<String> = rows.iter()
            .filter_map(|r| col_index.get("printout_type").and_then(|&i| r.get(i)).cloned())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        let mut line = format!("{},{},{},{},{},{},{},{},{},{},{},\"{}\"",
            csv_escape(&get("patient_id")),
            csv_escape(&get("patient_name")),
            csv_escape(&get("dob")),
            csv_escape(&get("sex")),
            csv_escape(&get("eye")),
            csv_escape(&get("exam_date")),
            csv_escape(&get("exam_time")),
            csv_escape(&get("device_serial")),
            csv_escape(&get("software_version")),
            csv_escape(hash),
            rows.len(),
            source_types.join("|"),
        );

        // For each field, find best value
        for &field in ALL_FIELDS {
            let best = select_best_value(field, rows, &col_index);
            match best {
                Some(bv) => {
                    line.push_str(&format!(",{},{},{:.4},{:.3},{}",
                        bv.value, bv.source, bv.confidence, bv.reliability, bv.flag));
                }
                None => {
                    line.push_str(",,,,,");
                }
            }
        }

        writeln!(writer, "{}", line)
            .map_err(|e| format!("Write row: {}", e))?;
        n_visits += 1;
    }

    writer.flush().map_err(|e| format!("Flush: {}", e))?;
    Ok(n_visits)
}

/// Select the best value for a field across all raw rows.
///
/// Priority: SR (conf=1.0) > SPR (conf=1.0) > highest-confidence OCR.
/// Exception: HWTW prefers SPR > SR > OCR.
fn select_best_value(
    field: &str,
    rows: &[Vec<String>],
    col_index: &HashMap<&str, usize>,
) -> Option<BestValue> {
    let field_idx = *col_index.get(field)?;
    let conf_col = format!("{}_conf", field);
    let conf_idx = col_index.get(conf_col.as_str()).copied();
    let type_idx = *col_index.get("printout_type")?;

    let reliability = field_map::field_reliability(field);

    // Collect all non-empty values with their source and confidence
    let mut candidates: Vec<(f64, String, f32)> = Vec::new();
    for row in rows {
        let val_str = row.get(field_idx).map(|s| s.as_str()).unwrap_or("");
        if val_str.is_empty() { continue; }
        let val: f64 = match val_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let source = row.get(type_idx).cloned().unwrap_or_default();
        let conf: f32 = conf_idx
            .and_then(|i| row.get(i))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        candidates.push((val, source, conf));
    }

    if candidates.is_empty() { return None; }

    // Priority selection
    let is_hwtw = field == "HWTW";

    // Find SR value
    let sr = candidates.iter().find(|(_, src, _)| src == "SR");
    // Find SPR value
    let spr = candidates.iter().find(|(_, src, _)| src == "SPR");
    // Find best OCR value (highest confidence)
    let best_ocr = candidates.iter()
        .filter(|(_, src, _)| src != "SR" && src != "SPR")
        .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

    let (value, source, confidence) = if is_hwtw {
        // HWTW: SPR > SR > OCR
        spr.or(sr).or(best_ocr)
    } else {
        // All other fields: SR > SPR > OCR
        sr.or(spr).or(best_ocr)
    }
    .map(|(v, s, c)| (*v, s.clone(), *c))?;

    // Cross-validation flag
    let mut flag = String::new();
    if let (Some(sr_val), Some(ocr_val)) = (sr, best_ocr) {
        let delta = (sr_val.0 - ocr_val.0).abs();
        let threshold = cross_validation_threshold(field);
        if delta > threshold {
            flag = "SR_OCR_MISMATCH".to_string();
        }
    }
    if flag.is_empty() && reliability < 0.96 {
        flag = "REVIEW".to_string();
    }
    if flag.is_empty() && confidence < 0.85 && confidence > 0.0 {
        flag = "LOW_CONF".to_string();
    }

    Some(BestValue { value, source, confidence, reliability, flag })
}

/// Field-specific cross-validation thresholds for SR vs OCR comparison.
fn cross_validation_threshold(field: &str) -> f64 {
    if field.contains("Df") || field.contains("Db") || field.contains("Dp")
        || field.contains("Dt") || field.contains("Da") || field.contains("D_final")
        || field.contains("Prog_")
    {
        0.02 // BAD-D and progression: two decimal places
    } else if field.contains("Pachy") || field.contains("Thinnest") {
        2.0 // Pachymetry: integer micrometers
    } else if field.contains("ARTmax") {
        5.0 // ARTmax: integer scale
    } else if field.contains("Axis") {
        1.0 // Axis: integer degrees
    } else if field.contains("K1") || field.contains("K2") || field.contains("Km")
        || field.contains("KMax") || field.contains("Kmax")
    {
        0.1 // Keratometry: 0.1 D precision
    } else {
        0.05 // Default
    }
}

// ---------------------------------------------------------------------------
// CSV parsing helpers
// ---------------------------------------------------------------------------

/// Parse a CSV line respecting quoted fields.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(ch);
            }
        } else {
            match ch {
                ',' => {
                    fields.push(current.clone());
                    current.clear();
                }
                '"' => {
                    in_quotes = true;
                }
                _ => {
                    current.push(ch);
                }
            }
        }
    }
    fields.push(current);
    fields
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
