//! Test OCR on a single image file.
//! Usage: cargo run -p ocr_import --example test_ocr_single --release -- <image.png>

fn main() {
    let path = std::env::args().nth(1).expect("usage: test_ocr_single <image>");
    let model_dir = std::path::PathBuf::from("models");
    ocr_import::ocr_engine::init(
        model_dir.join("pp-ocrv5_server_det.onnx").to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).expect("init");
    let items = ocr_import::ocr_engine::run_full_page(std::path::Path::new(&path)).expect("ocr");
    if items.is_empty() {
        println!("  (no text detected)");
    }
    for item in &items {
        println!("  cx={:.0} cy={:.0} conf={:.3} text=\"{}\"", item.cx, item.cy, item.confidence, item.text);
    }
}
