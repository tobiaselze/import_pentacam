//! Proof-of-concept: DICOM → PDF → PNG → full-page OCR → printout detection → field location
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

    // Step 1: Get a PNG image of the first page
    let png_path = match ext.as_str() {
        "dcm" => {
            println!("[1/5] Extracting PDF from DICOM...");
            let pdf_bytes = extract_pdf_from_dicom(input);
            let pdf_path = PathBuf::from("/tmp/_poc_pentacam.pdf");
            fs::write(&pdf_path, &pdf_bytes).expect("Failed to write temp PDF");
            println!("[2/5] Rendering PDF page 1 at 300 DPI...");
            render_pdf_to_png(&pdf_path, 1)
        }
        "pdf" => {
            println!("[1/5] (skipped — input is PDF)");
            println!("[2/5] Rendering PDF page 1 at 300 DPI...");
            render_pdf_to_png(input, 1)
        }
        "png" | "jpg" | "jpeg" => {
            println!("[1/5] (skipped — input is image)");
            println!("[2/5] (skipped — input is image)");
            input.to_path_buf()
        }
        _ => {
            eprintln!("Unsupported file type: {}", ext);
            std::process::exit(1);
        }
    };

    // Step 3: Run full-page OCR
    println!("[3/5] Running PaddleOCR (oar-ocr) on rendered page...");
    let items = ocr_engine::run_full_page(&png_path)
        .expect("OCR failed");
    println!("      {} text regions detected", items.len());

    // Step 4: Detect printout type
    println!("[4/5] Detecting printout type...");
    let printout_type = printout_detect::detect_printout_type(&items);
    match &printout_type {
        Some(pt) => println!("      Detected: {:?}", pt),
        None => {
            println!("      No recognized printout type found.");
            println!("\n      All OCR text (first 50 items):");
            for item in items.iter().take(50) {
                println!("        {:>8.1} {:>8.1}  {:.3}  {}", item.cx, item.cy, item.confidence, item.text);
            }
            return;
        }
    };

    // Step 5: Label matching
    if let Some(ref pt) = printout_type {
        let is_topo = matches!(pt, pentacam_types::PrintoutType::TopometricKcStaging);

        println!("[5/6] Running label matching...");
        let labeled = ocr_import::label_match::match_labels(&items, is_topo);
        println!("      {} fields located by label matching", labeled.len());

        // Step 6: Affine fit
        println!("[6/6] Fitting affine transform...");
        let archetype = field_locate::archetype_for(pt);
        let fit = field_locate::fit_affine(&labeled, archetype);
        println!("      alpha={:.4}, beta={:.1}, delta_cx={:.1}, resid_std={:.1}, inliers={}/{}",
            fit.alpha, fit.beta, fit.delta_cx, fit.resid_std, fit.n_inliers, fit.n_pairs);

        // Print all located fields sorted by name
        println!("\n{:<20} {:>8} {:>8} {:>8} {:>6}  {}", "FIELD", "VALUE", "CX", "CY", "CONF", "RAW");
        println!("{}", "-".repeat(90));
        let mut fields: Vec<_> = labeled.iter().collect();
        fields.sort_by_key(|(name, _)| name.clone());
        for (name, f) in &fields {
            println!("{:<20} {:>8.2} {:>8.1} {:>8.1} {:>6.3}  {}",
                name, f.value, f.cx, f.cy, f.conf, f.raw_text);
        }
    }
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
        .expect("Failed to run pdftoppm — is poppler-utils installed?");

    if !status.success() {
        panic!("pdftoppm failed with status: {}", status);
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
