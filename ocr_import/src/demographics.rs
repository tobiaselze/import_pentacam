//! Extract patient demographics from PDF/image printout header.
//!
//! Used when DICOM metadata is not available (standalone PDF/image input).
//! Matches header labels: "Last Name:", "First Name:", "ID:", "Date of Birth:",
//! "Exam Date:", "Eye:", and finds values to their right.

use pentacam_types::{Laterality, PdfDemographics};
use super::ocr_engine::OcrItem;

/// Extract patient name, ID, DOB, exam date, eye from the printout header region.
///
/// Header labels are consistently in the top ~15% of the page (cy < page_height * 0.15),
/// left side (cx < 700). Values appear to the right of labels on the same line (similar cy).
pub fn extract_from_header(items: &[OcrItem]) -> Option<PdfDemographics> {
    let mut demo = PdfDemographics::default();
    let mut found_any = false;

    // Only consider items in the header region (top portion, left side)
    // Labels are at cx < 200, values at cx ~ 250-600
    for i in 0..items.len() {
        let label = items[i].text.trim();
        let label_cy = items[i].cy;

        // Find value: the next item to the right on approximately the same line
        let value = find_value_right(items, items[i].cx, label_cy);

        match label {
            s if s.eq_ignore_ascii_case("Last Name:") => {
                if let Some(v) = value {
                    demo.patient_name = Some(v);
                    found_any = true;
                }
            }
            s if s.eq_ignore_ascii_case("First Name:") => {
                if let Some(v) = value {
                    // Append to patient_name as "LastName^FirstName"
                    demo.patient_name = Some(match &demo.patient_name {
                        Some(last) => format!("{}^{}", last, v),
                        None => format!("^{}", v),
                    });
                    found_any = true;
                }
            }
            s if s.eq_ignore_ascii_case("ID:") || s.eq_ignore_ascii_case("Pat-ID:") => {
                if let Some(v) = value {
                    demo.patient_id = Some(v);
                    found_any = true;
                }
            }
            s if s.eq_ignore_ascii_case("Date of Birth:") || s.eq_ignore_ascii_case("DOB:") => {
                if let Some(v) = value {
                    demo.date_of_birth = Some(normalize_date(&v));
                    found_any = true;
                }
            }
            s if s.eq_ignore_ascii_case("Exam Date:") || s.eq_ignore_ascii_case("Exam:") => {
                if let Some(v) = value {
                    demo.exam_date = Some(normalize_date(&v));
                    found_any = true;
                }
            }
            s if s.eq_ignore_ascii_case("Eye:") => {
                if let Some(v) = value {
                    let vu = v.to_uppercase();
                    demo.eye = if vu.contains("RIGHT") || vu == "OD" || vu == "R" {
                        Some(Laterality::OD)
                    } else if vu.contains("LEFT") || vu == "OS" || vu == "L" {
                        Some(Laterality::OS)
                    } else {
                        None
                    };
                    if demo.eye.is_some() { found_any = true; }
                }
            }
            s if s.eq_ignore_ascii_case("Time:") => {
                if let Some(v) = value {
                    demo.exam_time = Some(normalize_time(&v));
                    found_any = true;
                }
            }
            _ => {}
        }
    }

    if found_any { Some(demo) } else { None }
}

/// Find the nearest OCR item to the right of a label on the same line.
fn find_value_right(items: &[OcrItem], label_cx: f32, label_cy: f32) -> Option<String> {
    let mut best: Option<(f32, &str)> = None;
    for item in items {
        // Must be to the right (cx > label_cx + 50) and on same line (cy within ±20)
        if item.cx > label_cx + 50.0
            && (item.cy - label_cy).abs() < 20.0
            && item.cx < label_cx + 500.0  // not too far right
        {
            let dist = item.cx - label_cx;
            if best.is_none() || dist < best.unwrap().0 {
                let text = item.text.trim();
                if !text.is_empty() {
                    best = Some((dist, text));
                }
            }
        }
    }
    best.map(|(_, text)| text.to_string())
}

/// Normalize date from various formats to YYYYMMDD.
/// Handles: "01.31.2017", "01/31/2017", "2017-01-31", "31.01.2017" etc.
fn normalize_date(s: &str) -> String {
    let s = s.trim();

    // Try MM.DD.YYYY or MM/DD/YYYY
    if let Some((m, d, y)) = parse_date_mdy(s) {
        return format!("{:04}{:02}{:02}", y, m, d);
    }

    // Try YYYY-MM-DD
    if s.len() == 10 && s.chars().nth(4) == Some('-') {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() == 3 {
            if let (Ok(y), Ok(m), Ok(d)) = (
                parts[0].parse::<u32>(),
                parts[1].parse::<u32>(),
                parts[2].parse::<u32>(),
            ) {
                return format!("{:04}{:02}{:02}", y, m, d);
            }
        }
    }

    // Already YYYYMMDD?
    if s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()) {
        return s.to_string();
    }

    s.to_string()
}

fn parse_date_mdy(s: &str) -> Option<(u32, u32, u32)> {
    let sep = if s.contains('.') { '.' } else if s.contains('/') { '/' } else { return None };
    let parts: Vec<&str> = s.split(sep).collect();
    if parts.len() != 3 { return None; }
    let a: u32 = parts[0].parse().ok()?;
    let b: u32 = parts[1].parse().ok()?;
    let c: u32 = parts[2].parse().ok()?;

    if c > 1900 {
        // MM.DD.YYYY or DD.MM.YYYY — assume MM.DD.YYYY (US format, matches Pentacam)
        Some((a, b, c))
    } else if a > 1900 {
        // YYYY.MM.DD
        Some((b, c, a))
    } else {
        None
    }
}

/// Normalize time from "HH:MM:SS" to "HHMMSS".
fn normalize_time(s: &str) -> String {
    s.replace(':', "").replace('.', "")
}
