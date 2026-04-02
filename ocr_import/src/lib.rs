//! OCR-based extraction from Pentacam PDF printouts and images.
//!
//! Pipeline per page:
//! 1. Render PDF page to 300 DPI PNG
//! 2. Full-page OCR via oar-ocr
//! 3. Detect printout type
//! 4. Locate fields (label matching + affine fit + positional fallback)
//! 5. Post-processing corrections (domain knowledge)
//! 6. Quality gate (min 25 located fields)
//! 7. Optionally save crops for debugging / re-reading

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

use image::GenericImageView;
use pentacam_types::{PrintoutType, PrintoutResult, QaStatus};
use field_locate::LocatedField;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Minimum fields to consider extraction trustworthy.
const MIN_LOCATED_FIELDS: usize = 25;

/// Options for saving debug artifacts.
pub struct SaveOptions {
    /// Directory to save rendered pages and crops.
    pub output_dir: PathBuf,
    /// If true, save the rendered full-page PNG.
    pub save_pages: bool,
    /// If true, save individual field crops.
    pub save_crops: bool,
    /// Padding around each crop in pixels.
    pub crop_pad: u32,
}

/// Process a single rendered page image through the full OCR pipeline.
///
/// Returns a PrintoutResult with all extracted field values, or None if the
/// printout type is not recognized.
pub fn process_page(
    img_path: &Path,
    source_file: &Path,
    page_number: usize,
) -> Option<PrintoutResult> {
    process_page_with_options(img_path, source_file, page_number, None)
}

/// Process a page with optional crop saving.
pub fn process_page_with_options(
    img_path: &Path,
    source_file: &Path,
    page_number: usize,
    save: Option<&SaveOptions>,
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

    // Step 5: Crop-based re-reading for missing fields
    // For fields in the archetype that weren't found by full-page OCR,
    // crop the predicted position, preprocess, and re-OCR.
    let archetype = field_locate::archetype_for(&printout_type);
    let fit = field_locate::fit_affine(&labeled, archetype);
    if fit.n_inliers >= 5 {
        crop_rescue_missing(&mut labeled, img_path, archetype, &fit);
    }

    // Step 5b: Post-processing again (crop rescue may have added raw values needing fixes)
    postprocess::apply_corrections(&mut labeled);

    // Step 6: Save crops if requested
    if let Some(opts) = save {
        save_artifacts(img_path, source_file, page_number, &printout_type, &labeled, &items, opts);
    }

    // Step 7: Quality gate
    let n_located = labeled.len();
    let qa_status = if n_located >= MIN_LOCATED_FIELDS {
        QaStatus::Ok
    } else {
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
    // (after all extraction steps including crop rescue)
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

/// Crop-based re-reading for fields missing from full-page OCR.
///
/// For each archetype field not yet found, crop the predicted position from
/// the page image, preprocess (3x upscale + fill_hollow_digits), run OCR
/// on the crop, and extract the value.
fn crop_rescue_missing(
    labeled: &mut HashMap<String, LocatedField>,
    img_path: &Path,
    archetype: &[(&str, f32, f32)],
    fit: &field_locate::AffineFit,
) {
    // Only load the image if there are missing fields
    let missing: Vec<(&str, f32, f32)> = archetype.iter()
        .filter(|&&(name, _, _)| !labeled.contains_key(name))
        .map(|&(name, cy, cx)| (name, cy, cx))
        .collect();

    if missing.is_empty() {
        return;
    }

    let img = match image::open(img_path) {
        Ok(i) => i,
        Err(_) => return,
    };

    let (iw, ih) = img.dimensions();

    for (field_name, cy_ref, cx_ref) in missing {
        // Wider crop for Axis fields (includes "(steep)" text left of value)
        let is_axis = field_name.starts_with("Axis");
        let crop_half_w: u32 = if is_axis { 180 } else { 100 };
        let crop_half_h: u32 = if is_axis { 40 } else { 30 };
        let cy_pred = (fit.alpha * cy_ref as f64 + fit.beta) as f32;
        let cx_pred = cx_ref + fit.delta_cx as f32;

        // Bounds check
        let cx_u = cx_pred as u32;
        let cy_u = cy_pred as u32;
        if cx_u < crop_half_w || cy_u < crop_half_h
            || cx_u + crop_half_w >= iw || cy_u + crop_half_h >= ih
        {
            continue;
        }

        // Crop the predicted region
        let crop = img.crop_imm(
            cx_u - crop_half_w,
            cy_u - crop_half_h,
            crop_half_w * 2,
            crop_half_h * 2,
        );

        // Preprocess: 3x upscale + fill hollow digits
        let processed = field_read::preprocess_crop(&crop);

        // Save to temp file and run OCR
        let tmp_path = PathBuf::from(format!("/tmp/_crop_rescue_{}.png", field_name));
        if processed.save(&tmp_path).is_err() {
            continue;
        }
        if let Ok(crop_items) = ocr_engine::run_full_page(&tmp_path) {
            if let Some((val, conf)) = field_read::extract_best_numeric(&crop_items) {
                labeled.insert(field_name.to_string(), LocatedField {
                    value: val,
                    conf,
                    cx: cx_pred,
                    cy: cy_pred,
                    raw_text: format!("[crop-rescue] {}",
                        crop_items.iter()
                            .map(|i| i.text.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")),
                });
            }
        }

        let _ = std::fs::remove_file(&tmp_path);
    }
}

/// Save rendered page and/or individual field crops.
fn save_artifacts(
    img_path: &Path,
    source_file: &Path,
    page_number: usize,
    printout_type: &PrintoutType,
    labeled: &HashMap<String, LocatedField>,
    items: &[ocr_engine::OcrItem],
    opts: &SaveOptions,
) {
    use std::fs;
    let _ = fs::create_dir_all(&opts.output_dir);

    let stem = source_file.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .replace('.', "_");
    let prefix = format!("{}_p{}", stem, page_number);

    // Save full page
    if opts.save_pages {
        let dst = opts.output_dir.join(format!("{}_page.png", prefix));
        let _ = fs::copy(img_path, &dst);
    }

    // Save crops
    if opts.save_crops {
        if let Ok(img) = image::open(img_path) {
            // Build OCR bounding boxes for snapping
            let ocr_boxes: Vec<field_read::OcrBox> = items.iter().map(|item| {
                // Estimate a box from centroid (crude — real pipeline would use actual bbox)
                field_read::OcrBox {
                    x1: item.cx - 60.0,
                    y1: item.cy - 20.0,
                    x2: item.cx + 60.0,
                    y2: item.cy + 20.0,
                }
            }).collect();

            for (name, loc) in labeled {
                let crop = field_read::get_tight_crop(&img, loc.cx, loc.cy, &ocr_boxes, opts.crop_pad);
                let crop_path = opts.output_dir.join(format!("{}_{}.png", prefix, name));
                let _ = crop.save(&crop_path);
            }
        }
    }
}
