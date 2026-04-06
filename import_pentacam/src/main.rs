use clap::Parser;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use pentacam_types::{EyeVisit, EyeVisitKey};

mod eye_visit;
pub mod field_map;

#[derive(Parser)]
#[command(name = "import_pentacam")]
#[command(about = "Extract clinical measurements from Pentacam DICOM/PDF/image files")]
struct Args {
    /// Root directory to scan (recursively), or a single file
    input: PathBuf,

    /// Output directory for results and extracted images
    #[arg(short, long, default_value = "pentacam_output")]
    output_dir: PathBuf,

    /// Output CSV filename prefix
    #[arg(short = 'f', long, default_value = "pentacam_")]
    output_prefix: String,

    /// Error log file path (default: <output_dir>/errors.log)
    #[arg(short, long)]
    error_log: Option<PathBuf>,

    /// Processed files log (for incremental re-runs)
    #[arg(short, long)]
    processed_log: Option<PathBuf>,

    /// Strip patient names from output
    #[arg(short, long)]
    strip_names: bool,
}

fn main() {
    let args = Args::parse();

    // Eye-visit accumulator: all data for all scans, keyed by unique eye-visit
    let mut visits: HashMap<EyeVisitKey, EyeVisit> = HashMap::new();

    // Discover input files
    let input = &args.input;
    if input.is_dir() {
        for entry in WalkDir::new(input).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                match ext.to_lowercase().as_str() {
                    "dcm" => process_dicom(path, &mut visits),
                    "pdf" => process_pdf(path, &mut visits),
                    "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" => {
                        process_image(path, &mut visits)
                    }
                    _ => {}
                }
            }
        }
    } else {
        // Single file
        if let Some(ext) = input.extension().and_then(|e| e.to_str()) {
            match ext.to_lowercase().as_str() {
                "dcm" => process_dicom(input, &mut visits),
                "pdf" => process_pdf(input, &mut visits),
                "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" => {
                    process_image(input, &mut visits)
                }
                _ => eprintln!("Unsupported file type: {}", input.display()),
            }
        }
    }

    // Write output
    println!(
        "Processed {} eye-visits from input files",
        visits.len()
    );

    // TODO: Write merged CSV output with priority rules (SR > OCR > blob, except HWTW)
    // TODO: Write processed-files log for incremental re-runs
}

fn process_dicom(path: &Path, visits: &mut HashMap<EyeVisitKey, EyeVisit>) {
    let _ = (path, visits);
    todo!("1. extract_metadata -> DicomMeta
          2. extract_sr -> Option<DicomSrValues>
          3. extract_pdf_bytes -> if Some, call ocr_import::process_pdf_bytes
          4. extract_blob -> if Some, call blob_import::extract_exact
          5. Build EyeVisitKey, insert/merge into visits HashMap")
}

fn process_pdf(path: &Path, visits: &mut HashMap<EyeVisitKey, EyeVisit>) {
    let _ = (path, visits);
    todo!("1. ocr_import::process_pdf_file (need_demographics=true)
          2. Build EyeVisitKey from PdfDemographics
          3. Insert/merge into visits HashMap")
}

fn process_image(path: &Path, visits: &mut HashMap<EyeVisitKey, EyeVisit>) {
    let _ = (path, visits);
    todo!("1. ocr_import::process_image_file (need_demographics=true)
          2. Build EyeVisitKey from PdfDemographics
          3. Insert/merge into visits HashMap")
}
