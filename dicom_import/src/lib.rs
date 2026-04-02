//! DICOM tag extraction for Pentacam files.

use pentacam_types::{DicomMeta, DicomSrValues, Laterality};
use std::path::Path;

use dicom_object::{open_file, InMemDicomObject};
use dicom_core::Tag;

/// Extract standard DICOM metadata tags.
pub fn extract_metadata(path: &Path) -> Result<DicomMeta, String> {
    let obj = open_file(path).map_err(|e| format!("Failed to open DICOM: {}", e))?;

    let get_str = |tag: Tag| -> Option<String> {
        obj.element(tag)
            .ok()
            .and_then(|e| e.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let laterality = get_str(Tag(0x0020, 0x0062))
        .or_else(|| get_str(Tag(0x0020, 0x0060)))
        .and_then(|s| parse_laterality(&s));

    Ok(DicomMeta {
        patient_id: get_str(Tag(0x0010, 0x0020)),
        patient_name: get_str(Tag(0x0010, 0x0010)),
        date_of_birth: get_str(Tag(0x0010, 0x0030)),
        sex: get_str(Tag(0x0010, 0x0040)),
        exam_date: get_str(Tag(0x0008, 0x0023))
            .or_else(|| get_str(Tag(0x0008, 0x0020))),
        exam_time: get_str(Tag(0x0008, 0x0033))
            .or_else(|| get_str(Tag(0x0008, 0x0030))),
        laterality,
        series_number: get_str(Tag(0x0020, 0x0011)),
        instance_number: get_str(Tag(0x0020, 0x0013)),
        software_version: get_str(Tag(0x0018, 0x1020)),
        device_serial: get_str(Tag(0x0018, 0x1000)),
    })
}

/// Extract Structured Report numeric values from ContentSequence (0040,a730).
/// Returns None if the SR tag is not present (firmware <1.30).
pub fn extract_sr(path: &Path) -> Result<Option<DicomSrValues>, String> {
    let obj = open_file(path).map_err(|e| format!("Failed to open DICOM: {}", e))?;

    let elem = match obj.element(Tag(0x0040, 0xa730)) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    let items = match elem.value().items() {
        Some(items) => items,
        None => return Ok(None),
    };

    let mut results = DicomSrValues::new();
    for item in items {
        walk_sr_item(item, &mut results);
    }

    if results.is_empty() { Ok(None) } else { Ok(Some(results)) }
}

fn walk_sr_item(item: &InMemDicomObject, results: &mut DicomSrValues) {
    let get_str = |obj: &InMemDicomObject, tag: Tag| -> Option<String> {
        obj.element(tag).ok().and_then(|e| e.to_str().ok()).map(|s| s.trim().to_string())
    };

    // Check ValueType == "NUM"
    let value_type = get_str(item, Tag(0x0040, 0xa040)).unwrap_or_default();
    if value_type == "NUM" {
        // Get CodeValue from ConceptNameCodeSequence
        let code = item.element(Tag(0x0040, 0xa043)).ok()
            .and_then(|e| e.value().items())
            .and_then(|seq| seq.first())
            .and_then(|code_item| get_str(code_item, Tag(0x0008, 0x0100)));

        // Get NumericValue from MeasuredValueSequence
        let value = item.element(Tag(0x0040, 0xa300)).ok()
            .and_then(|e| e.value().items())
            .and_then(|seq| seq.first())
            .and_then(|mv_item| get_str(mv_item, Tag(0x0040, 0xa30a)))
            .and_then(|s| s.parse::<f64>().ok());

        if let (Some(code), Some(val)) = (code, value) {
            results.insert(format!("DICOM_{}", code), val);
        }
    }

    // Recurse into nested ContentSequence
    if let Ok(nested) = item.element(Tag(0x0040, 0xa730)) {
        if let Some(sub_items) = nested.value().items() {
            for sub_item in sub_items {
                walk_sr_item(sub_item, results);
            }
        }
    }
}

/// Extract raw PDF bytes from EncapsulatedDocument tag (0042,0011).
pub fn extract_pdf_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let obj = open_file(path).map_err(|e| format!("Failed to open DICOM: {}", e))?;
    match obj.element(Tag(0x0042, 0x0011)) {
        Ok(elem) => {
            let bytes = elem.to_bytes().map_err(|e| format!("PDF bytes: {}", e))?;
            Ok(Some(bytes.to_vec()))
        }
        Err(_) => Ok(None),
    }
}

/// Parse laterality from DICOM tag string.
pub fn parse_laterality(s: &str) -> Option<Laterality> {
    match s.trim().to_uppercase().as_str() {
        "R" | "OD" => Some(Laterality::OD),
        "L" | "OS" => Some(Laterality::OS),
        _ => None,
    }
}
