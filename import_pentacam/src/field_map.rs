//! Field name mappings, reliability scores, and canonical field lists.
//!
//! Maps DICOM SR codes to canonical field names, provides per-field
//! reliability scores from the 333-file evaluation, and defines the
//! master list of all extractable fields.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Canonical field list (62 fields)
// ---------------------------------------------------------------------------

/// All extractable field names, in output column order.
pub const ALL_FIELDS: &[&str] = &[
    // Front surface keratometry
    "Rf_front", "Rs_front", "Rm_front",
    "K1_front", "K2_front", "Km_front",
    "Astig_front", "Axis_front", "Qval_front",
    "Rmin_front", "Rper_front",
    // Back surface keratometry
    "Rf_back", "Rs_back", "Rm_back",
    "K1_back", "K2_back", "Km_back",
    "Astig_back", "Axis_back", "Qval_back",
    "Rmin_back", "Rper_back",
    // Pupil, pachymetry, thinnest, Kmax
    "PupilCenter", "PupilCenter_x", "PupilCenter_y",
    "PachyVertex", "PachyVertex_x", "PachyVertex_y",
    "Thinnest", "Thinnest_x", "Thinnest_y",
    "Kmax", "Kmax_x", "Kmax_y",
    // Volumes and chamber
    "CorneaVol", "HWTW", "ChamberVol", "Angle", "AC_depth", "PupilDia",
    // True Net Power (Topometric only)
    "TNP_Astig", "TNP_K1", "TNP_Axis", "TNP_K2", "TNP_PMax", "TNP_Km",
    // Belin/Ambrosio
    "Belin_K1", "Belin_K2", "Belin_KMax", "Belin_Axis", "Belin_Qval", "Belin_QS",
    "Belin_PachyThin", "Belin_DistVertex", "Belin_F_Ele_Th", "Belin_B_Ele_Th",
    "Belin_Prog_Min", "Belin_Prog_Max", "Belin_Prog_Avg", "Belin_ARTmax",
    "Belin_Df", "Belin_Db", "Belin_Dp", "Belin_Dt", "Belin_Da", "Belin_D_final",
];

// ---------------------------------------------------------------------------
// DICOM SR code → canonical field name mapping
// ---------------------------------------------------------------------------

/// Maps DICOM Structured Report codes (from ContentSequence) to canonical field names.
/// SR values are machine-precision — no OCR noise.
pub const SR_FIELD_MAP: &[(&str, &str)] = &[
    // BAD-D scores
    ("DICOM_BAMD",     "Belin_D_final"),
    ("DICOM_BAMF",     "Belin_Df"),
    ("DICOM_BAMB",     "Belin_Db"),
    ("DICOM_BAMP",     "Belin_Dp"),
    ("DICOM_BAMT",     "Belin_Dt"),
    ("DICOM_BAMAM",    "Belin_Da"),
    // BAD-D (history-adjusted — same values, different code)
    ("DICOM_BAHD",     "Belin_D_final"),
    ("DICOM_BAHF",     "Belin_Df"),
    ("DICOM_BAHB",     "Belin_Db"),
    ("DICOM_BAHP",     "Belin_Dp"),
    ("DICOM_BAHT",     "Belin_Dt"),
    ("DICOM_BAHAM",    "Belin_Da"),
    // Progression indices
    ("DICOM_ARTMAX",   "Belin_ARTmax"),
    ("DICOM_PACHYMIN", "Belin_PachyThin"),
    ("DICOM_PIAVG",    "Belin_Prog_Avg"),
    ("DICOM_PIMIN",    "Belin_Prog_Min"),
    ("DICOM_PIMAX",    "Belin_Prog_Max"),
    // General measurements
    ("DICOM_CORNEADIA","HWTW"),
    ("DICOM_ACD",      "AC_depth"),
    ("DICOM_PUPILDIA", "PupilDia"),
    ("DICOM_RSAGMIN",  "Kmax"),
];

/// Convert a DICOM SR HashMap (code → value) to canonical field names.
/// Returns (fields, confidences) where all confidences are 1.0 (machine-precision).
pub fn map_sr_to_fields(sr: &HashMap<String, f64>) -> (HashMap<String, f64>, HashMap<String, f32>) {
    let mut fields = HashMap::new();
    let mut confs = HashMap::new();
    for &(sr_code, field_name) in SR_FIELD_MAP {
        if let Some(&val) = sr.get(sr_code) {
            // Don't overwrite if already set (first mapping wins for duplicates like BAM/BAH)
            fields.entry(field_name.to_string()).or_insert(val);
            confs.entry(field_name.to_string()).or_insert(1.0_f32);
        }
    }
    (fields, confs)
}

// ---------------------------------------------------------------------------
// Per-field reliability scores (from 333-file Belin evaluation)
// ---------------------------------------------------------------------------

/// Per-field reliability scores calibrated against human-reviewed GT.
/// These are independent of OCR confidence — they indicate how often
/// the pipeline gets the field right on average.
const RELIABILITY_TABLE: &[(&str, f32)] = &[
    // 4Maps fields — from training300 evaluation (99.46% overall)
    ("Rf_front", 0.997), ("Rs_front", 0.997), ("Rm_front", 0.995),
    ("K1_front", 0.997), ("K2_front", 0.997), ("Km_front", 0.997),
    ("Astig_front", 0.993), ("Axis_front", 0.990), ("Qval_front", 0.990),
    ("Rmin_front", 0.997), ("Rper_front", 0.997),
    ("Rf_back", 0.997), ("Rs_back", 0.997), ("Rm_back", 0.995),
    ("K1_back", 0.997), ("K2_back", 0.997), ("Km_back", 0.997),
    ("Astig_back", 0.993), ("Axis_back", 0.990), ("Qval_back", 0.990),
    ("Rmin_back", 0.997), ("Rper_back", 0.997),
    ("PupilCenter", 0.997), ("PupilCenter_x", 0.990), ("PupilCenter_y", 0.990),
    ("PachyVertex", 0.997), ("PachyVertex_x", 0.990), ("PachyVertex_y", 0.990),
    ("Thinnest", 0.997), ("Thinnest_x", 0.990), ("Thinnest_y", 0.990),
    ("Kmax", 0.997), ("Kmax_x", 0.990), ("Kmax_y", 0.990),
    ("CorneaVol", 0.997), ("HWTW", 0.997), ("ChamberVol", 0.997),
    ("Angle", 0.997), ("AC_depth", 0.997), ("PupilDia", 0.997),
    ("TNP_Astig", 0.990), ("TNP_K1", 0.990), ("TNP_Axis", 0.990),
    ("TNP_K2", 0.990), ("TNP_PMax", 0.990), ("TNP_Km", 0.990),
    // Belin fields — from 333-file evaluation (99.03% overall)
    ("Belin_K1", 0.997), ("Belin_K2", 0.997), ("Belin_KMax", 0.997),
    ("Belin_Axis", 0.988), ("Belin_Qval", 0.954),
    ("Belin_QS", 0.990),
    ("Belin_PachyThin", 0.994), ("Belin_DistVertex", 0.957),
    ("Belin_F_Ele_Th", 0.991), ("Belin_B_Ele_Th", 0.985),
    ("Belin_Prog_Min", 0.997), ("Belin_Prog_Max", 0.997), ("Belin_Prog_Avg", 0.997),
    ("Belin_ARTmax", 0.994),
    ("Belin_Df", 0.994), ("Belin_Db", 0.997), ("Belin_Dp", 0.991),
    ("Belin_Dt", 0.997), ("Belin_Da", 0.994), ("Belin_D_final", 0.994),
];

/// Look up the reliability score for a field name.
/// Returns 0.95 (conservative default) for unknown fields.
pub fn field_reliability(field_name: &str) -> f32 {
    RELIABILITY_TABLE.iter()
        .find(|&&(name, _)| name == field_name)
        .map(|&(_, r)| r)
        .unwrap_or(0.95)
}

// ---------------------------------------------------------------------------
// Data source identification
// ---------------------------------------------------------------------------

/// Where a field value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    /// DICOM Structured Report (machine-precision, firmware 1.30+)
    DicomSR,
    /// SPR proprietary binary blob (exact readouts)
    Spr,
    /// OCR from 4-Maps Refractive or Selectable page
    OcrFourMaps,
    /// OCR from Belin/Ambrosio Enhanced Ectasia Display
    OcrBelin,
    /// OCR from Topometric/KC-Staging page
    OcrTopometric,
    /// OCR from other printout type
    OcrOther,
}

impl DataSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            DataSource::DicomSR => "SR",
            DataSource::Spr => "SPR",
            DataSource::OcrFourMaps => "4Maps",
            DataSource::OcrBelin => "Belin",
            DataSource::OcrTopometric => "Topo",
            DataSource::OcrOther => "Other",
        }
    }

    /// Confidence floor for this source type.
    /// SR and SPR are machine-precision (1.0). OCR uses Paddle confidence.
    pub fn base_confidence(&self) -> f32 {
        match self {
            DataSource::DicomSR | DataSource::Spr => 1.0,
            _ => 0.0, // OCR uses actual Paddle confidence
        }
    }
}

impl std::fmt::Display for DataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
