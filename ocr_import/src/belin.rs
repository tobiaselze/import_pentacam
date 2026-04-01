//! Belin/Ambrosio Enhanced Ectasia printout extraction.
//!
//! Keratoconus screening indices: BAD-D, IHD, ARTmax, Belin D-values, ABCD staging.

use pentacam_types::PrintoutResult;

pub fn extract(_page_img: &image::DynamicImage, _ocr_items: &[super::ocr_engine::OcrItem]) -> Result<PrintoutResult, String> {
    todo!()
}
