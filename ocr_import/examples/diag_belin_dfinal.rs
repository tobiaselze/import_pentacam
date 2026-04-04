//! Diagnostic: dump affine fit params and nearby OCR items for Belin D_final.
//!
//! Usage:
//!   cargo run -p ocr_import --example diag_belin_dfinal --release -- \
//!     /tmp/quick50_belin.txt [--mupdf]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::collections::HashMap;

use ocr_import::ocr_engine;
use ocr_import::render::{self, Renderer};
use ocr_import::printout_detect;
use ocr_import::belin;
use ocr_import::field_locate::{fit_affine, LocatedField, ARCHETYPE_BELIN};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: diag_belin_dfinal <input_list.txt> [--mupdf]");
        std::process::exit(1);
    }
    let input = PathBuf::from(&args[1]);
    let renderer = if args.iter().any(|a| a == "--mupdf") {
        Renderer::MuPdf
    } else {
        Renderer::Poppler
    };

    let model_dir = PathBuf::from("models");
    ocr_engine::init(
        model_dir.join("pp-ocrv5_server_det.onnx").to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).expect("Failed to initialize OCR engine");

    let files = discover_files(&input);
    eprintln!("Processing {} files", files.len());

    for file_path in &files {
        let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let pages = match ext.as_str() {
            "dcm" => {
                let obj = dicom_import::open(file_path).ok();
                let pdf_bytes = obj.as_ref().and_then(|o| dicom_import::extract_pdf_bytes(o));
                match pdf_bytes {
                    Some(ref pdf) => render_pdf_pages(pdf, renderer).unwrap_or_default(),
                    None => continue,
                }
            }
            "pdf" => {
                let pdf_bytes = fs::read(file_path).ok();
                match pdf_bytes {
                    Some(ref pdf) => render_pdf_pages(pdf, renderer).unwrap_or_default(),
                    None => continue,
                }
            }
            _ => {
                let img = match image::open(file_path) {
                    Ok(i) => i.to_rgb8(),
                    Err(_) => continue,
                };
                vec![(img, 1)]
            }
        };

        let fname = file_path.file_name().unwrap().to_str().unwrap();

        for (page_img, page_num) in &pages {
            // Save to temp for OCR
            let tmp = PathBuf::from(format!("/tmp/_diag_p{}.png", page_num));
            image::DynamicImage::ImageRgb8(page_img.clone()).save(&tmp).unwrap();
            let items = match ocr_engine::run_full_page(&tmp) {
                Ok(i) => i,
                Err(_) => { let _ = fs::remove_file(&tmp); continue; }
            };
            let _ = fs::remove_file(&tmp);

            // Check if Belin page
            let pt = printout_detect::detect_printout_type(&items);
            if !matches!(pt, Some(pentacam_types::PrintoutType::BelinAmbrosio)) {
                continue;
            }

            // Run belin extraction
            let result = belin::extract(&items);
            let fit = fit_affine(&result, ARCHETYPE_BELIN);

            // D_final archetype: cy_ref=2368, cx_ref=3218
            let cy_ref = 2368.0_f64;
            let cx_ref = 3218.0_f32;
            let cy_pred = (fit.alpha * cy_ref + fit.beta) as f32;
            let cx_pred = cx_ref + fit.delta_cx as f32;

            // Group B position
            let cx_group_b = 3123.0_f32;
            let cx_pred_b = cx_group_b + fit.delta_cx as f32;

            let has_dfinal = result.contains_key("Belin_D_final");
            let dfinal_val = result.get("Belin_D_final").map(|f| format!("{} (raw: {})", f.value, f.raw_text));

            println!("=== {} p{} ===", fname, page_num);
            println!("  fit: alpha={:.4} beta={:.1} delta_cx={:.1} n_inliers={} n_pairs={}",
                fit.alpha, fit.beta, fit.delta_cx, fit.n_inliers, fit.n_pairs);
            println!("  D_final found: {} val: {:?}", has_dfinal, dfinal_val);
            println!("  Predicted GroupA: cx={:.0} cy={:.0}", cx_pred, cy_pred);
            println!("  Predicted GroupB: cx={:.0} cy={:.0}", cx_pred_b, cy_pred);

            // Show all items near D_final position (both Group A and B)
            let search_cx_min = (cx_pred_b - 100.0).min(cx_pred - 100.0);
            let search_cx_max = (cx_pred + 100.0).max(cx_pred_b + 100.0);
            let search_cy_min = cy_pred - 60.0;
            let search_cy_max = cy_pred + 60.0;

            println!("  OCR items near D_final (cx {:.0}-{:.0}, cy {:.0}-{:.0}):",
                search_cx_min, search_cx_max, search_cy_min, search_cy_max);
            for item in &items {
                if item.cx >= search_cx_min && item.cx <= search_cx_max
                    && item.cy >= search_cy_min && item.cy <= search_cy_max
                {
                    println!("    cx={:.0} cy={:.0} conf={:.3} text=\"{}\"",
                        item.cx, item.cy, item.confidence, item.text);
                }
            }

            // Also show ALL items in BAD-D row (cy > 1900) for context
            println!("  All BAD-D row items (cy > 1900 predicted):");
            let bad_d_cy_min = (fit.alpha * 1900.0 + fit.beta) as f32;
            for item in &items {
                if item.cy >= bad_d_cy_min {
                    println!("    cx={:.0} cy={:.0} conf={:.3} text=\"{}\"",
                        item.cx, item.cy, item.confidence, item.text);
                }
            }
            println!();
        }
    }
}

fn render_pdf_pages(pdf_bytes: &[u8], renderer: Renderer) -> Result<Vec<(image::RgbImage, u32)>, String> {
    let n = render::page_count(pdf_bytes, renderer)?;
    let mut pages = Vec::new();
    for p in 1..=n {
        let png_path = render::render_pdf_page(pdf_bytes, p, 300, renderer)?;
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
