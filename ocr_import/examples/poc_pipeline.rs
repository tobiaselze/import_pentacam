//! Proof-of-concept: DICOM → PDF → PNG → full-page OCR
//!
//! Usage:
//!   cargo run -p ocr_import --example poc_pipeline -- <dicom_or_pdf_or_png>
//!
//! Downloads ONNX models on first run to ./models/

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: poc_pipeline <dicom_or_pdf_or_png>");
        std::process::exit(1);
    }
    let input = Path::new(&args[1]);
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Step 1: Get a PNG image of the first page
    let png_path = match ext.as_str() {
        "dcm" => {
            println!("[1/4] Extracting PDF from DICOM...");
            let pdf_bytes = extract_pdf_from_dicom(input);
            let pdf_path = PathBuf::from("/tmp/_poc_pentacam.pdf");
            fs::write(&pdf_path, &pdf_bytes).expect("Failed to write temp PDF");

            println!("[2/4] Rendering PDF page 1 at 300 DPI...");
            render_pdf_to_png(&pdf_path, 1)
        }
        "pdf" => {
            println!("[1/4] (skipped — input is PDF)");
            println!("[2/4] Rendering PDF page 1 at 300 DPI...");
            render_pdf_to_png(input, 1)
        }
        "png" | "jpg" | "jpeg" => {
            println!("[1/4] (skipped — input is image)");
            println!("[2/4] (skipped — input is image)");
            input.to_path_buf()
        }
        _ => {
            eprintln!("Unsupported file type: {}", ext);
            std::process::exit(1);
        }
    };

    // Step 3: Run full-page OCR
    println!("[3/4] Running PaddleOCR (oar-ocr) on rendered page...");
    let items = run_full_page_ocr(&png_path);

    // Step 4: Print results
    println!("[4/4] Results: {} text regions detected\n", items.len());
    println!(
        "{:<50} {:>6} {:>8} {:>8}",
        "TEXT", "CONF", "CX", "CY"
    );
    println!("{}", "-".repeat(76));
    for item in &items {
        println!(
            "{:<50} {:>6.3} {:>8.1} {:>8.1}",
            truncate(&item.text, 50),
            item.conf,
            item.cx,
            item.cy
        );
    }
}

struct OcrItem {
    text: String,
    conf: f32,
    cx: f32,
    cy: f32,
}

fn extract_pdf_from_dicom(path: &Path) -> Vec<u8> {
    use dicom_core::Tag;
    use dicom_object::open_file;

    let obj = open_file(path).expect("Failed to open DICOM file");
    let pdf_tag = obj
        .element(Tag(0x0042, 0x0011))
        .expect("No EncapsulatedDocument tag (0042,0011) found");
    pdf_tag
        .to_bytes()
        .expect("Failed to extract PDF bytes")
        .to_vec()
}

fn render_pdf_to_png(pdf_path: &Path, page: u32) -> PathBuf {
    // Use pdftoppm (Poppler) for now — matches Python pipeline exactly.
    // Will switch to pdfium-render once validated.
    let out_prefix = "/tmp/_poc_pentacam_page";
    let status = Command::new("pdftoppm")
        .args([
            "-r", "300",
            "-png",
            "-f", &page.to_string(),
            "-l", &page.to_string(),
            pdf_path.to_str().unwrap(),
            out_prefix,
        ])
        .status()
        .expect("Failed to run pdftoppm — is poppler-utils installed?");

    if !status.success() {
        panic!("pdftoppm failed with status: {}", status);
    }

    // pdftoppm outputs files like /tmp/_poc_pentacam_page-01.png
    let pattern = format!("{}*.png", out_prefix);
    let mut pages: Vec<PathBuf> = glob::glob(&pattern)
        .expect("glob failed")
        .filter_map(|p| p.ok())
        .collect();
    pages.sort();

    if pages.is_empty() {
        panic!("pdftoppm produced no output");
    }
    pages[0].clone()
}

fn run_full_page_ocr(img_path: &Path) -> Vec<OcrItem> {
    use oar_ocr::prelude::*;

    let model_dir = PathBuf::from("models");
    let det_path = model_dir.join("pp-ocrv5_server_det.onnx");
    let rec_path = model_dir.join("en_pp-ocrv5_mobile_rec.onnx");
    let dict_path = model_dir.join("en_ppocrv5_dict.txt");

    // Check models exist
    for (name, path) in [
        ("detection", &det_path),
        ("recognition", &rec_path),
        ("dictionary", &dict_path),
    ] {
        if !path.exists() {
            eprintln!(
                "Model file not found: {}\n\
                 Download from https://github.com/GreatV/oar-ocr/releases\n\
                 and place in ./models/",
                path.display()
            );
            eprintln!("Missing {} model", name);
            std::process::exit(1);
        }
    }

    let ocr = OAROCRBuilder::new(
        det_path.to_str().unwrap(),
        rec_path.to_str().unwrap(),
        dict_path.to_str().unwrap(),
    )
    .build()
    .expect("Failed to build OCR engine");

    let image = load_image(img_path).expect("Failed to load image");
    let results = ocr.predict(vec![image]).expect("OCR prediction failed");

    results[0]
        .text_regions
        .iter()
        .filter_map(|region| {
            let (text, conf) = region.text_with_confidence()?;
            let bb = &region.bounding_box;
            // Compute centroid from bounding box
            // BoundingBox is likely a polygon with 4 points or a rect
            let (cx, cy) = bounding_box_centroid(bb);
            Some(OcrItem {
                text: text.to_string(),
                conf,
                cx,
                cy,
            })
        })
        .collect()
}

fn bounding_box_centroid(bb: &oar_ocr::processors::BoundingBox) -> (f32, f32) {
    let n = bb.points.len() as f32;
    if n == 0.0 {
        return (0.0, 0.0);
    }
    let cx: f32 = bb.points.iter().map(|p| p.x).sum::<f32>() / n;
    let cy: f32 = bb.points.iter().map(|p| p.y).sum::<f32>() / n;
    (cx, cy)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max - 3).collect();
        format!("{}...", truncated)
    }
}
