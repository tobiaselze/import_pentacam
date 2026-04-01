//! Tier 1 extraction from Pentacam private-tag blobs.
//!
//! The 38,400-byte binary blob contains proprietary measurement data.
//! Only exact, deterministic readouts are extracted:
//! - CorneaDia / HWTW (mm): int16 at offset 0x05BA / 100
//! - ACD (mm): a1[0][col] - a1[0][4], layout-dependent
//!
//! Tier 2 (formulas) and Tier 3 (GBR models) are intentionally omitted —
//! those fields are available via OCR at 99%+ accuracy.

use pentacam_types::{BlobExact, Laterality};

const ARRAY1_BASE: usize = 0x080C;
const ARRAY1_STRIDE: usize = 48; // 6 x f64

const OFFSET_EYE: usize = 0x0500;
const OFFSET_EXAM_FILE: usize = 0x0420;
const OFFSET_CORNEA_DIA: usize = 0x05BA;
const OFFSET_FW_MARKER: usize = 0x05BE;
const OFFSET_FORMAT_VERSION: usize = 0x0690;

const EXPECTED_BLOB_SIZE: usize = 38_400;

/// Read a little-endian f64 from array 1 at (record, column).
fn a1(blob: &[u8], rec: usize, col: usize) -> f64 {
    let offset = ARRAY1_BASE + rec * ARRAY1_STRIDE + col * 8;
    f64::from_le_bytes(blob[offset..offset + 8].try_into().unwrap())
}

/// Extract Tier 1 exact values from a 38,400-byte blob.
pub fn extract_exact(blob: &[u8]) -> Result<BlobExact, String> {
    if blob.len() != EXPECTED_BLOB_SIZE {
        return Err(format!(
            "Blob size mismatch: expected {}, got {}",
            EXPECTED_BLOB_SIZE,
            blob.len()
        ));
    }

    // Eye
    let eye_code = u16::from_le_bytes(blob[OFFSET_EYE..OFFSET_EYE + 2].try_into().unwrap());
    let eye = match eye_code {
        1 => Laterality::OD,
        0 => Laterality::OS,
        _ => return Err(format!("Unknown eye code: {}", eye_code)),
    };

    // Exam file name
    let exam_file_bytes = &blob[OFFSET_EXAM_FILE..OFFSET_EXAM_FILE + 12];
    let exam_file = exam_file_bytes
        .split(|&b| b == 0)
        .next()
        .unwrap_or(b"")
        .iter()
        .map(|&b| b as char)
        .collect::<String>();

    // Blob format version
    let blob_format =
        u16::from_le_bytes(blob[OFFSET_FORMAT_VERSION..OFFSET_FORMAT_VERSION + 2].try_into().unwrap());
    let shifted = blob_format == 6;

    // CorneaDia / HWTW: range-check (valid = 9-15 mm)
    let cornea_dia_raw =
        u16::from_le_bytes(blob[OFFSET_CORNEA_DIA..OFFSET_CORNEA_DIA + 2].try_into().unwrap());
    let cornea_dia_mm = cornea_dia_raw as f64 / 100.0;
    let cornea_dia = if (9.0..15.0).contains(&cornea_dia_mm) {
        Some(cornea_dia_mm)
    } else {
        None
    };

    // ACD: exact identity, layout-dependent
    let acd = if shifted {
        a1(blob, 0, 0) - a1(blob, 0, 4)
    } else {
        a1(blob, 0, 1) - a1(blob, 0, 4)
    };

    Ok(BlobExact {
        eye,
        cornea_dia_mm: cornea_dia,
        acd_mm: acd,
        exam_file,
        blob_format,
    })
}
