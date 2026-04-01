//! Benchmark: run oar-ocr on the Python agent's training300 crops and compare against GT.
//!
//! Usage:
//!   ORT_LIB_LOCATION=/tmp/onnxruntime-linux-x64-gpu-1.20.1/lib ORT_PREFER_DYNAMIC_LINK=1 \
//!   CUDA_VISIBLE_DEVICES=1 \
//!   cargo run -p ocr_import --example benchmark_crops --release -- \
//!     /path/to/training300_crops /path/to/training_300.csv [--limit N]

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ocr_import::ocr_engine;
use ocr_import::field_locate;
use ocr_import::field_read;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: benchmark_crops <crops_dir> <gt_csv> [--limit N]");
        std::process::exit(1);
    }
    let crops_dir = Path::new(&args[1]);
    let gt_csv = Path::new(&args[2]);
    let limit: Option<usize> = args.iter()
        .position(|a| a == "--limit")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());

    // Initialize OCR engine
    let model_dir = PathBuf::from("models");
    let use_mobile = args.iter().any(|a| a == "--mobile");
    let use_preprocess = args.iter().any(|a| a == "--preprocess");
    let use_postprocess = args.iter().any(|a| a == "--postprocess");
    let det_model = if use_mobile {
        eprintln!("Using MOBILE detection model (faster, less accurate)");
        "pp-ocrv5_mobile_det.onnx"
    } else {
        eprintln!("Using SERVER detection model (slower, more accurate)");
        "pp-ocrv5_server_det.onnx"
    };
    if use_preprocess {
        eprintln!("Preprocessing enabled: 3x upscale + fill_hollow_digits");
    }
    ocr_engine::init(
        model_dir.join(det_model).to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).expect("Failed to initialize OCR engine");

    // Load ground truth
    eprintln!("Loading ground truth from {}...", gt_csv.display());
    let gt = load_ground_truth(gt_csv);
    eprintln!("  {} files with GT", gt.len());

    // Enumerate crops
    let mut crop_files: Vec<PathBuf> = fs::read_dir(crops_dir)
        .expect("Can't read crops dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "png").unwrap_or(false))
        .collect();
    crop_files.sort();

    if let Some(lim) = limit {
        crop_files.truncate(lim);
    }
    eprintln!("  {} crops to process", crop_files.len());

    // Process crops
    let mut total = 0u32;
    let mut correct = 0u32;
    let mut missing_gt = 0u32;
    let mut no_ocr = 0u32;
    let mut mismatches: Vec<(String, String, f64, f64)> = Vec::new(); // (file, field, gt, ocr)

    let mut per_field_total: HashMap<String, u32> = HashMap::new();
    let mut per_field_correct: HashMap<String, u32> = HashMap::new();

    let start = Instant::now();

    for (i, crop_path) in crop_files.iter().enumerate() {
        if i > 0 && i % 500 == 0 {
            let elapsed = start.elapsed().as_secs_f64();
            let rate = i as f64 / elapsed;
            eprintln!("  [{}/{}] {:.0} crops/sec, {:.1}% correct so far",
                i, crop_files.len(), rate,
                if total > 0 { correct as f64 / total as f64 * 100.0 } else { 0.0 });
        }

        let fname = crop_path.file_stem().unwrap().to_str().unwrap();
        let (file_key, field_name) = match parse_crop_filename(fname) {
            Some(v) => v,
            None => continue,
        };

        // Look up GT value
        let gt_val = match gt.get(&file_key).and_then(|fields| fields.get(&field_name)) {
            Some(&v) => v,
            None => {
                missing_gt += 1;
                continue;
            }
        };

        // Run OCR on crop
        let mut ocr_val = match run_ocr_on_crop(crop_path, use_preprocess) {
            Some(v) => v,
            None => {
                no_ocr += 1;
                total += 1;
                *per_field_total.entry(field_name.clone()).or_insert(0) += 1;
                mismatches.push((file_key, field_name, gt_val, f64::NAN));
                continue;
            }
        };

        // Apply postprocessing if enabled
        if use_postprocess {
            if let Some(val) = ocr_val {
                // Get the raw text from the last OCR run
                let raw = crop_path.file_stem().unwrap().to_str().unwrap_or("");
                let mut fields = HashMap::new();
                fields.insert(field_name.clone(), ocr_import::field_locate::LocatedField {
                    value: val, conf: 0.9, cx: 0.0, cy: 0.0,
                    raw_text: format!("{}", val), // simplified — real pipeline has actual raw text
                });
                ocr_import::postprocess::apply_corrections(&mut fields);
                if let Some(f) = fields.get(&field_name) {
                    ocr_val = Some(f.value);
                }
            }
        }

        total += 1;
        *per_field_total.entry(field_name.clone()).or_insert(0) += 1;

        let tol = tolerance_for_field(&field_name);
        if (ocr_val - gt_val).abs() <= tol {
            correct += 1;
            *per_field_correct.entry(field_name.clone()).or_insert(0) += 1;
        } else {
            mismatches.push((file_key, field_name, gt_val, ocr_val));
        }
    }

    let elapsed = start.elapsed().as_secs_f64();

    // Print results
    println!("\n========== BENCHMARK RESULTS ==========");
    println!("Crops processed: {}", total);
    println!("Correct: {} ({:.2}%)", correct, correct as f64 / total as f64 * 100.0);
    println!("Wrong: {}", total - correct);
    println!("No OCR result: {}", no_ocr);
    println!("Missing GT: {}", missing_gt);
    println!("Time: {:.1}s ({:.1} crops/sec)", elapsed, crop_files.len() as f64 / elapsed);

    // Per-field accuracy
    println!("\n--- Per-field accuracy ---");
    let mut fields: Vec<_> = per_field_total.keys().cloned().collect();
    fields.sort();
    for field in &fields {
        let t = per_field_total[field];
        let c = per_field_correct.get(field).copied().unwrap_or(0);
        let pct = if t > 0 { c as f64 / t as f64 * 100.0 } else { 0.0 };
        if pct < 100.0 {
            println!("  {:<20} {:>4}/{:>4} ({:>5.1}%)", field, c, t, pct);
        }
    }

    // Show first N mismatches
    let show_n = 30.min(mismatches.len());
    if show_n > 0 {
        println!("\n--- First {} mismatches ---", show_n);
        for (file_key, field, gt_val, ocr_val) in mismatches.iter().take(show_n) {
            let short_file: String = file_key.chars().take(40).collect();
            println!("  {:<40} {:<20} GT={:<10} OCR={}", short_file, field, gt_val, ocr_val);
        }
    }
}

fn run_ocr_on_crop(path: &Path, preprocess: bool) -> Option<f64> {
    if !preprocess {
        let items = ocr_engine::run_full_page(path).ok()?;
        return extract_best_value(&items);
    }

    // Smart strategy: try raw first. If we get a high-confidence float with
    // decimal, use it (raw preserves decimal points that preprocessing destroys).
    // Fall back to preprocessed if raw fails or returns no decimal.
    let raw_items = ocr_engine::run_full_page(path).ok()?;
    let raw_val = extract_best_value(&raw_items);

    // If raw gave a good float with decimal, use it
    if let Some(v) = raw_val {
        // Check if this looks like a proper decimal value (not just an integer)
        let has_decimal = raw_items.iter().any(|item| {
            item.text.contains('.') && item.confidence > 0.8
        });
        if has_decimal {
            return Some(v);
        }
    }

    // Fall back to preprocessed
    let img = image::open(path).ok()?;
    let processed = field_read::preprocess_crop(&img);
    let tmp = path.with_extension("_proc.png");
    processed.save(&tmp).ok()?;
    let items = ocr_engine::run_full_page(&tmp).ok()?;
    let _ = fs::remove_file(&tmp);
    let proc_val = extract_best_value(&items);

    // Prefer preprocessed if raw failed entirely
    if raw_val.is_none() {
        return proc_val;
    }

    // If both succeeded, prefer preprocessed (better for hollow fonts)
    // unless raw had a decimal and preprocessed lost it
    match (raw_val, proc_val) {
        (Some(rv), Some(pv)) => {
            // If raw has decimal precision that preprocessed lost, prefer raw
            let rv_has_frac = (rv - rv.round()).abs() > 0.001;
            let pv_has_frac = (pv - pv.round()).abs() > 0.001;
            if rv_has_frac && !pv_has_frac {
                Some(rv)
            } else {
                Some(pv)
            }
        }
        (Some(rv), None) => Some(rv),
        (None, Some(pv)) => Some(pv),
        (None, None) => None,
    }
    extract_best_value(&items)
}

fn extract_best_value(items: &[ocr_import::ocr_engine::OcrItem]) -> Option<f64> {
    // Find the best numeric value among OCR items.
    // Prefer longer text (more digits) over short fragments like "3" from "mm³".
    let mut best_val: Option<f64> = None;
    let mut best_score: f64 = -1.0;
    for item in items {
        let stripped = item.text.trim().trim_start_matches(&['(', '['][..]);
        if stripped.is_empty() { continue; }
        let first = match stripped.chars().next() {
            Some(c) => c,
            None => continue,
        };
        if !first.is_ascii_digit() && first != '+' && first != '-' { continue; }
        if stripped.ends_with(':') { continue; }
        if let Some(val) = field_locate::extract_numeric(&item.text) {
            // Weight by text length squared to strongly prefer multi-digit values
            let text_len = stripped.len().max(1) as f64;
            let score = item.confidence as f64 * text_len * text_len;
            if score > best_score {
                best_val = Some(val);
                best_score = score;
            }
        }
    }
    best_val
}

/// Parse crop filename: "1_3_6_..._dcm_p1_Rf_front.png" → (file_key, field_name)
fn parse_crop_filename(fname: &str) -> Option<(String, String)> {
    // Split on "_dcm_p" to separate filename from page+field
    let parts: Vec<&str> = fname.splitn(2, "_dcm_p").collect();
    if parts.len() != 2 { return None; }
    let file_key = parts[0].to_string();
    // parts[1] is like "1_Rf_front"
    let page_field = parts[1];
    let underscore_pos = page_field.find('_')?;
    let field_name = &page_field[underscore_pos + 1..];
    Some((file_key, field_name.to_string()))
}

/// Load ground truth CSV into HashMap<file_key, HashMap<field, value>>
fn load_ground_truth(csv_path: &Path) -> HashMap<String, HashMap<String, f64>> {
    let content = fs::read_to_string(csv_path).expect("Can't read GT CSV");
    let mut lines = content.lines();
    let header: Vec<&str> = lines.next().expect("Empty CSV").split(',').collect();

    // Find column indices for filename and field columns
    let filename_idx = header.iter().position(|&h| h == "filename").expect("No 'filename' column");

    // Field columns start after the metadata columns
    let field_names: Vec<String> = header.iter()
        .enumerate()
        .filter(|(_, &h)| {
            !["dcm_path", "filename", "page", "printout_type", "dataset", "confirmed_gt"]
                .contains(&h)
            && !h.ends_with("_conf")
            && !h.ends_with("_src")
        })
        .map(|(_, &h)| h.to_string())
        .collect();

    let mut gt: HashMap<String, HashMap<String, f64>> = HashMap::new();

    for line in lines {
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() <= filename_idx { continue; }

        let filename = cols[filename_idx];
        // Convert filename to crop file key: replace dots with underscores, strip extension
        let file_key = filename
            .trim_end_matches(".dcm")
            .replace('.', "_");

        let mut fields: HashMap<String, f64> = HashMap::new();
        for field_name in &field_names {
            if let Some(idx) = header.iter().position(|&h| h == field_name.as_str()) {
                if idx < cols.len() {
                    if let Ok(val) = cols[idx].trim().parse::<f64>() {
                        fields.insert(field_name.clone(), val);
                    }
                }
            }
        }

        gt.insert(file_key, fields);
    }

    gt
}

/// Tolerance for comparing GT vs OCR values.
/// Integer fields (pachymetry, volume) get ±0.5, others get ±0.05.
fn tolerance_for_field(field: &str) -> f64 {
    let integer_fields = [
        "Thinnest", "PachyVertex", "PupilCenter",
        "CorneaVol", "ChamberVol",
    ];
    if integer_fields.iter().any(|&f| field == f) {
        0.5
    } else {
        0.05
    }
}
