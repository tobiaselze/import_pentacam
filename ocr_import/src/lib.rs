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
pub mod extract_maps;

use image::GenericImageView;
use pentacam_types::{PrintoutType, PrintoutResult, QaStatus};
use field_locate::LocatedField;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Per-process temp directory for all intermediate files (crops, renders, etc.).
/// Created once on first use, cleaned up on process exit (unless debug mode).
static TEMP_DIR: Lazy<PathBuf> = Lazy::new(|| {
    let dir = std::env::temp_dir().join(format!("pentacam_ocr_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Failed to create temp directory");
    dir
});

/// Get a temp file path within the session temp directory.
pub fn temp_path(name: &str) -> PathBuf {
    TEMP_DIR.join(name)
}

/// Clean up the session temp directory.
pub fn cleanup_temp() {
    if let Err(e) = std::fs::remove_dir_all(TEMP_DIR.as_path()) {
        eprintln!("Warning: failed to clean up temp dir {}: {}", TEMP_DIR.display(), e);
    }
}

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
    let t_start = std::time::Instant::now();
    let items = match ocr_engine::run_full_page(img_path) {
        Ok(items) => items,
        Err(e) => {
            eprintln!("  WARNING: OCR failed on {}: {}", img_path.display(), e);
            return None;
        }
    };
    let t_fullpage = t_start.elapsed();

    process_page_inner(img_path, source_file, page_number, items, save, Some(t_fullpage))
}

/// Process a page using pre-computed OCR items (avoids double OCR run).
///
/// `img_path` is still needed for crop rescue (which loads image regions from disk).
/// `items` are the OCR results from a prior `run_full_page` or `run_full_page_mem` call.
pub fn process_page_with_items(
    img_path: &Path,
    source_file: &Path,
    page_number: usize,
    items: Vec<ocr_engine::OcrItem>,
) -> Option<PrintoutResult> {
    process_page_inner(img_path, source_file, page_number, items, None, None)
}

/// Shared implementation for page processing after OCR items are available.
fn process_page_inner(
    img_path: &Path,
    source_file: &Path,
    page_number: usize,
    items: Vec<ocr_engine::OcrItem>,
    save: Option<&SaveOptions>,
    t_fullpage: Option<std::time::Duration>,
) -> Option<PrintoutResult> {
    let t_start = std::time::Instant::now();

    // Step 2: Detect printout type
    let printout_type = match printout_detect::detect_printout_type(&items) {
        Some(pt) => pt,
        None => {
            eprintln!("  WARNING: printout type not recognized on {} ({} OCR items)",
                img_path.display(), items.len());
            return None;
        }
    };

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

    // Step 5: Routine re-crop of all found Belin fields.
    // Full-page OCR misreads some values (e.g., "0.88mm" → "0.8mm", sign loss).
    // Isolated crop OCR reads the same text correctly. Re-crop at the LOCATED
    // position and re-read for ALL Belin fields. For other printout types, only
    // re-crop missing/suspicious fields.
    let archetype = field_locate::archetype_for(&printout_type);
    let fit = field_locate::fit_affine(&labeled, archetype);

    // NOTE: Routine re-crop of all Belin fields does NOT work.
    // Tested both fixed-size (200x60) and tight-bbox crops — both cause massive
    // regressions (48-121 regressions vs 2-3 improvements). PaddleOCR v5 via ORT
    // performs WORSE on isolated crops than full pages — the model needs page context.
    // Only selective re-crop works (suspicious values, sign rescue, missing fields).

    // Step 5a: Crop-based re-reading for missing fields (archetype fallback)
    if fit.n_inliers >= 5 {
        crop_rescue_missing(&mut labeled, img_path, archetype, &fit);
    }

    // Step 5a2: Re-crop fields with suspicious values.
    crop_rescue_suspicious(&mut labeled, img_path);

    // Step 5b: Crop-based re-reading for sign-ambiguous fields.
    crop_rescue_signs(&mut labeled, img_path);

    // Step 5c: Post-processing again (crop rescue may have added raw values needing fixes)
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

    // Log timing
    let t_total = t_start.elapsed();
    if matches!(printout_type, PrintoutType::BelinAmbrosio) {
        if let Some(t_fp) = t_fullpage {
            eprintln!("  timing: fullpage={:.1}s total={:.1}s ({} fields)",
                t_fp.as_secs_f64(), t_total.as_secs_f64(), n_located);
        } else {
            eprintln!("  timing: total={:.1}s ({} fields)",
                t_total.as_secs_f64(), n_located);
        }
    }

    // Convert to output format
    // (after all extraction steps including crop rescue)
    let mut fields = HashMap::new();
    let mut confidences = HashMap::new();
    for (name, loc) in &labeled {
        fields.insert(name.clone(), loc.value);
        confidences.insert(name.clone(), loc.conf);
    }

    // Extract demographics from header (for standalone image/PDF input)
    let demo = demographics::extract_from_header(&items);

    Some(PrintoutResult {
        printout_type,
        source_file: source_file.to_path_buf(),
        page_number,
        fields,
        confidences,
        qa_status,
        demographics: demo,
    })
}

/// Routine re-crop and re-read of ALL found Belin fields.
///
/// Full-page OCR sometimes misreads values that isolated crop OCR reads
/// correctly (e.g., "0.88mm" → "0.8mm" on full page, but "0.88mm" on crop).
/// Re-crop at each field's LOCATED position and replace with crop value.
///
/// Logs how many fields were corrected vs confirmed.
fn crop_reread_all(
    labeled: &mut HashMap<String, LocatedField>,
    img_path: &Path,
) {
    // Only re-read Belin data table + BAD-D fields (skip QS — categorical)
    let reread_fields: Vec<(String, LocatedField)> = labeled.iter()
        .filter(|(name, _)| name.starts_with("Belin_") && name.as_str() != "Belin_QS")
        .map(|(name, loc)| (name.clone(), loc.clone()))
        .collect();

    if reread_fields.is_empty() { return; }

    let img = match image::open(img_path) {
        Ok(i) => i,
        Err(_) => return,
    };
    let (iw, ih) = img.dimensions();
    let mut reread_count = 0u32;
    let mut changed_count = 0u32;

    for (field_name, loc) in &reread_fields {
        let crop_half_w: u32 = 100;
        let crop_half_h: u32 = 30;
        let cx_u = loc.cx as u32;
        let cy_u = loc.cy as u32;
        if cx_u < crop_half_w || cy_u < crop_half_h
            || cx_u + crop_half_w >= iw || cy_u + crop_half_h >= ih
        { continue; }

        let crop = img.crop_imm(
            cx_u - crop_half_w,
            cy_u - crop_half_h,
            crop_half_w * 2,
            crop_half_h * 2,
        );

        // Use RAW crop (no preprocessing) — the text is already detected,
        // we just need a cleaner isolated read. Preprocessing (3x upscale +
        // morphological closing) can distort already-readable text.
        let tmp_path = temp_path(&format!("crop_reread_{}.png", field_name));
        if crop.save(&tmp_path).is_err() { continue; }

        if let Ok(crop_items) = ocr_engine::run_full_page(&tmp_path) {
            if let Some((crop_val, crop_conf)) = field_read::extract_best_numeric(&crop_items) {
                reread_count += 1;
                if (crop_val - loc.value).abs() > 0.001 {
                    changed_count += 1;
                }
                // Replace with crop value — isolated crop reads more accurately
                labeled.insert(field_name.clone(), LocatedField {
                    value: crop_val,
                    conf: crop_conf,
                    cx: loc.cx,
                    cy: loc.cy,
                    raw_text: format!("[reread] {}",
                        crop_items.iter()
                            .map(|i| i.text.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")),
                });
            }
        }
        let _ = std::fs::remove_file(&tmp_path);
    }

    if reread_count > 0 {
        eprintln!("  crop-reread: {}/{} fields re-read, {} changed",
            reread_count, reread_fields.len(), changed_count);
    }
}

/// Re-crop all Belin fields using TIGHT bounding boxes from full-page OCR.
///
/// For each found Belin field, find the nearest OCR item's bounding box,
/// crop tightly around it (+ small padding), and re-read. This avoids
/// picking up neighboring values that fixed-size crops include.
fn crop_reread_tight(
    labeled: &mut HashMap<String, LocatedField>,
    items: &[ocr_engine::OcrItem],
    img_path: &Path,
) {
    let reread_fields: Vec<(String, LocatedField)> = labeled.iter()
        .filter(|(name, _)| name.starts_with("Belin_") && name.as_str() != "Belin_QS")
        .map(|(name, loc)| (name.clone(), loc.clone()))
        .collect();

    if reread_fields.is_empty() { return; }

    let img = match image::open(img_path) {
        Ok(i) => i,
        Err(_) => return,
    };
    let (iw, ih) = img.dimensions();
    let pad: u32 = 15;
    let mut reread_count = 0u32;
    let mut changed_count = 0u32;

    for (field_name, loc) in &reread_fields {
        // Find the nearest NUMERIC OCR item's bounding box to snap to.
        // Skip labels (contain ':' or start with letters) to avoid snapping
        // to "K1:" instead of "36.8".
        let mut best_dist = f32::MAX;
        let mut best_bbox = (loc.cx - 75.0, loc.cy - 25.0, loc.cx + 75.0, loc.cy + 25.0);

        for item in items {
            // Skip items that look like labels
            let t = item.text.trim();
            if t.contains(':') { continue; }
            let first = t.chars().next().unwrap_or('x');
            if first.is_ascii_alphabetic() && first != 'e' && first != 'E' { continue; }

            let dist = (item.cx - loc.cx).abs() + (item.cy - loc.cy).abs();
            if dist < best_dist && dist < 40.0 {
                best_dist = dist;
                best_bbox = item.bbox;
            }
        }

        // Crop with padding, enforce minimum size
        let x1 = (best_bbox.0 as u32).saturating_sub(pad);
        let y1 = (best_bbox.1 as u32).saturating_sub(pad);
        let x2 = ((best_bbox.2 as u32) + pad).min(iw);
        let y2 = ((best_bbox.3 as u32) + pad).min(ih);

        if x2 <= x1 || y2 <= y1 { continue; }
        // Minimum crop size — too small crops fail OCR
        if (x2 - x1) < 30 || (y2 - y1) < 15 { continue; }

        let crop = img.crop_imm(x1, y1, x2 - x1, y2 - y1);

        let tmp_path = temp_path(&format!("crop_tight_{}.png", field_name));
        if crop.save(&tmp_path).is_err() { continue; }

        if let Ok(crop_items) = ocr_engine::run_full_page(&tmp_path) {
            if let Some((crop_val, crop_conf)) = field_read::extract_best_numeric(&crop_items) {
                // Only accept crop value if it's plausibly the same field:
                // - Same sign and similar magnitude (allows small corrections)
                // - OR crop fixes a sign (abs values match)
                // - OR crop fixes a decimal shift (ratio ~100x)
                // Reject if completely different (snapped to wrong item)
                let dominated = loc.value.abs() < 0.001; // original is ~0
                let same_ballpark = (crop_val - loc.value).abs() < loc.value.abs().max(1.0) * 0.5;
                let sign_fix = (crop_val.abs() - loc.value.abs()).abs() < 0.06 && crop_val * loc.value < 0.0;
                let decimal_fix = loc.value.abs() > 1.0 && (crop_val * 100.0 - loc.value).abs() < 1.0;

                if same_ballpark || sign_fix || decimal_fix || dominated {
                    reread_count += 1;
                    if (crop_val - loc.value).abs() > 0.001 {
                        changed_count += 1;
                    }
                    labeled.insert(field_name.clone(), LocatedField {
                        value: crop_val,
                        conf: crop_conf,
                        cx: loc.cx,
                        cy: loc.cy,
                        raw_text: format!("[tight-reread] {}",
                            crop_items.iter()
                                .map(|i| i.text.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")),
                    });
                }
            }
        }
        let _ = std::fs::remove_file(&tmp_path);
    }

    if reread_count > 0 {
        eprintln!("  tight-reread: {}/{} fields re-read, {} changed",
            reread_count, reread_fields.len(), changed_count);
    }
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
        let cx_pred = (fit.alpha_cx * cx_ref as f64 + fit.beta_cx) as f32;

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

        // Save to temp file and run OCR.
        // Try preprocessed first, then raw if preprocessing finds nothing
        // (preprocessing can destroy text on colored backgrounds like yellow/red).
        let tmp_path = temp_path(&format!("crop_rescue_{}.png", field_name));
        let mut found = false;

        // Try 1: preprocessed crop
        if processed.save(&tmp_path).is_ok() {
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
                    found = true;
                }
            }
        }

        // Try 2: raw crop (no preprocessing) — handles colored backgrounds
        if !found {
            if crop.save(&tmp_path).is_ok() {
                if let Ok(crop_items) = ocr_engine::run_full_page(&tmp_path) {
                    if let Some((val, conf)) = field_read::extract_best_numeric(&crop_items) {
                        labeled.insert(field_name.to_string(), LocatedField {
                            value: val,
                            conf,
                            cx: cx_pred,
                            cy: cy_pred,
                            raw_text: format!("[crop-rescue-raw] {}",
                                crop_items.iter()
                                    .map(|i| i.text.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" ")),
                        });
                    }
                }
            }
        }

        let _ = std::fs::remove_file(&tmp_path);
    }
}

/// Crop-based re-reading for fields with suspicious values.
///
/// Full-page OCR sometimes misreads small or colored text (e.g., green LCD
/// digits: "0.77" → "D.77" → parsed as 77). Isolated crop OCR reads the same
/// text correctly. For fields where the extracted value is outside the plausible
/// range, re-crop at the LOCATED position and re-read.
fn crop_rescue_suspicious(
    labeled: &mut HashMap<String, LocatedField>,
    img_path: &Path,
) {
    // Plausible ranges for Belin fields. Values outside these trigger re-crop.
    // Conservative: only flag values that are clearly wrong, not borderline.
    let suspicious: Vec<(&str, f64, f64)> = [
        // Progression Index: typically 0-3, pathological up to ~15
        ("Belin_Prog_Min", -20.0, 20.0),
        ("Belin_Prog_Max", -20.0, 200.0),
        ("Belin_Prog_Avg", -20.0, 20.0),
        // Elevation thickness: typically -10 to 60 µm
        ("Belin_F_Ele_Th", -30.0, 80.0),
        ("Belin_B_Ele_Th", -30.0, 80.0),
        // Keratometry: 30-70 D
        ("Belin_K1", 30.0, 70.0),
        ("Belin_K2", 30.0, 70.0),
        ("Belin_KMax", 30.0, 90.0),
        // Qval: -2 to +1
        ("Belin_Qval", -3.0, 2.0),
        // Pachymetry: 200-700 µm
        ("Belin_PachyThin", 200.0, 750.0),
        // Axis: 0-180 degrees
        ("Belin_Axis", 0.0, 180.0),
        // DistVertex: 0-3 mm
        ("Belin_DistVertex", 0.0, 3.5),
        // ARTmax: 0-900
        ("Belin_ARTmax", 0.0, 900.0),
    ].iter()
        .filter(|&&(name, lo, hi)| {
            if let Some(loc) = labeled.get(name) {
                loc.value < lo || loc.value > hi
            } else {
                false
            }
        })
        .map(|&(name, lo, hi)| (name, lo, hi))
        .collect();

    // Also flag fields where OCR raw text starts with "D." — likely misread "0."
    // (colored LCD digits: green/blue "0" → "D"). These are in-range but wrong.
    let misread_zero: Vec<(String, f64, f64)> = labeled.iter()
        .filter(|(name, loc)| {
            name.starts_with("Belin_")
                && loc.raw_text.trim().starts_with("D.")
                && loc.raw_text.trim().len() > 2
                && loc.raw_text.trim().as_bytes().get(2).map_or(false, |b| b.is_ascii_digit())
        })
        .map(|(name, _)| (name.clone(), 0.0, 0.0))
        .collect();

    let mut all_suspicious: Vec<(String, f64, f64)> = suspicious.into_iter()
        .map(|(n, lo, hi)| (n.to_string(), lo, hi))
        .collect();
    all_suspicious.extend(misread_zero);

    if all_suspicious.is_empty() { return; }

    let img = match image::open(img_path) {
        Ok(i) => i,
        Err(_) => return,
    };
    let (iw, ih) = img.dimensions();

    for (field_name, _lo, _hi) in &all_suspicious {
        let loc = match labeled.get(field_name.as_str()) {
            Some(l) => l.clone(),
            None => continue,
        };

        // Crop at the LOCATED position (where full-page OCR found it)
        let crop_half_w: u32 = 100;
        let crop_half_h: u32 = 30;
        let cx_u = loc.cx as u32;
        let cy_u = loc.cy as u32;
        if cx_u < crop_half_w || cy_u < crop_half_h
            || cx_u + crop_half_w >= iw || cy_u + crop_half_h >= ih
        { continue; }

        let crop = img.crop_imm(
            cx_u - crop_half_w,
            cy_u - crop_half_h,
            crop_half_w * 2,
            crop_half_h * 2,
        );

        let processed = field_read::preprocess_crop(&crop);
        let tmp_path = temp_path(&format!("crop_suspicious_{}.png", field_name));
        if processed.save(&tmp_path).is_err() { continue; }

        if let Ok(crop_items) = ocr_engine::run_full_page(&tmp_path) {
            if let Some((crop_val, crop_conf)) = field_read::extract_best_numeric(&crop_items) {
                // Trust the crop value — it's from an isolated read at the known position
                labeled.insert(field_name.to_string(), LocatedField {
                    value: crop_val,
                    conf: crop_conf,
                    cx: loc.cx,
                    cy: loc.cy,
                    raw_text: format!("[suspicious-recrop] {}",
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

/// Crop-based re-reading for sign-ambiguous fields.
///
/// For coordinate fields and Qval where the full-page OCR may have missed
/// the minus sign: crop a wider region including the sign column, preprocess,
/// re-OCR, and use the crop value if it has a different sign.
fn crop_rescue_signs(
    labeled: &mut HashMap<String, LocatedField>,
    img_path: &Path,
) {
    // Fields where sign errors are common
    let sign_fields = [
        "Kmax_x", "Kmax_y",
        "PupilCenter_x", "PupilCenter_y",
        "PachyVertex_x", "PachyVertex_y",
        "Thinnest_x", "Thinnest_y",
        "Qval_front", "Qval_back",
        // Belin BAD-D and data table fields with frequent sign flips
        "Belin_Df", "Belin_Db", "Belin_Dp", "Belin_Dt", "Belin_Da",
        "Belin_D_final", "Belin_Qval", "Belin_F_Ele_Th",
    ];

    // Only re-crop fields that have a value (not missing) and are positive
    // (the sign might have been lost)
    let candidates: Vec<(&str, f64, f32, f32)> = sign_fields.iter()
        .filter_map(|&name| {
            let loc = labeled.get(name)?;
            // Only re-crop if value is positive (could be missing negative sign)
            // For Qval: typically negative, so positive is suspicious
            // For coordinates: could be either sign, re-crop to verify
            if loc.value <= 0.0 { return None; }
            // Use the LOCATED position — the field was already found here
            Some((name, loc.value, loc.cy, loc.cx))
        })
        .collect();

    if candidates.is_empty() { return; }

    let img = match image::open(img_path) {
        Ok(i) => i,
        Err(_) => return,
    };
    let (iw, ih) = img.dimensions();

    for (field_name, current_val, cy_loc, cx_loc) in candidates {
        // Shift crop LEFT by 30px to capture the sign column (minus sign sits
        // to the left of the value). Wider crop than normal.
        let crop_half_w: u32 = 100;
        let crop_half_h: u32 = 25;
        let cx_center = (cx_loc as i32 - 30).max(crop_half_w as i32) as u32;
        let cx_u = cx_center;
        let cy_u = cy_loc as u32;
        if cx_u < crop_half_w || cy_u < crop_half_h
            || cx_u + crop_half_w >= iw || cy_u + crop_half_h >= ih
        { continue; }

        let crop = img.crop_imm(
            cx_u - crop_half_w,
            cy_u - crop_half_h,
            crop_half_w * 2,
            crop_half_h * 2,
        );

        let processed = field_read::preprocess_crop(&crop);
        let tmp_path = temp_path(&format!("sign_rescue_{}.png", field_name));
        if processed.save(&tmp_path).is_err() { continue; }

        if let Ok(crop_items) = ocr_engine::run_full_page(&tmp_path) {
            if let Some((crop_val, crop_conf)) = field_read::extract_best_numeric(&crop_items) {
                // If crop reads a negative value where we had positive, and the
                // absolute values are very close, trust the crop's sign.
                // Use the ORIGINAL magnitude (more reliable) with the CROP's sign.
                if crop_val < 0.0 && current_val > 0.0 && (crop_val.abs() - current_val).abs() < 0.5 {
                    let corrected = -current_val; // flip sign of original value
                    labeled.get_mut(field_name).unwrap().value = corrected;
                    // Keep original confidence
                    labeled.get_mut(field_name).unwrap().raw_text =
                        format!("[sign-rescue] {}", crop_items.iter()
                            .map(|i| i.text.as_str()).collect::<Vec<_>>().join(" "));
                }
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
