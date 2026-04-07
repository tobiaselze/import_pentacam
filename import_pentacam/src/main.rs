//! import_pentacam — Extract clinical measurements from Pentacam DICOM/PDF/image files.
//!
//! Input modes:
//!   - Single file (DICOM, PDF, image)
//!   - File list (.txt with one path per line)
//!   - PACS directory (recursive scan for Pentacam DICOMs)
//!
//! Output:
//!   - pentacam_raw.csv     — one row per extraction event (SR, SPR, OCR page)
//!   - pentacam_compact.csv — one row per eye-visit, best values only
//!   - pentacam_detailed.csv — one row per eye-visit, full per-field metadata

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
    /// DICOM file, PDF file, image file, file list (.txt), or PACS directory
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

    /// Use Poppler renderer instead of MuPDF (default)
    #[arg(long)]
    poppler: bool,

    /// Error/warning log file (default: <output_dir>/errors.log)
    #[arg(short, long)]
    error_log: Option<PathBuf>,
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
        args.save_pages,
        !args.no_images,
    );

    // Create and run pipeline
    let mut pipeline = PentacamPipeline::new(config)
        .expect("Failed to initialize pipeline");

    let t0 = std::time::Instant::now();
    pipeline.process_input(&args.input);
    pipeline.finish();

    // Generate output CSVs
    let raw_path = pipeline.config.raw_csv_path.clone();
    let omit = args.omit_patient_names;
    if pipeline.total_rows > 0 {
        eprintln!("\nGenerating output CSVs...");

        if !args.no_detailed {
            let path = pipeline.config.detailed_csv_path.clone();
            match compact_csv::generate_detailed(&raw_path, &path, omit) {
                Ok(n) => eprintln!("Detailed CSV: {} eye-visits → {}", n, path.display()),
                Err(e) => eprintln!("WARNING: Detailed CSV failed: {}", e),
            }
        }

        if !args.no_compact {
            let path = pipeline.config.compact_csv_path.clone();
            match compact_csv::generate_compact(&raw_path, &path, omit) {
                Ok(n) => eprintln!("Compact CSV: {} eye-visits → {}", n, path.display()),
                Err(e) => eprintln!("WARNING: Compact CSV failed: {}", e),
            }
        }
    }

    eprintln!("Total time: {:.1}s", t0.elapsed().as_secs_f64());
}
