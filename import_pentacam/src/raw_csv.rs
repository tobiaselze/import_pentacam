//! Raw CSV writer for incremental output.
//!
//! One row per data extraction event (SR, SPR, or OCR page). Written
//! incrementally after each PACS subfolder — survives interruptions.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::field_map::ALL_FIELDS;

// ---------------------------------------------------------------------------
// Raw row data structure
// ---------------------------------------------------------------------------

/// One row in the raw CSV — represents a single extraction event.
#[derive(Debug, Clone)]
pub struct RawRow {
    // Identity
    pub patient_id: String,
    pub family_name: String,
    pub given_name: String,
    pub dob: String,
    pub eye: String,
    pub exam_date: String,
    pub exam_time: String,
    // Source tracking
    pub source_folder: String,
    pub source_file: String,
    pub page_number: u32,
    pub printout_type: String,
    pub qa_status: String,
    pub n_fields: u32,
    pub device_serial: String,
    pub software_version: String,
    pub imagedir: String,
    // Field values and confidences
    pub fields: HashMap<String, f64>,
    pub confidences: HashMap<String, f32>,
}

// ---------------------------------------------------------------------------
// CSV writer
// ---------------------------------------------------------------------------

/// Append-mode raw CSV writer. Creates file with header if new, appends if existing.
pub struct RawCsvWriter {
    writer: BufWriter<File>,
    omit_patient_name: bool,
}

impl RawCsvWriter {
    /// Open or create the raw CSV file. Writes header if the file is new/empty.
    pub fn open(path: &Path, omit_patient_name: bool) -> std::io::Result<Self> {
        let exists = path.exists() && std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        let mut writer = BufWriter::new(file);

        if !exists {
            Self::write_header(&mut writer, omit_patient_name)?;
        }

        Ok(RawCsvWriter { writer, omit_patient_name })
    }

    fn write_header(writer: &mut BufWriter<File>, omit_patient_name: bool) -> std::io::Result<()> {
        let mut header = if omit_patient_name {
            "id,birthdate,eye,exam_date,exam_time,\
                source_folder,source_file,page_number,printout_type,qa_status,\
                n_fields,device_serial,software_version,imagedir"
                .to_string()
        } else {
            "id,FamilyName,GivenName,birthdate,eye,exam_date,exam_time,\
                source_folder,source_file,page_number,printout_type,qa_status,\
                n_fields,device_serial,software_version,imagedir"
                .to_string()
        };

        for &field in ALL_FIELDS {
            header.push(',');
            header.push_str(field);
        }
        for &field in ALL_FIELDS {
            header.push(',');
            header.push_str(field);
            header.push_str("_Paddle_conf");
        }
        writeln!(writer, "{}", header)
    }

    /// Write one raw row to the CSV.
    pub fn write_row(&mut self, row: &RawRow) -> std::io::Result<()> {
        let mut line = if self.omit_patient_name {
            format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                csv_escape(&row.patient_id),
                csv_escape(&row.dob),
                csv_escape(&row.eye),
                csv_escape(&row.exam_date),
                csv_escape(&row.exam_time),
                csv_escape(&row.source_folder),
                csv_escape(&row.source_file),
                row.page_number,
                csv_escape(&row.printout_type),
                csv_escape(&row.qa_status),
                row.n_fields,
                csv_escape(&row.device_serial),
                csv_escape(&row.software_version),
                csv_escape(&row.imagedir),
            )
        } else {
            format!(
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                csv_escape(&row.patient_id),
                csv_escape(&row.family_name),
                csv_escape(&row.given_name),
                csv_escape(&row.dob),
                csv_escape(&row.eye),
                csv_escape(&row.exam_date),
                csv_escape(&row.exam_time),
                csv_escape(&row.source_folder),
                csv_escape(&row.source_file),
                row.page_number,
                csv_escape(&row.printout_type),
                csv_escape(&row.qa_status),
                row.n_fields,
                csv_escape(&row.device_serial),
                csv_escape(&row.software_version),
                csv_escape(&row.imagedir),
            )
        };

        // Field values
        for &field in ALL_FIELDS {
            line.push(',');
            if let Some(&val) = row.fields.get(field) {
                line.push_str(&format!("{}", val));
            }
        }
        // Confidence scores
        for &field in ALL_FIELDS {
            line.push(',');
            if let Some(&conf) = row.confidences.get(field) {
                line.push_str(&format!("{:.4}", conf));
            }
        }

        writeln!(self.writer, "{}", line)
    }

    /// Flush all buffered data to disk.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

// ---------------------------------------------------------------------------
// Source manifest writer (for image directories)
// ---------------------------------------------------------------------------

/// Write a sources.csv manifest into an image directory.
pub fn write_source_manifest(
    image_dir: &Path,
    entries: &[(String, u32, String, String)],
) -> std::io::Result<()> {
    let path = image_dir.join("sources.csv");
    let mut f = BufWriter::new(File::create(path)?);
    writeln!(f, "filename,page,printout_type,data_source")?;
    for (filename, page, pt, ds) in entries {
        writeln!(f, "{},{},{},{}", csv_escape(filename), page, csv_escape(pt), csv_escape(ds))?;
    }
    f.flush()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
