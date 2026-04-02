//! Test: compare file-based vs in-memory OCR to verify identical results.
//!
//! Usage:
//!   cargo run -p ocr_import --example test_inmemory --release -- <image.png>

use std::env;
use std::path::{Path, PathBuf};
use ocr_import::ocr_engine;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: test_inmemory <image.png>");
        std::process::exit(1);
    }
    let img_path = Path::new(&args[1]);

    let model_dir = PathBuf::from("models");
    let det = if args.iter().any(|a| a == "--mobile") { "pp-ocrv5_mobile_det.onnx" } else { "pp-ocrv5_server_det.onnx" };
    ocr_engine::init(
        model_dir.join(det).to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).unwrap();

    // Method 1: file-based
    let t1 = std::time::Instant::now();
    let items_file = ocr_engine::run_full_page(img_path).unwrap();
    let d1 = t1.elapsed();

    // Method 2: in-memory
    let img = image::open(img_path).unwrap().to_rgb8();
    let t2 = std::time::Instant::now();
    let items_mem = ocr_engine::run_full_page_mem(img).unwrap();
    let d2 = t2.elapsed();

    println!("File-based: {} items in {:.3}s", items_file.len(), d1.as_secs_f64());
    println!("In-memory:  {} items in {:.3}s", items_mem.len(), d2.as_secs_f64());
    println!();

    // Compare
    let n = items_file.len().min(items_mem.len());
    let mut matches = 0;
    let mut diffs = 0;
    for i in 0..n {
        let f = &items_file[i];
        let m = &items_mem[i];
        if f.text == m.text && (f.cx - m.cx).abs() < 0.1 && (f.cy - m.cy).abs() < 0.1 {
            matches += 1;
        } else {
            diffs += 1;
            if diffs <= 10 {
                println!("DIFF [{}]: file=({:.1},{:.1}) {:?}  mem=({:.1},{:.1}) {:?}",
                    i, f.cx, f.cy, f.text, m.cx, m.cy, m.text);
            }
        }
    }
    if items_file.len() != items_mem.len() {
        println!("COUNT DIFF: file={} mem={}", items_file.len(), items_mem.len());
    }
    println!();
    println!("Identical: {}/{}", matches, n);
    if diffs == 0 && items_file.len() == items_mem.len() {
        println!("RESULT: IDENTICAL ✓");
    } else {
        println!("RESULT: DIFFERENT ({} diffs)", diffs + (items_file.len() as i64 - items_mem.len() as i64).unsigned_abs() as usize);
    }
}
