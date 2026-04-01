//! DICOM tag extraction for Pentacam files.
//!
//! Reads standard DICOM metadata, Structured Report (SR) values,
//! and detects embedded PDFs and private-tag blobs for downstream modules.

use pentacam_types::{DicomMeta, DicomSrValues, Laterality};
use std::path::Path;

/// Extract standard DICOM metadata tags.
pub fn extract_metadata(path: &Path) -> Result<DicomMeta, String> {
    let _ = path;
    todo!("Read DICOM tags: patient ID, name, DOB, sex, exam date/time, laterality, device info")
}

/// Extract Structured Report numeric values from ContentSequence (0040,a730).
/// Returns None if the SR tag is not present (firmware <1.30).
pub fn extract_sr(path: &Path) -> Result<Option<DicomSrValues>, String> {
    let _ = path;
    todo!("Walk SR ContentSequence, collect NUM items as DICOM_<CodeValue> -> f64")
}

/// Extract raw PDF bytes from EncapsulatedDocument tag (0042,0011).
/// Returns None if no embedded PDF is present.
pub fn extract_pdf_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let _ = path;
    todo!("Read tag (0042,0011) and return raw bytes")
}

/// Extract the private-tag blob (38,400 bytes) if present.
/// Returns None if the private tag is not found.
pub fn extract_blob(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let _ = path;
    todo!("Read Pentacam private tag blob")
}

/// Parse laterality from DICOM tag string ("R"/"L"/"OD"/"OS").
pub fn parse_laterality(s: &str) -> Option<Laterality> {
    match s.trim().to_uppercase().as_str() {
        "R" | "OD" => Some(Laterality::OD),
        "L" | "OS" => Some(Laterality::OS),
        _ => None,
    }
}
