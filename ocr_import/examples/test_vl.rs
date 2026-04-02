//! Test VLMs (PaddleOCR-VL, UniRec) on Pentacam crops.
//!
//! Usage:
//!   cargo run -p ocr_import --example test_vl --release -- \
//!     --model paddleocr-vl --model-dir models/PaddleOCR-VL \
//!     <crop_or_page.png> [<crop2.png> ...]

use std::env;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = env::args().collect();

    let model_name = args.iter()
        .position(|a| a == "--model")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("paddleocr-vl");

    let model_dir = args.iter()
        .position(|a| a == "--model-dir")
        .and_then(|i| args.get(i + 1))
        .map(|s| PathBuf::from(s))
        .unwrap_or_else(|| PathBuf::from(format!("models/{}", model_name)));

    // Collect image paths — skip flag values
    let mut skip_next = false;
    let mut images: Vec<PathBuf> = Vec::new();
    for a in &args[1..] {
        if skip_next { skip_next = false; continue; }
        if a == "--model" || a == "--model-dir" { skip_next = true; continue; }
        if a.starts_with("--") { continue; }
        if a.ends_with(".png") || a.ends_with(".jpg") || a.ends_with(".jpeg") {
            images.push(PathBuf::from(a));
        }
    }

    if images.is_empty() {
        eprintln!("Usage: test_vl --model paddleocr-vl|unirec --model-dir <dir> <image.png> ...");
        std::process::exit(1);
    }

    eprintln!("Model: {}  Dir: {}", model_name, model_dir.display());
    eprintln!("Images: {}", images.len());

    // Load images into RgbImage
    let rgb_images: Vec<image::RgbImage> = images.iter()
        .filter_map(|p| image::open(p).ok().map(|i| i.to_rgb8()))
        .collect();

    eprintln!("Loaded {} images", rgb_images.len());

    let device = if args.iter().any(|a| a == "--cpu") {
        eprintln!("Using CPU");
        candle_core::Device::Cpu
    } else {
        match candle_core::Device::new_cuda(0) {
            Ok(d) => { eprintln!("Using CUDA GPU 0"); d }
            Err(_) => { eprintln!("CUDA not available, falling back to CPU"); candle_core::Device::Cpu }
        }
    };

    match model_name {
        "paddleocr-vl" => {
            use oar_ocr_vl::PaddleOcrVl;
            use oar_ocr_vl::paddleocr_vl::PaddleOcrVlTask;

            eprintln!("Loading PaddleOCR-VL from {}...", model_dir.display());
            let model = PaddleOcrVl::from_dir(&model_dir, device)
                .expect("Failed to load PaddleOCR-VL");

            let tasks: Vec<PaddleOcrVlTask> = rgb_images.iter()
                .map(|_| PaddleOcrVlTask::Ocr)
                .collect();

            eprintln!("Running inference...");
            let results = model.generate(&rgb_images, &tasks, 256);

            for (i, (path, result)) in images.iter().zip(results.iter()).enumerate() {
                println!("=== {} ===", path.display());
                match result {
                    Ok(text) => println!("{}", text),
                    Err(e) => println!("ERROR: {}", e),
                }
                println!();
            }
        }
        "unirec" => {
            use oar_ocr_vl::UniRec;

            eprintln!("Loading UniRec from {}...", model_dir.display());
            let model = UniRec::from_dir(&model_dir, device)
                .expect("Failed to load UniRec");

            eprintln!("Running inference...");
            let results = model.generate(&rgb_images, 256);

            for (path, result) in images.iter().zip(results.iter()) {
                println!("=== {} ===", path.display());
                match result {
                    Ok(text) => println!("{}", text),
                    Err(e) => println!("ERROR: {}", e),
                }
                println!();
            }
        }
        "hunyuanocr" => {
            use oar_ocr_vl::HunyuanOcr;

            eprintln!("Loading HunyuanOCR from {}...", model_dir.display());
            let model = HunyuanOcr::from_dir(&model_dir, device)
                .expect("Failed to load HunyuanOCR");

            let prompt = "Transcribe the exact numeric value printed in this image. Reply with ONLY the number, nothing else.";
            eprintln!("Running inference with prompt: {}", prompt);
            let results = model.generate(&rgb_images, &vec![prompt; rgb_images.len()], 64);

            for (path, result) in images.iter().zip(results.iter()) {
                println!("=== {} ===", path.display());
                match result {
                    Ok(text) => println!("{}", text),
                    Err(e) => println!("ERROR: {}", e),
                }
                println!();
            }
        }
        _ => {
            eprintln!("Unknown model: {}. Use 'paddleocr-vl' or 'unirec'", model_name);
            std::process::exit(1);
        }
    }
}
