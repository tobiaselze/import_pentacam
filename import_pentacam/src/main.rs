//! import_pentacam — Extract clinical measurements from Pentacam DICOM/PDF/image files.

use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};

mod eye_visit;
pub mod field_map;
pub mod logging;
pub mod raw_csv;
pub mod compact_csv;
pub mod pipeline;

use ocr_import::render::Renderer;
use pipeline::{PipelineConfig, PentacamPipeline};

const ABOUT: &str = r#"
Extracts clinical measurements and map images from Pentacam DICOM, PDF, and
image files. Supports data from all Pentacam generations, including old JPEG
printouts from PACS (pre-2010) and modern DICOM files with Structured Reports.

INPUT MODES:

  Single file     Pass a .dcm, .pdf, or image file directly.
  File list       Pass a .txt file with one file path per line.
  CSV file list   Pass a .csv file with a header row. Only the column
                  "filename" is required; optional columns: id, familyname,
                  givenname, dob (YYYY-MM-DD or YYYYMMDD), examdate
                  (YYYY-MM-DD or YYYYMMDD), examtime (6-digit HHMMSS),
                  laterality (OD or OS), printouttype.
                  When metadata is provided, OCR demographics extraction is
                  skipped for image files. Files with unsupported printout types
                  are skipped without opening. Paths may use ~ for $HOME.
  PACS directory  Pass a directory to recursively scan for Pentacam DICOMs
                  (identified by prefix 1.3.6.1.4.1.34714.).

Image files with non-standard extensions (e.g. .JPG_1) are auto-detected by
magic bytes. If a file is not found, the _1 suffix is tried automatically
(Harmony PACS quirk). Low-resolution images (< 2800px wide) are auto-upscaled
to ~300 DPI with Lanczos3 before OCR.

OUTPUT:

  A single Pentacam scan may produce multiple data sources: a DICOM Structured
  Report (SR), a proprietary binary readout (SPR), and OCR readings from one or
  more printout pages (4Maps, Belin, Topometric). These are first written as
  individual rows to the raw CSV, then merged per eye-visit into two summary
  CSVs:

  pentacam_raw.csv       One row per extraction event (SR, SPR, or OCR page).
                         Contains all extracted field values and per-field OCR
                         confidence scores. Written incrementally during
                         processing — survives interruptions.

  pentacam_compact.csv   One row per eye-visit (patient + eye + exam date),
                         with only the best value per field. Ready for clinical
                         analysis — import directly into R or pandas. Use
                         --no-compact to skip generation.

  pentacam_detailed.csv  One row per eye-visit, like compact, but with five
                         columns per field: value, source (SR/SPR/OCR type),
                         OCR confidence, field reliability score, and a flag
                         (SR_OCR_MISMATCH, REVIEW, or LOW_CONF). Intended for
                         quality assurance and research. Use --no-detailed to
                         skip generation.

  The best value per field is selected by priority: SR > SPR > highest-
  confidence OCR (exception: HWTW prefers SPR > SR > OCR). When SR and OCR
  disagree beyond a field-specific threshold, the SR_OCR_MISMATCH flag is set
  in the detailed CSV.

  images/<hash>/         Extracted map images (4Maps 2x2 grid, Belin elevation/
                         thickness/charts) and a sources.csv manifest linking
                         back to the raw data. Each eye-visit gets its own
                         directory named by a 16-char hash. Use --no-images to
                         skip image extraction, --save-pages to also save full
                         rendered printout pages (may contain PII).

  processed_files.csv    Restart log for file list / CSV mode.
  processed_folders.csv  Restart log for PACS directory mode.
  errors.log             Structured warnings and errors.

Previously processed files/folders are skipped on restart. To reprocess,
delete the corresponding processed_*.csv file.

SUPPORTED PRINTOUT TYPES:

  4 Maps Refractive, 4 Maps Selectable, Topometric/KC-Staging,
  Belin/Ambrosio Enhanced Ectasia Display, Refractive (gen1, pre-2010).
  Unsupported types (Holladay, Compare, Cataract Pre-OP, etc.) are skipped.

DATA SOURCES (priority: SR > SPR > OCR):

  DICOM SR      Some Pentacam DICOMs (firmware 1.30+) include DICOM Structured
                Reports with machine-precision measurement values. When present,
                these are prioritized over all other sources.
  SPR blob      Some Pentacam DICOMs embed a proprietary binary file (SPR format)
                with exact measurement readouts. Used when SR is not available.
  OCR           PaddleOCR v5 via ONNX Runtime, with field-specific confidence.
                Used as fallback when neither SR nor SPR is available, and as
                cross-validation when SR is present.

BACKUP:

  Use --backup-dir to incrementally copy new image directories and CSV files
  to a backup location after each run. Use --backup-only to perform only the
  backup step (e.g. to resume an interrupted backup). Before regenerating
  output CSVs, the previous versions are saved as .previous files locally.

(C) 2026 Tobias Elze
"#;

#[derive(Parser)]
#[command(name = "import_pentacam")]
#[command(about = "Extract clinical measurements from Pentacam DICOM/PDF/image files")]
#[command(long_about = ABOUT)]
#[command(version)]
struct Args {
    /// DICOM file, PDF file, image file, file list (.txt), CSV file list (.csv),
    /// or PACS directory
    input: PathBuf,

    /// Output directory for results and extracted images
    #[arg(short, long, default_value = "pentacam_output")]
    output_dir: PathBuf,

    /// Strip patient names from output (omits FamilyName/GivenName columns)
    #[arg(short = 'x', long)]
    omit_patient_names: bool,

    /// Save full rendered printout pages (contains PII — disabled by default)
    #[arg(long)]
    save_pages: bool,

    /// Skip map image extraction (no images/ directory)
    #[arg(long)]
    no_images: bool,

    /// Skip detailed CSV generation
    #[arg(long)]
    no_detailed: bool,

    /// Skip compact CSV generation
    #[arg(long)]
    no_compact: bool,

    /// Use Poppler (pdftoppm) for PDF rendering instead of MuPDF (default).
    /// Requires poppler-utils to be installed on the system.
    #[arg(long)]
    poppler: bool,

    /// Error/warning log file (default: <output_dir>/errors.log)
    #[arg(short, long)]
    error_log: Option<PathBuf>,

    /// Backup directory. After successful export, new image directories and
    /// CSV files are incrementally copied here. If missing, no backup is performed.
    #[arg(short, long)]
    backup_dir: Option<PathBuf>,

    /// Perform only the incremental backup (see --backup-dir). Skips data export.
    /// If --backup-dir is not specified, this does nothing.
    #[arg(short = 'B', long)]
    backup_only: bool,
}

/// Save a .previous backup of a file (if it exists).
fn backup_file(path: &Path) {
    if !path.exists() { return; }
    if let (Some(parent), Some(filename)) = (path.parent(), path.file_name()) {
        let backup_name = format!("{}.previous", filename.to_string_lossy());
        let backup_path = parent.join(backup_name);
        if backup_path.exists() {
            let _ = fs::remove_file(&backup_path);
        }
        let _ = fs::copy(path, &backup_path);
    }
}

/// Incrementally copy new top-level subdirectories from source to backup.
fn incremental_backup_dirs(source: &Path, backup: &Path) -> Result<u32, String> {
    if !source.exists() { return Ok(0); }

    fs::create_dir_all(backup).map_err(|e| format!("Create backup dir: {}", e))?;

    // Collect existing backup subdirs
    let existing: std::collections::HashSet<std::ffi::OsString> = if backup.exists() {
        fs::read_dir(backup).map_err(|e| format!("Read backup dir: {}", e))?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.path().file_name().map(|n| n.to_os_string()))
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    let mut copied = 0u32;
    let entries = fs::read_dir(source).map_err(|e| format!("Read source dir: {}", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("Read entry: {}", e))?;
        let path = entry.path();
        if !path.is_dir() { continue; }

        if let Some(name) = path.file_name() {
            if existing.contains(name) { continue; }

            let dest = backup.join(name);
            fs::create_dir_all(&dest).map_err(|e| format!("Create dir: {}", e))?;

            for file in fs::read_dir(&path).map_err(|e| format!("Read dir: {}", e))? {
                let file = file.map_err(|e| format!("Read file: {}", e))?;
                let src = file.path();
                let dst = dest.join(file.file_name());
                fs::copy(&src, &dst).map_err(|e| format!("Copy {}: {}", src.display(), e))?;
            }
            copied += 1;
        }
    }
    Ok(copied)
}

fn run_backup(output_dir: &Path, backup_dir: &Path) {
    eprintln!("\nPerforming backup to {} ...", backup_dir.display());
    let _ = fs::create_dir_all(backup_dir);

    // 1. Incremental backup of images/ subdirectories
    let images_src = output_dir.join("images");
    let images_dst = backup_dir.join("images");
    if images_src.exists() {
        match incremental_backup_dirs(&images_src, &images_dst) {
            Ok(n) => eprintln!("  Backed up {} new image directories", n),
            Err(e) => eprintln!("  WARNING: Image backup failed: {}", e),
        }
    }

    // 2. Copy CSV and log files
    let files_to_backup = [
        "pentacam_raw.csv",
        "pentacam_compact.csv",
        "pentacam_detailed.csv",
        "processed_files.csv",
        "processed_folders.csv",
        "errors.log",
    ];
    for name in &files_to_backup {
        let src = output_dir.join(name);
        if src.exists() {
            let dst = backup_dir.join(name);
            match fs::copy(&src, &dst) {
                Ok(_) => {}
                Err(e) => eprintln!("  WARNING: Failed to backup {}: {}", name, e),
            }
        }
    }
    eprintln!("Backup complete.");
}

fn main() {
    let args = Args::parse();

    // Backup-only mode: just do the backup and exit
    if args.backup_only {
        if let Some(ref backup_dir) = args.backup_dir {
            run_backup(&args.output_dir, backup_dir);
        } else {
            eprintln!("--backup-only requires --backup-dir");
        }
        return;
    }

    // Determine renderer
    let renderer = if args.poppler {
        eprintln!("Using Poppler renderer");
        Renderer::Poppler
    } else {
        eprintln!("Using MuPDF renderer");
        Renderer::MuPdf
    };

    // Initialize OCR engine
    // Look for models/ next to the binary first (system-wide install),
    // then fall back to current directory (development / dist mode).
    let model_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("models")))
        .filter(|d| d.exists())
        .unwrap_or_else(|| PathBuf::from("models"));
    ocr_import::ocr_engine::init(
        model_dir.join("pp-ocrv5_server_det.onnx").to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).expect("Failed to initialize OCR engine");

    // Build pipeline config
    let config = PipelineConfig::new(
        args.output_dir.clone(),
        args.omit_patient_names,
        renderer,
        args.save_pages,
        !args.no_images,
    );

    // Create and run pipeline
    let mut pipeline = PentacamPipeline::new(config)
        .expect("Failed to initialize pipeline");

    let t0 = std::time::Instant::now();
    pipeline.process_input(&args.input);
    pipeline.finish();

    // Generate output CSVs (with .previous backup)
    let raw_path = pipeline.config.raw_csv_path.clone();
    let omit = args.omit_patient_names;
    if pipeline.total_rows > 0 {
        eprintln!("\nGenerating output CSVs...");

        if !args.no_detailed {
            let path = pipeline.config.detailed_csv_path.clone();
            backup_file(&path);
            match compact_csv::generate_detailed(&raw_path, &path, omit) {
                Ok(n) => eprintln!("Detailed CSV: {} eye-visits → {}", n, path.display()),
                Err(e) => eprintln!("WARNING: Detailed CSV failed: {}", e),
            }
        }

        if !args.no_compact {
            let path = pipeline.config.compact_csv_path.clone();
            backup_file(&path);
            match compact_csv::generate_compact(&raw_path, &path, omit) {
                Ok(n) => eprintln!("Compact CSV: {} eye-visits → {}", n, path.display()),
                Err(e) => eprintln!("WARNING: Compact CSV failed: {}", e),
            }
        }
    }

    eprintln!("Total time: {:.1}s", t0.elapsed().as_secs_f64());

    // Backup (if specified)
    if let Some(ref backup_dir) = args.backup_dir {
        run_backup(&args.output_dir, backup_dir);
    }
}
