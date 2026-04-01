//! Proof-of-concept: DICOM → PDF → all pages → OCR → printout detection → field location
//!
//! Usage:
//!   ORT_LIB_LOCATION=/path/to/ort/lib ORT_PREFER_DYNAMIC_LINK=1 \
//!   cargo run -p ocr_import --example poc_pipeline --release -- <dicom_or_pdf_or_png>

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ocr_import::ocr_engine;
use ocr_import::printout_detect;
use ocr_import::field_locate;

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

    // Initialize OCR engine
    let model_dir = PathBuf::from("models");
    ocr_engine::init(
        model_dir.join("pp-ocrv5_server_det.onnx").to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).expect("Failed to initialize OCR engine");

    match ext.as_str() {
        "dcm" => {
            let pdf_bytes = extract_pdf_from_dicom(input);
            let pdf_path = PathBuf::from("/tmp/_poc_pentacam.pdf");
            fs::write(&pdf_path, &pdf_bytes).expect("Failed to write temp PDF");
            let n_pages = get_pdf_page_count(&pdf_path);
            println!("DICOM: {} page(s) in embedded PDF", n_pages);
            for page in 1..=n_pages {
                println!("\n========== Page {}/{} ==========", page, n_pages);
                let png = render_pdf_to_png(&pdf_path, page);
                process_page(&png, page);
            }
        }
        "pdf" => {
            let n_pages = get_pdf_page_count(input);
            println!("PDF: {} page(s)", n_pages);
            for page in 1..=n_pages {
                println!("\n========== Page {}/{} ==========", page, n_pages);
                let png = render_pdf_to_png(input, page);
                process_page(&png, page);
            }
        }
        "png" | "jpg" | "jpeg" => {
            process_page(input, 1);
        }
        _ => {
            eprintln!("Unsupported file type: {}", ext);
            std::process::exit(1);
        }
    }
}

fn process_page(png_path: &Path, page_num: u32) {
    let items = ocr_engine::run_full_page(png_path).expect("OCR failed");
    println!("  OCR: {} text regions", items.len());

    let printout_type = printout_detect::detect_printout_type(&items);
    match &printout_type {
        Some(pt) => println!("  Type: {:?}", pt),
        None => {
            println!("  Type: unrecognized");
            return;
        }
    };

    let pt = printout_type.unwrap();
    let is_topo = matches!(pt, pentacam_types::PrintoutType::TopometricKcStaging);
    let labeled = ocr_import::label_match::match_labels(&items, is_topo);

    let archetype = field_locate::archetype_for(&pt);
    let fit = field_locate::fit_affine(&labeled, archetype);
    println!("  Fields: {}/{} | alpha={:.4} beta={:.1} delta_cx={:.1} resid={:.1}",
        labeled.len(), archetype.len(), fit.alpha, fit.beta, fit.delta_cx, fit.resid_std);

    // Print located fields
    let mut fields: Vec<_> = labeled.iter().collect();
    fields.sort_by_key(|(name, _)| name.clone());
    for (name, f) in &fields {
        println!("    {:<20} {:>8.2}  {:.3}  {}", name, f.value, f.conf, f.raw_text);
    }
}

fn extract_pdf_from_dicom(path: &Path) -> Vec<u8> {
    use dicom_core::Tag;
    use dicom_object::open_file;
    let obj = open_file(path).expect("Failed to open DICOM file");
    let pdf_tag = obj
        .element(Tag(0x0042, 0x0011))
        .expect("No EncapsulatedDocument tag (0042,0011) found");
    pdf_tag.to_bytes().expect("Failed to extract PDF bytes").to_vec()
}

fn get_pdf_page_count(pdf_path: &Path) -> u32 {
    let output = Command::new("pdfinfo")
        .arg(pdf_path)
        .output()
        .expect("Failed to run pdfinfo");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.starts_with("Pages:") {
            if let Some(n) = line.split_whitespace().last() {
                return n.parse().unwrap_or(1);
            }
        }
    }
    1
}

fn render_pdf_to_png(pdf_path: &Path, page: u32) -> PathBuf {
    let out_prefix = "/tmp/_poc_pentacam_page";
    // Clean up stale files
    if let Ok(entries) = glob::glob(&format!("{}*.png", out_prefix)) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry);
        }
    }
    let status = Command::new("pdftoppm")
        .args([
            "-r", "300", "-png",
            "-f", &page.to_string(),
            "-l", &page.to_string(),
            pdf_path.to_str().unwrap(),
            out_prefix,
        ])
        .status()
        .expect("Failed to run pdftoppm");
    if !status.success() {
        panic!("pdftoppm failed");
    }
    let mut pages: Vec<PathBuf> = glob::glob(&format!("{}*.png", out_prefix))
        .expect("glob failed")
        .filter_map(|p| p.ok())
        .collect();
    pages.sort();
    if pages.is_empty() {
        panic!("pdftoppm produced no output");
    }
    pages[0].clone()
}
