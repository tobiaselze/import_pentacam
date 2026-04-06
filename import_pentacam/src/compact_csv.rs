//! Compact and detailed CSV generators.
//!
//! - **pentacam_detailed.csv**: per-field: value + source + conf + reliability + flag
//! - **pentacam_compact.csv**: per-field: best value only (ready for clinical use)

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use crate::field_map::{self, ALL_FIELDS};

// ---------------------------------------------------------------------------
// Detailed CSV (was "compact" — full per-field metadata)
// ---------------------------------------------------------------------------

pub fn generate_detailed(raw_csv_path: &Path, output_path: &Path, omit_names: bool) -> Result<u32, String> {
    let groups = load_and_group(raw_csv_path)?;

    let out = File::create(output_path).map_err(|e| format!("Create detailed CSV: {}", e))?;
    let mut writer = BufWriter::new(out);

    // Header
    let mut header = if omit_names {
        "id,birthdate,righteye,exam_date,exam_time,timeoftest,timeoftestEpoch,\
            device_serial,software_version,imagedir,n_source_rows,source_types".to_string()
    } else {
        "id,FamilyName,GivenName,birthdate,righteye,exam_date,exam_time,timeoftest,timeoftestEpoch,\
            device_serial,software_version,imagedir,n_source_rows,source_types".to_string()
    };
    for &field in ALL_FIELDS {
        header.push_str(&format!(",{},{}_source,{}_conf,{}_reliability,{}_flag",
            field, field, field, field, field));
    }
    writeln!(writer, "{}", header).map_err(|e| format!("Write: {}", e))?;

    let mut n_visits = 0u32;
    for (hash, rows, col_index) in &groups {
        let line = format_visit_line(hash, rows, col_index, omit_names, true);
        writeln!(writer, "{}", line).map_err(|e| format!("Write: {}", e))?;
        n_visits += 1;
    }
    writer.flush().map_err(|e| format!("Flush: {}", e))?;
    Ok(n_visits)
}

// ---------------------------------------------------------------------------
// Compact CSV (truly compact — just best values, for clinical use)
// ---------------------------------------------------------------------------

pub fn generate_compact(raw_csv_path: &Path, output_path: &Path, omit_names: bool) -> Result<u32, String> {
    let groups = load_and_group(raw_csv_path)?;

    let out = File::create(output_path).map_err(|e| format!("Create compact CSV: {}", e))?;
    let mut writer = BufWriter::new(out);

    // Header — minimal: identity + timeoftest + just field values
    let mut header = if omit_names {
        "id,birthdate,righteye,timeoftest,timeoftestEpoch,imagedir".to_string()
    } else {
        "id,FamilyName,GivenName,birthdate,righteye,timeoftest,timeoftestEpoch,imagedir".to_string()
    };
    for &field in ALL_FIELDS {
        header.push(',');
        header.push_str(field);
    }
    writeln!(writer, "{}", header).map_err(|e| format!("Write: {}", e))?;

    let mut n_visits = 0u32;
    for (hash, rows, col_index) in &groups {
        let line = format_visit_line(hash, rows, col_index, omit_names, false);
        writeln!(writer, "{}", line).map_err(|e| format!("Write: {}", e))?;
        n_visits += 1;
    }
    writer.flush().map_err(|e| format!("Flush: {}", e))?;
    Ok(n_visits)
}

// ---------------------------------------------------------------------------
// Shared logic
// ---------------------------------------------------------------------------

type GroupedData = Vec<(String, Vec<Vec<String>>, HashMap<String, usize>)>;

fn load_and_group(raw_csv_path: &Path) -> Result<GroupedData, String> {
    let file = File::open(raw_csv_path).map_err(|e| format!("Open raw CSV: {}", e))?;
    let reader = BufReader::new(file);

    let mut lines = reader.lines();
    let header_line = lines.next()
        .ok_or("Empty raw CSV")?
        .map_err(|e| format!("Read header: {}", e))?;
    let headers: Vec<String> = header_line.split(',').map(|s| s.to_string()).collect();
    let col_index: HashMap<String, usize> = headers.iter().enumerate()
        .map(|(i, h)| (h.clone(), i))
        .collect();

    let mut groups: HashMap<String, Vec<Vec<String>>> = HashMap::new();
    let hash_idx = col_index.get("imagedir").copied().unwrap_or(999);

    for line in lines {
        let line = line.map_err(|e| format!("Read: {}", e))?;
        if line.trim().is_empty() { continue; }
        let cols = parse_csv_line(&line);
        let hash = cols.get(hash_idx).cloned().unwrap_or_default();
        if hash.is_empty() { continue; }
        groups.entry(hash).or_default().push(cols);
    }

    Ok(groups.into_iter()
        .map(|(hash, rows)| (hash, rows, col_index.clone()))
        .collect())
}

fn format_visit_line(
    hash: &str,
    rows: &[Vec<String>],
    col_index: &HashMap<String, usize>,
    omit_names: bool,
    detailed: bool,
) -> String {
    let first = &rows[0];
    let get = |col: &str| -> String {
        col_index.get(col).and_then(|&i| first.get(i)).cloned().unwrap_or_default()
    };

    let exam_date = get("exam_date");
    let exam_time = get("exam_time");
    let (timeoftest, epoch) = format_timeoftest(&exam_date, &exam_time);

    let eye_raw = get("eye").to_uppercase();
    let righteye = match eye_raw.as_str() {
        "OD" | "R" => "1", "OS" | "L" => "0", _ => ""
    };

    // Build fixed columns
    let mut line = if detailed {
        let source_types: Vec<String> = rows.iter()
            .filter_map(|r| col_index.get("printout_type").and_then(|&i| r.get(i)).cloned())
            .collect::<std::collections::HashSet<_>>()
            .into_iter().collect();

        if omit_names {
            format!("{},{},{},{},{},{},{},{},{},{},{},\"{}\"",
                csv_escape(&get("id")),
                csv_escape(&get("birthdate")),
                righteye,
                csv_escape(&exam_date), csv_escape(&exam_time),
                csv_escape(&timeoftest),
                epoch.map(|e| e.to_string()).unwrap_or_default(),
                csv_escape(&get("device_serial")), csv_escape(&get("software_version")),
                csv_escape(hash), rows.len(), source_types.join("|"))
        } else {
            format!("{},{},{},{},{},{},{},{},{},{},{},{},{},\"{}\"",
                csv_escape(&get("id")),
                csv_escape(&get("FamilyName")), csv_escape(&get("GivenName")),
                csv_escape(&get("birthdate")),
                righteye,
                csv_escape(&exam_date), csv_escape(&exam_time),
                csv_escape(&timeoftest),
                epoch.map(|e| e.to_string()).unwrap_or_default(),
                csv_escape(&get("device_serial")), csv_escape(&get("software_version")),
                csv_escape(hash), rows.len(), source_types.join("|"))
        }
    } else {
        // Compact — minimal fixed columns
        if omit_names {
            format!("{},{},{},{},{},{}",
                csv_escape(&get("id")),
                csv_escape(&get("birthdate")),
                righteye,
                csv_escape(&timeoftest),
                epoch.map(|e| e.to_string()).unwrap_or_default(),
                csv_escape(hash))
        } else {
            format!("{},{},{},{},{},{},{},{}",
                csv_escape(&get("id")),
                csv_escape(&get("FamilyName")), csv_escape(&get("GivenName")),
                csv_escape(&get("birthdate")),
                righteye,
                csv_escape(&timeoftest),
                epoch.map(|e| e.to_string()).unwrap_or_default(),
                csv_escape(hash))
        }
    };

    // Per-field values
    for &field in ALL_FIELDS {
        let best = select_best_value(field, rows, col_index);
        if detailed {
            match best {
                Some(bv) => line.push_str(&format!(",{},{},{:.4},{:.3},{}",
                    bv.value, bv.source, bv.confidence, bv.reliability, bv.flag)),
                None => line.push_str(",,,,,"),
            }
        } else {
            // Compact: just the value
            match best {
                Some(bv) => line.push_str(&format!(",{}", bv.value)),
                None => line.push(','),
            }
        }
    }

    line
}

struct BestValue {
    value: f64,
    source: String,
    confidence: f32,
    reliability: f32,
    flag: String,
}

fn select_best_value(
    field: &str,
    rows: &[Vec<String>],
    col_index: &HashMap<String, usize>,
) -> Option<BestValue> {
    let field_idx = *col_index.get(field)?;
    let conf_col = format!("{}_Paddle_conf", field);
    let conf_idx = col_index.get(conf_col.as_str()).copied();
    let type_idx = *col_index.get("printout_type")?;

    let reliability = field_map::field_reliability(field);

    let mut candidates: Vec<(f64, String, f32)> = Vec::new();
    for row in rows {
        let val_str = row.get(field_idx).map(|s| s.as_str()).unwrap_or("");
        if val_str.is_empty() { continue; }
        let val: f64 = match val_str.parse() { Ok(v) => v, Err(_) => continue };
        let source = row.get(type_idx).cloned().unwrap_or_default();
        let conf: f32 = conf_idx.and_then(|i| row.get(i)).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        candidates.push((val, source, conf));
    }

    if candidates.is_empty() { return None; }

    let is_hwtw = field == "HWTW";
    let sr = candidates.iter().find(|(_, src, _)| src == "SR");
    let spr = candidates.iter().find(|(_, src, _)| src == "SPR");
    let best_ocr = candidates.iter()
        .filter(|(_, src, _)| src != "SR" && src != "SPR")
        .max_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));

    let (value, source, confidence) = if is_hwtw {
        spr.or(sr).or(best_ocr)
    } else {
        sr.or(spr).or(best_ocr)
    }.map(|(v, s, c)| (*v, s.clone(), *c))?;

    let mut flag = String::new();
    if let (Some(sr_val), Some(ocr_val)) = (sr, best_ocr) {
        let delta = (sr_val.0 - ocr_val.0).abs();
        if delta > cross_validation_threshold(field) {
            flag = "SR_OCR_MISMATCH".to_string();
        }
    }
    if flag.is_empty() && reliability < 0.96 { flag = "REVIEW".to_string(); }
    if flag.is_empty() && confidence < 0.85 && confidence > 0.0 { flag = "LOW_CONF".to_string(); }

    Some(BestValue { value, source, confidence, reliability, flag })
}

fn cross_validation_threshold(field: &str) -> f64 {
    if field.contains("Df") || field.contains("Db") || field.contains("Dp")
        || field.contains("Dt") || field.contains("Da") || field.contains("D_final")
        || field.contains("Prog_") {
        0.02
    } else if field.contains("Pachy") || field.contains("Thinnest") {
        2.0
    } else if field.contains("ARTmax") {
        5.0
    } else if field.contains("Axis") {
        1.0
    } else if field.contains("K1") || field.contains("K2") || field.contains("Km")
        || field.contains("KMax") || field.contains("Kmax") {
        0.1
    } else {
        0.05
    }
}

fn format_timeoftest(exam_date: &str, exam_time: &str) -> (String, Option<i64>) {
    if exam_date.len() < 8 { return (String::new(), None); }
    let yy = &exam_date[2..4];
    let mmdd = &exam_date[4..8];
    let time_part = if exam_time.len() >= 6 { &exam_time[0..6] } else { "000000" };
    let timeoftest = format!("{}{}{}", yy, mmdd, time_part);

    let y: i32 = exam_date[0..4].parse().unwrap_or(2000);
    let m: u32 = exam_date[4..6].parse().unwrap_or(1);
    let d: u32 = exam_date[6..8].parse().unwrap_or(1);
    let h: u32 = if exam_time.len() >= 2 { exam_time[0..2].parse().unwrap_or(0) } else { 0 };
    let min: u32 = if exam_time.len() >= 4 { exam_time[2..4].parse().unwrap_or(0) } else { 0 };
    let s: u32 = if exam_time.len() >= 6 { exam_time[4..6].parse().unwrap_or(0) } else { 0 };

    use chrono::{NaiveDate, NaiveTime, NaiveDateTime};
    let epoch = NaiveDate::from_ymd_opt(y, m, d)
        .and_then(|date| NaiveTime::from_hms_opt(h, min, s).map(|time| NaiveDateTime::new(date, time)))
        .map(|dt| dt.and_utc().timestamp());

    (timeoftest, epoch)
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') { current.push('"'); chars.next(); }
                else { in_quotes = false; }
            } else { current.push(ch); }
        } else {
            match ch {
                ',' => { fields.push(current.clone()); current.clear(); }
                '"' => { in_quotes = true; }
                _ => { current.push(ch); }
            }
        }
    }
    fields.push(current);
    fields
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else { s.to_string() }
}
