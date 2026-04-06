//! Save Belin crop rescue crops for inspection.
//!
//! For each file, renders the Belin page, runs full-page OCR, computes the
//! affine fit, then saves crops for ALL archetype fields — both the raw crop
//! and the preprocessed version that crop rescue would feed to OCR.
//!
//! Usage:
//!   cargo run -p ocr_import --example save_belin_crops --release -- \
//!     <input_list.txt> --output /tmp/belin_crops_debug [--mupdf]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use image::GenericImageView;

use ocr_import::ocr_engine;
use ocr_import::render::{self, Renderer};
use ocr_import::printout_detect;
use ocr_import::belin;
use ocr_import::field_read;
use ocr_import::field_locate::{fit_affine, ARCHETYPE_BELIN};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: save_belin_crops <input_list.txt> --output <dir> [--mupdf]");
        std::process::exit(1);
    }
    let input = PathBuf::from(&args[1]);
    let output_dir = args.iter().position(|a| a == "--output")
        .and_then(|i| args.get(i + 1)).map(|s| PathBuf::from(s))
        .unwrap_or_else(|| PathBuf::from("/tmp/belin_crops_debug"));
    let renderer = if args.iter().any(|a| a == "--mupdf") {
        Renderer::MuPdf
    } else {
        Renderer::Poppler
    };

    fs::create_dir_all(&output_dir).unwrap();

    let model_dir = PathBuf::from("models");
    ocr_engine::init(
        model_dir.join("pp-ocrv5_server_det.onnx").to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).expect("Failed to initialize OCR engine");

    let files = discover_files(&input);
    eprintln!("Processing {} files, output to {}", files.len(), output_dir.display());

    for file_path in &files {
        let pages = match load_pages(file_path, renderer) {
            Ok(p) => p,
            Err(e) => { eprintln!("  ERROR {}: {}", file_path.display(), e); continue; }
        };

        let fname = file_path.file_name().unwrap().to_str().unwrap()
            .replace('.', "_");

        for (page_img, page_num) in &pages {
            // Save to temp for OCR
            let tmp = PathBuf::from(format!("/tmp/_crop_debug_p{}.png", page_num));
            image::DynamicImage::ImageRgb8(page_img.clone()).save(&tmp).unwrap();
            let items = match ocr_engine::run_full_page(&tmp) {
                Ok(i) => i,
                Err(_) => { let _ = fs::remove_file(&tmp); continue; }
            };

            // Check if Belin page
            let pt = printout_detect::detect_printout_type(&items);
            if !matches!(pt, Some(pentacam_types::PrintoutType::BelinAmbrosio)) {
                let _ = fs::remove_file(&tmp);
                continue;
            }

            // Run belin extraction to get labeled fields
            let result = belin::extract(&items);
            let fit = fit_affine(&result, ARCHETYPE_BELIN);

            eprintln!("  {} p{}: n_fields={} alpha={:.4} beta={:.1} delta_cx={:.1}",
                fname, page_num, result.len(), fit.alpha, fit.beta, fit.delta_cx);

            if fit.n_inliers < 5 {
                eprintln!("    Skipping crops: too few inliers ({})", fit.n_inliers);
                let _ = fs::remove_file(&tmp);
                continue;
            }

            let img = image::open(&tmp).unwrap();
            let (iw, ih) = img.dimensions();

            // Save crops for each archetype field
            for &(field_name, cy_ref, cx_ref) in ARCHETYPE_BELIN {
                let cy_pred = (fit.alpha * cy_ref as f64 + fit.beta) as f32;
                let cx_pred = cx_ref + fit.delta_cx as f32;

                let found = result.contains_key(field_name);
                let found_marker = if found { "FOUND" } else { "MISSING" };

                // Crop dimensions (same as crop_rescue_missing)
                let crop_half_w: u32 = 100;
                let crop_half_h: u32 = 30;
                let cx_u = cx_pred as u32;
                let cy_u = cy_pred as u32;

                if cx_u < crop_half_w || cy_u < crop_half_h
                    || cx_u + crop_half_w >= iw || cy_u + crop_half_h >= ih
                {
                    eprintln!("    {} [{}]: out of bounds cx={} cy={}", field_name, found_marker, cx_u, cy_u);
                    continue;
                }

                // Raw crop
                let crop = img.crop_imm(
                    cx_u - crop_half_w,
                    cy_u - crop_half_h,
                    crop_half_w * 2,
                    crop_half_h * 2,
                );

                let crop_name = format!("{}_p{}_{}_{}",
                    fname, page_num, field_name, found_marker);
                let raw_path = output_dir.join(format!("{}_raw.png", crop_name));
                crop.save(&raw_path).unwrap();

                // Preprocessed crop (what crop rescue feeds to OCR)
                let processed = field_read::preprocess_crop(&crop);
                let proc_path = output_dir.join(format!("{}_proc.png", crop_name));
                processed.save(&proc_path).unwrap();

                // If found, also show what value we got
                if let Some(loc) = result.get(field_name) {
                    eprintln!("    {} [FOUND]: val={} raw=\"{}\" cx={:.0} cy={:.0}",
                        field_name, loc.value, loc.raw_text, loc.cx, loc.cy);
                } else {
                    eprintln!("    {} [MISSING]: predicted cx={:.0} cy={:.0}",
                        field_name, cx_pred, cy_pred);

                    // Also run OCR on the preprocessed crop to see what it finds
                    let tmp_crop = PathBuf::from(format!("/tmp/_crop_test_{}.png", field_name));
                    if processed.save(&tmp_crop).is_ok() {
                        if let Ok(crop_items) = ocr_engine::run_full_page(&tmp_crop) {
                            let texts: Vec<&str> = crop_items.iter().map(|i| i.text.as_str()).collect();
                            eprintln!("      crop OCR: {:?}", texts);
                        }
                        let _ = fs::remove_file(&tmp_crop);
                    }
                }
            }

            let _ = fs::remove_file(&tmp);
        }
    }
}

fn load_pages(path: &Path, renderer: Renderer) -> Result<Vec<(image::RgbImage, u32)>, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let pdf_bytes = match ext.as_str() {
        "dcm" => {
            let obj = dicom_import::open(path)?;
            dicom_import::extract_pdf_bytes(&obj)
                .ok_or_else(|| "No embedded PDF".to_string())?
        }
        "pdf" => fs::read(path).map_err(|e| format!("Read: {}", e))?,
        _ => {
            let img = image::open(path).map_err(|e| format!("Load: {}", e))?.to_rgb8();
            return Ok(vec![(img, 1)]);
        }
    };
    let n = render::page_count(&pdf_bytes, renderer)?;
    let mut pages = Vec::new();
    for p in 1..=n {
        let png_path = render::render_pdf_page(&pdf_bytes, p, 300, renderer)?;
        let img = image::open(&png_path).map_err(|e| format!("Load page: {}", e))?.to_rgb8();
        let _ = fs::remove_file(&png_path);
        pages.push((img, p));
    }
    Ok(pages)
}

fn discover_files(input: &Path) -> Vec<PathBuf> {
    if input.is_file() {
        if input.extension().and_then(|e| e.to_str()) == Some("txt") {
            return fs::read_to_string(input).expect("Can't read file list")
                .lines().filter(|l| !l.trim().is_empty())
                .map(|l| PathBuf::from(l.trim())).filter(|p| p.exists()).collect();
        }
        return vec![input.to_path_buf()];
    }
    vec![]
}
