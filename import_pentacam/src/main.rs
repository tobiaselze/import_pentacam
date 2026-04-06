//! import_pentacam — Extract clinical measurements from Pentacam DICOM/PDF/image files.
//!
//! Supports single files (DICOM, PDF, image) and PACS directory scanning.
//! Outputs raw CSV (incremental, per-page) and compact CSV (best value per eye-visit).

use clap::Parser;
use std::path::PathBuf;

mod eye_visit;
pub mod field_map;
pub mod logging;
pub mod raw_csv;
pub mod compact_csv;
pub mod pipeline;

use ocr_import::render::Renderer;
use pipeline::{PipelineConfig, PentacamPipeline};

#[derive(Parser)]
#[command(name = "import_pentacam")]
#[command(about = "Extract clinical measurements from Pentacam DICOM/PDF/image files")]
#[command(version)]
struct Args {
    /// DICOM file, PDF file, image file, or PACS directory to process
    input: PathBuf,

    /// Output directory for results and extracted images
    #[arg(short, long, default_value = "pentacam_output")]
    output_dir: PathBuf,

    /// Processed folders log for restart (default: <output_dir>/processed_folders.csv)
    #[arg(short, long)]
    processed_log: Option<PathBuf>,

    /// Error/warning log file (default: <output_dir>/errors.log)
    #[arg(short, long)]
    error_log: Option<PathBuf>,

    /// Strip patient names from output
    #[arg(short = 'x', long)]
    omit_patient_names: bool,

    /// Log relative paths instead of absolute
    #[arg(short = 'r', long)]
    log_relative_paths: bool,

    /// Use MuPDF renderer (default)
    #[arg(long)]
    mupdf: bool,

    /// Use Poppler renderer
    #[arg(long)]
    poppler: bool,

    /// Disable CUDA GPU acceleration
    #[arg(long)]
    no_gpu: bool,

    /// Skip raw CSV, only produce compact output
    #[arg(long)]
    compact_only: bool,
}

fn main() {
    let args = Args::parse();

    // Determine renderer
    let renderer = if args.poppler {
        eprintln!("Using Poppler renderer");
        Renderer::Poppler
    } else {
        eprintln!("Using MuPDF renderer");
        Renderer::MuPdf
    };

    // Initialize OCR engine
    let model_dir = PathBuf::from("models");
    ocr_import::ocr_engine::init(
        model_dir.join("pp-ocrv5_server_det.onnx").to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).expect("Failed to initialize OCR engine");

    // Build pipeline config
    let config = PipelineConfig::new(
        args.output_dir,
        args.omit_patient_names,
        renderer,
    );

    // Create and run pipeline
    let mut pipeline = PentacamPipeline::new(config)
        .expect("Failed to initialize pipeline");

    let t0 = std::time::Instant::now();
    pipeline.process_input(&args.input);
    pipeline.finish();

    // Generate compact CSV
    let compact_path = pipeline.config.compact_csv_path.clone();
    let raw_path = pipeline.config.raw_csv_path.clone();
    if pipeline.total_rows > 0 {
        eprintln!("\nGenerating compact CSV...");
        match compact_csv::generate_compact(&raw_path, &compact_path) {
            Ok(n) => eprintln!("Compact CSV: {} eye-visits → {}", n, compact_path.display()),
            Err(e) => eprintln!("WARNING: Compact CSV generation failed: {}", e),
        }
    }

    eprintln!("Total time: {:.1}s", t0.elapsed().as_secs_f64());
}
