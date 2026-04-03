//! DICOM tag extraction for Pentacam files.
//!
//! All extraction functions accept `&InMemDicomObject` — the caller opens
//! the file once and passes the reference to each function.

use pentacam_types::{DicomMeta, DicomSrValues, Laterality};
use std::path::Path;

pub use dicom_object::{open_file, InMemDicomObject, FileDicomObject};
use dicom_core::Tag;

/// Open a DICOM file and return the in-memory object.
pub fn open(path: &Path) -> Result<FileDicomObject<InMemDicomObject>, String> {
    open_file(path).map_err(|e| format!("Failed to open DICOM: {}", e))
}

/// Extract standard DICOM metadata tags.
pub fn extract_metadata(obj: &InMemDicomObject) -> DicomMeta {
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

    DicomMeta {
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
    }
}

/// Extract Structured Report numeric values from ContentSequence (0040,a730).
pub fn extract_sr(obj: &InMemDicomObject) -> Option<DicomSrValues> {
    let elem = obj.element(Tag(0x0040, 0xa730)).ok()?;
    let items = elem.value().items()?;

    let mut results = DicomSrValues::new();
    for item in items {
        walk_sr_item(item, &mut results);
    }

    if results.is_empty() { None } else { Some(results) }
}

fn walk_sr_item(item: &InMemDicomObject, results: &mut DicomSrValues) {
    let get_str = |obj: &InMemDicomObject, tag: Tag| -> Option<String> {
        obj.element(tag).ok().and_then(|e| e.to_str().ok()).map(|s| s.trim().to_string())
    };

    let value_type = get_str(item, Tag(0x0040, 0xa040)).unwrap_or_default();
    if value_type == "NUM" {
        let code = item.element(Tag(0x0040, 0xa043)).ok()
            .and_then(|e| e.value().items())
            .and_then(|seq| seq.first())
            .and_then(|code_item| get_str(code_item, Tag(0x0008, 0x0100)));

        let value = item.element(Tag(0x0040, 0xa300)).ok()
            .and_then(|e| e.value().items())
            .and_then(|seq| seq.first())
            .and_then(|mv_item| get_str(mv_item, Tag(0x0040, 0xa30a)))
            .and_then(|s| s.parse::<f64>().ok());

        if let (Some(code), Some(val)) = (code, value) {
            results.insert(format!("DICOM_{}", code), val);
        }
    }

    if let Ok(nested) = item.element(Tag(0x0040, 0xa730)) {
        if let Some(sub_items) = nested.value().items() {
            for sub_item in sub_items {
                walk_sr_item(sub_item, results);
            }
        }
    }
}

/// Extract raw PDF bytes from EncapsulatedDocument tag (0042,0011).
pub fn extract_pdf_bytes(obj: &InMemDicomObject) -> Option<Vec<u8>> {
    let elem = obj.element(Tag(0x0042, 0x0011)).ok()?;
    let bytes = elem.to_bytes().ok()?;
    Some(bytes.to_vec())
}

/// Extract the private-tag blob (38,400 bytes) at (0029,1010).
pub fn extract_blob(obj: &InMemDicomObject) -> Option<Vec<u8>> {
    let elem = obj.element(Tag(0x0029, 0x1010)).ok()?;
    let bytes = elem.to_bytes().ok()?;
    if bytes.len() >= 38_400 {
        Some(bytes[..38_400].to_vec())
    } else {
        None
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
