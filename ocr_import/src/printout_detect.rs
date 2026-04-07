//! Detect printout type from full-page OCR results.
//!
//! Searches all OCR text regions for known title keywords that appear
//! in the header of each Pentacam printout page.

use pentacam_types::PrintoutType;
use super::ocr_engine::OcrItem;

/// Detect the printout type by matching OCR text against known title patterns.
///
/// Joins all OCR text into one uppercase string and checks for keywords.
/// Returns None if no known type is recognized.
pub fn detect_printout_type(items: &[OcrItem]) -> Option<PrintoutType> {
    let all_text: String = items.iter()
        .map(|item| item.text.to_uppercase())
        .collect::<Vec<_>>()
        .join(" ");

    if all_text.contains("CATARACT") && all_text.contains("PRE") {
        Some(PrintoutType::Other("Cataract Pre-OP".into()))
    } else if all_text.contains("REFRACTIVE") {
        Some(PrintoutType::FourMapsRefractive)
    } else if all_text.contains("SELECTABLE") {
        Some(PrintoutType::FourMapsSelectable)
    } else if all_text.contains("TOPOMETRIC")
        || all_text.contains("KC-STAGING")
        || all_text.contains("KC STAGING")
    {
        Some(PrintoutType::TopometricKcStaging)
    } else if all_text.contains("BELIN") && all_text.contains("ECTASIA") {
        Some(PrintoutType::BelinAmbrosio)
    } else if all_text.contains("BELIN") && all_text.contains("ABCD") {
        Some(PrintoutType::BelinAbcdProgression)
    } else if all_text.contains("FOURIER") {
        Some(PrintoutType::Fourier)
    } else if all_text.contains("DENSITOMETRY") {
        Some(PrintoutType::Densitometry)
    } else if all_text.contains("HOLLADAY") {
        Some(PrintoutType::Holladay)
    } else if all_text.contains("COMPARE") && all_text.contains("EXAM") {
        Some(PrintoutType::Other("Compare 4 Exams".into()))
    } else if all_text.contains("SCHEIMPFLUG") {
        Some(PrintoutType::Other("Scheimpflug Images".into()))
    } else if all_text.contains("OVERVIEW") {
        Some(PrintoutType::Other("General Overview".into()))
    } else {
        None
    }
}
