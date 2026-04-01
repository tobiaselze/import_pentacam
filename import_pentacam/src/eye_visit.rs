//! Eye-visit accumulation and merge logic.

use pentacam_types::{EyeVisit, EyeVisitKey};
use std::collections::HashMap;
use std::path::Path;

/// Insert or merge an eye-visit into the global map.
/// If the key already exists, merge new data into the existing record.
pub fn upsert(
    visits: &mut HashMap<EyeVisitKey, EyeVisit>,
    key: EyeVisitKey,
    visit: EyeVisit,
) {
    visits
        .entry(key.clone())
        .and_modify(|existing| merge(existing, &visit))
        .or_insert(visit);
}

/// Merge new data into an existing eye-visit record.
fn merge(existing: &mut EyeVisit, new: &EyeVisit) {
    // DICOM meta: keep first non-None
    if existing.dicom_meta.is_none() {
        existing.dicom_meta = new.dicom_meta.clone();
    }
    if existing.dicom_sr.is_none() {
        existing.dicom_sr = new.dicom_sr.clone();
    }
    if existing.blob.is_none() {
        existing.blob = new.blob.clone();
    }
    if existing.pdf_demographics.is_none() {
        existing.pdf_demographics = new.pdf_demographics.clone();
    }

    // Append new printouts (avoid duplicates by checking source_file + page_number)
    for printout in &new.printouts {
        let is_dup = existing.printouts.iter().any(|p| {
            p.source_file == printout.source_file && p.page_number == printout.page_number
        });
        if !is_dup {
            existing.printouts.push(printout.clone());
        }
    }

    // Track all source files
    for f in &new.source_files {
        if !existing.source_files.contains(f) {
            existing.source_files.push(f.clone());
        }
    }
}

/// Generate the output image directory path for an eye-visit.
pub fn image_dir(output_base: &Path, key: &EyeVisitKey) -> std::path::PathBuf {
    output_base.join("images").join(key.dir_hash())
}
