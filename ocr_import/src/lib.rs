//! OCR-based extraction from Pentacam PDF printouts and images.
//!
//! Pipeline per page:
//! 1. Render PDF page to 300 DPI PNG
//! 2. Full-page OCR via oar-ocr
//! 3. Detect printout type
//! 4. Locate fields (label matching + affine fit + positional fallback)
//! 5. Crop each field, preprocess, re-OCR for improved accuracy
//! 6. Post-processing corrections (domain knowledge)
//! 7. Quality gate (min 25 located fields)

pub mod ocr_engine;
pub mod printout_detect;
pub mod field_locate;
pub mod label_match;
pub mod field_read;
pub mod four_maps;
pub mod topometric;
pub mod belin;
pub mod demographics;
pub mod postprocess;
pub mod render;

use pentacam_types::{PrintoutType, PrintoutResult, QaStatus};
use field_locate::LocatedField;
use std::collections::HashMap;
use std::path::Path;

/// Minimum fields to consider extraction trustworthy.
const MIN_LOCATED_FIELDS: usize = 25;

/// Process a single rendered page image through the full OCR pipeline.
///
/// Returns a PrintoutResult with all extracted field values, or None if the
/// printout type is not recognized.
pub fn process_page(
    img_path: &Path,
    source_file: &Path,
    page_number: usize,
) -> Option<PrintoutResult> {
    // Step 1: Full-page OCR
    let items = ocr_engine::run_full_page(img_path).ok()?;

    // Step 2: Detect printout type
    let printout_type = printout_detect::detect_printout_type(&items)?;

    // Step 3: Extract fields based on printout type
    let mut labeled = match &printout_type {
        PrintoutType::BelinAmbrosio => belin::extract(&items),
        _ => {
            let is_topo = matches!(printout_type, PrintoutType::TopometricKcStaging);
            label_match::match_labels(&items, is_topo)
        }
    };

    // Step 4: Post-processing corrections
    postprocess::apply_corrections(&mut labeled);

    // Step 5: Quality gate
    let n_located = labeled.len();
    let qa_status = if n_located >= MIN_LOCATED_FIELDS {
        QaStatus::Ok
    } else {
        // Belin has only 20 fields — use a lower threshold
        let min = match &printout_type {
            PrintoutType::BelinAmbrosio => 8,
            _ => MIN_LOCATED_FIELDS,
        };
        if n_located >= min {
            QaStatus::Ok
        } else {
            QaStatus::Incomplete {
                reason: format!("too few fields: {}/{}", n_located, min),
            }
        }
    };

    // Convert to output format
    let mut fields = HashMap::new();
    let mut confidences = HashMap::new();
    for (name, loc) in &labeled {
        fields.insert(name.clone(), loc.value);
        confidences.insert(name.clone(), loc.conf);
    }

    Some(PrintoutResult {
        printout_type,
        source_file: source_file.to_path_buf(),
        page_number,
        fields,
        confidences,
        qa_status,
    })
}
