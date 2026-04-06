//! Wrapper around oar-ocr for full-page OCR and field-crop reading.

use oar_ocr::prelude::*;
use oar_ocr_core::core::config::{OrtSessionConfig, OrtExecutionProvider};
use once_cell::sync::OnceCell;
use std::path::Path;

/// A detected text region: text content, confidence, centroid, and bounding box.
#[derive(Debug, Clone)]
pub struct OcrItem {
    pub text: String,
    pub confidence: f32,
    pub cx: f32,
    pub cy: f32,
    /// Bounding box: (x_min, y_min, x_max, y_max)
    pub bbox: (f32, f32, f32, f32),
}

/// Global OCR engine singleton.
static OCR_ENGINE: OnceCell<OAROCR> = OnceCell::new();

/// Initialize the OCR engine with model paths.
/// Tries CUDA first, falls back to CPU.
pub fn init(det_model: &str, rec_model: &str, dict_path: &str) -> Result<(), String> {
    let ort_config = OrtSessionConfig::new()
        .with_execution_providers(vec![
            OrtExecutionProvider::CUDA {
                device_id: None,
                gpu_mem_limit: None, // use all available GPU memory
                arena_extend_strategy: Some("1".to_string()), // kSameAsRequested — no power-of-2 bloat
                cudnn_conv_algo_search: None,
                cudnn_conv_use_max_workspace: None,
            },
            OrtExecutionProvider::CPU,
        ]);

    let ocr = OAROCRBuilder::new(det_model, rec_model, dict_path)
        .ort_session(ort_config)
        .build()
        .map_err(|e| format!("Failed to build OCR engine: {}", e))?;
    OCR_ENGINE.set(ocr).map_err(|_| "OCR engine already initialized".to_string())
}

fn get_engine() -> &'static OAROCR {
    OCR_ENGINE.get().expect("OCR engine not initialized — call ocr_engine::init() first")
}

/// Extract OcrItems from oar-ocr results.
fn results_to_items(results: &[OAROCRResult]) -> Vec<OcrItem> {
    results[0]
        .text_regions
        .iter()
        .filter_map(|region| {
            let (text, conf) = region.text_with_confidence()?;
            let bb = &region.bounding_box;
            let n = bb.points.len() as f32;
            if n == 0.0 { return None; }
            let cx = bb.points.iter().map(|p| p.x).sum::<f32>() / n;
            let cy = bb.points.iter().map(|p| p.y).sum::<f32>() / n;
            let x_min = bb.points.iter().map(|p| p.x).fold(f32::MAX, f32::min);
            let y_min = bb.points.iter().map(|p| p.y).fold(f32::MAX, f32::min);
            let x_max = bb.points.iter().map(|p| p.x).fold(f32::MIN, f32::max);
            let y_max = bb.points.iter().map(|p| p.y).fold(f32::MIN, f32::max);
            Some(OcrItem {
                text: text.trim().to_string(),
                confidence: conf,
                cx,
                cy,
                bbox: (x_min, y_min, x_max, y_max),
            })
        })
        .collect()
}

/// Run full-page OCR on an image file.
pub fn run_full_page(img_path: &Path) -> Result<Vec<OcrItem>, String> {
    let ocr = get_engine();
    let image = load_image(img_path).map_err(|e| format!("Failed to load image: {}", e))?;
    let results = ocr.predict(vec![image]).map_err(|e| format!("OCR prediction failed: {}", e))?;
    Ok(results_to_items(&results))
}

/// Run full-page OCR on an in-memory RGB image. No temp files needed.
pub fn run_full_page_mem(image: image::RgbImage) -> Result<Vec<OcrItem>, String> {
    let ocr = get_engine();
    let results = ocr.predict(vec![image]).map_err(|e| format!("OCR prediction failed: {}", e))?;
    Ok(results_to_items(&results))
}
