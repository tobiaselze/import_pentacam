//! Standalone tool to regenerate compact and detailed CSVs from an existing raw CSV.
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: regenerate_csvs <output_dir> [-x]");
        eprintln!("  Reads pentacam_raw.csv from <output_dir> and regenerates");
        eprintln!("  pentacam_compact.csv and pentacam_detailed.csv");
        std::process::exit(1);
    }

    let output_dir = PathBuf::from(&args[1]);
    let omit_names = args.iter().any(|a| a == "-x");

    let raw_path = output_dir.join("pentacam_raw.csv");
    if !raw_path.exists() {
        eprintln!("ERROR: {} not found", raw_path.display());
        std::process::exit(1);
    }

    eprintln!("Regenerating from {}", raw_path.display());

    let detailed_path = output_dir.join("pentacam_detailed.csv");
    match import_pentacam::compact_csv::generate_detailed(&raw_path, &detailed_path, omit_names) {
        Ok(n) => eprintln!("Detailed CSV: {} eye-visits → {}", n, detailed_path.display()),
        Err(e) => eprintln!("WARNING: Detailed CSV failed: {}", e),
    }

    let compact_path = output_dir.join("pentacam_compact.csv");
    match import_pentacam::compact_csv::generate_compact(&raw_path, &compact_path, omit_names) {
        Ok(n) => eprintln!("Compact CSV: {} eye-visits → {}", n, compact_path.display()),
        Err(e) => eprintln!("WARNING: Compact CSV failed: {}", e),
    }
}
