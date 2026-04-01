//! DICOM tag extraction for Pentacam files.

use pentacam_types::{DicomMeta, DicomSrValues, Laterality};
use std::path::Path;

use dicom_object::open_file;
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

/// Extract Structured Report values. TODO: implement SR walking for dicom-object 0.6.
pub fn extract_sr(_path: &Path) -> Result<Option<DicomSrValues>, String> {
    // SR extraction requires walking the ContentSequence tree.
    // Deferred — the OCR pipeline provides the same values.
    Ok(None)
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
