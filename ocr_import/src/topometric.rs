//! Topometric / KC-Staging printout extraction.
//!
//! Same 40 fields as 4-Maps plus True Net Power section.

use pentacam_types::PrintoutResult;

pub fn extract(_page_img: &image::DynamicImage, _ocr_items: &[super::ocr_engine::OcrItem]) -> Result<PrintoutResult, String> {
    todo!()
}
