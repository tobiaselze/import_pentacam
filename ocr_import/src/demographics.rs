//! Extract patient demographics from PDF printout header.
//!
//! Used only when DICOM metadata is not available (standalone PDF/image input).

use pentacam_types::PdfDemographics;
use super::ocr_engine::OcrItem;

/// Extract patient name, ID, DOB, exam date, eye from the printout header region.
pub fn extract_from_header(_items: &[OcrItem]) -> Option<PdfDemographics> {
    todo!("Match header region labels: 'Name:', 'Pat-ID:', 'DOB:', 'Exam:', 'OD'/'OS'")
}
