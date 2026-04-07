use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Laterality
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Laterality {
    OD, // right eye
    OS, // left eye
}

impl std::fmt::Display for Laterality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Laterality::OD => write!(f, "OD"),
            Laterality::OS => write!(f, "OS"),
        }
    }
}

// ---------------------------------------------------------------------------
// Eye-visit key: uniquely identifies one measurement of one eye
// ---------------------------------------------------------------------------

/// Unique identifier for a single Pentacam measurement of one eye.
///
/// All printouts from the same capture share the same acquisition timestamp
/// (the device takes one rotational scan and generates all pages from it).
/// Using second-precision epoch should be sufficient, but this needs
/// verification against real data — see IMPLEMENTATION_PLAN.md.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EyeVisitKey {
    pub patient_id: String,
    pub eye: Laterality,
    pub exam_epoch_secs: i64,
}

impl EyeVisitKey {
    /// Produce a filesystem-safe, deterministic, non-PII directory name.
    /// Uses blake3 truncated to 16 hex characters.
    pub fn dir_hash(&self) -> String {
        let input = format!("{}|{}|{}", self.patient_id, self.eye, self.exam_epoch_secs);
        let hash = blake3::hash(input.as_bytes());
        hash.to_hex()[..16].to_string()
    }
}

// ---------------------------------------------------------------------------
// Printout types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrintoutType {
    FourMapsRefractive,
    FourMapsSelectable,
    TopometricKcStaging,
    BelinAmbrosio,
    BelinAbcdProgression,
    Fourier,
    Densitometry,
    Holladay,
    Other(String),
}

// ---------------------------------------------------------------------------
// Data from each source
// ---------------------------------------------------------------------------

/// DICOM tag metadata (None fields = not available, e.g. input was PDF/image).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DicomMeta {
    pub patient_id: Option<String>,
    pub patient_name: Option<String>,
    pub date_of_birth: Option<String>,
    pub sex: Option<String>,
    pub exam_date: Option<String>,
    pub exam_time: Option<String>,
    pub laterality: Option<Laterality>,
    pub series_number: Option<String>,
    pub instance_number: Option<String>,
    pub software_version: Option<String>,
    pub device_serial: Option<String>,
}

/// DICOM Structured Report values (firmware 1.30+).
/// Keys are CodeValue strings, values are the numeric measurements.
pub type DicomSrValues = HashMap<String, f64>;

/// Tier 1 blob extraction — exact, deterministic readouts only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobExact {
    pub eye: Laterality,
    pub cornea_dia_mm: Option<f64>, // HWTW, valid 9-15 mm
    pub acd_mm: f64,                // anterior chamber depth
    pub exam_file: String,
    pub blob_format: u16,
}

/// One printout page worth of OCR-extracted data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintoutResult {
    pub printout_type: PrintoutType,
    pub source_file: PathBuf,
    pub page_number: usize, // 1-based
    pub fields: HashMap<String, f64>,
    pub confidences: HashMap<String, f32>,
    pub qa_status: QaStatus,
    pub demographics: Option<PdfDemographics>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QaStatus {
    Ok,
    Incomplete { reason: String },
}

/// Demographics extracted from PDF header (used when DICOM metadata is absent).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PdfDemographics {
    pub patient_name: Option<String>,
    pub patient_id: Option<String>,
    pub date_of_birth: Option<String>,
    pub exam_date: Option<String>,
    pub exam_time: Option<String>,
    pub eye: Option<Laterality>,
}

// ---------------------------------------------------------------------------
// The aggregate eye-visit record
// ---------------------------------------------------------------------------

/// Accumulates all data for one eye-visit from all sources and files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EyeVisit {
    pub key: EyeVisitKey,

    // Module 1: DICOM
    pub dicom_meta: Option<DicomMeta>,
    pub dicom_sr: Option<DicomSrValues>,

    // Module 3: Blob (Tier 1 only)
    pub blob: Option<BlobExact>,

    // Module 2: OCR — multiple printout pages possible
    pub printouts: Vec<PrintoutResult>,

    // Demographics (from PDF header, used if dicom_meta is None)
    pub pdf_demographics: Option<PdfDemographics>,

    // Tracking
    pub source_files: Vec<PathBuf>,

    // Output directory for extracted images
    pub image_dir: Option<PathBuf>,
}

impl EyeVisit {
    pub fn new(key: EyeVisitKey) -> Self {
        Self {
            key,
            dicom_meta: None,
            dicom_sr: None,
            blob: None,
            printouts: Vec::new(),
            pdf_demographics: None,
            source_files: Vec::new(),
            image_dir: None,
        }
    }
}
