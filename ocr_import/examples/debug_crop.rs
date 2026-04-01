//! Debug: dump all OCR items from a single crop, with and without preprocessing.
//!
//! Usage: cargo run -p ocr_import --example debug_crop --release -- <crop.png>

use std::env;
use std::path::{Path, PathBuf};
use ocr_import::ocr_engine;
use ocr_import::field_read;
use ocr_import::field_locate;

fn main() {
    let args: Vec<String> = env::args().collect();
    let crop_path = Path::new(&args[1]);

    let model_dir = PathBuf::from("models");
    ocr_engine::init(
        model_dir.join("pp-ocrv5_mobile_det.onnx").to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).unwrap();

    println!("=== Raw crop (no preprocessing) ===");
    dump_ocr(crop_path);

    println!("\n=== With preprocessing (3x upscale + fill_hollow_digits) ===");
    let img = image::open(crop_path).unwrap();
    let processed = field_read::preprocess_crop(&img);
    let tmp = PathBuf::from("/tmp/_debug_crop_processed.png");
    processed.save(&tmp).unwrap();
    dump_ocr(&tmp);
}

fn dump_ocr(path: &Path) {
    let items = ocr_engine::run_full_page(path).unwrap();
    println!("  {} items:", items.len());
    for item in &items {
        let val = field_locate::extract_numeric(&item.text);
        println!("    text={:30}  conf={:.3}  cx={:>7.1}  cy={:>7.1}  → numeric={:?}",
            format!("{:?}", item.text), item.confidence, item.cx, item.cy, val);
    }
}
