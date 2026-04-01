//! Batch extraction: process DICOMs/PDFs end-to-end, output CSV.
//!
//! Usage:
//!   ORT_LIB_LOCATION=/tmp/onnxruntime-linux-x64-gpu-1.20.1/lib ORT_PREFER_DYNAMIC_LINK=1 \
//!   CUDA_VISIBLE_DEVICES=1 \
//!   cargo run -p ocr_import --example batch_extract --release -- \
//!     <input_dir_or_file> --output results.csv [--mobile]

use std::collections::HashMap;
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use ocr_import::ocr_engine;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: batch_extract <input_dir_or_file> [--output results.csv] [--mobile]");
        std::process::exit(1);
    }
    let input = PathBuf::from(&args[1]);
    let output_csv = args.iter()
        .position(|a| a == "--output")
        .and_then(|i| args.get(i + 1))
        .map(|s| PathBuf::from(s))
        .unwrap_or_else(|| PathBuf::from("pentacam_results.csv"));
    let use_mobile = args.iter().any(|a| a == "--mobile");

    // Initialize OCR
    let model_dir = PathBuf::from("models");
    let det_model = if use_mobile { "pp-ocrv5_mobile_det.onnx" } else { "pp-ocrv5_server_det.onnx" };
    ocr_engine::init(
        model_dir.join(det_model).to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).expect("Failed to initialize OCR engine");

    // Discover files
    let files = discover_files(&input);
    eprintln!("Found {} files to process", files.len());

    // All known fields in output order
    let all_fields = [
        "Rf_front","Rs_front","Rm_front","K1_front","K2_front","Km_front",
        "Astig_front","Axis_front","Qval_front","Rmin_front","Rper_front",
        "Rf_back","Rs_back","Rm_back","K1_back","K2_back","Km_back",
        "Astig_back","Axis_back","Qval_back","Rmin_back","Rper_back",
        "PupilCenter","PupilCenter_x","PupilCenter_y",
        "PachyVertex","PachyVertex_x","PachyVertex_y",
        "Thinnest","Thinnest_x","Thinnest_y",
        "Kmax","Kmax_x","Kmax_y",
        "CorneaVol","HWTW","ChamberVol","Angle","AC_depth","PupilDia",
        // TNP fields (Topometric only)
        "TNP_Astig","TNP_K1","TNP_Axis","TNP_K2","TNP_PMax","TNP_Km",
        // Belin fields
        "Belin_K1","Belin_K2","Belin_KMax","Belin_Axis","Belin_Qval","Belin_QS",
        "Belin_PachyThin","Belin_DistVertex","Belin_F_Ele_Th","Belin_B_Ele_Th",
        "Belin_Prog_Min","Belin_Prog_Max","Belin_Prog_Avg","Belin_ARTmax",
        "Belin_Df","Belin_Db","Belin_Dp","Belin_Dt","Belin_Da","Belin_D_final",
    ];

    // Open CSV
    let mut csv = File::create(&output_csv).expect("Can't create output CSV");

    // Header
    let mut header = "filename,page,printout_type,n_fields,qa_status".to_string();
    for f in &all_fields {
        header.push(',');
        header.push_str(f);
    }
    for f in &all_fields {
        header.push(',');
        header.push_str(f);
        header.push_str("_conf");
    }
    writeln!(csv, "{}", header).unwrap();

    let start = Instant::now();
    let mut total_pages = 0u32;
    let mut total_fields = 0u32;

    for (i, file_path) in files.iter().enumerate() {
        let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

        let pages: Vec<(PathBuf, u32)> = match ext.as_str() {
            "dcm" => {
                match extract_and_render_pdf(file_path) {
                    Ok(pages) => pages,
                    Err(e) => {
                        eprintln!("  ERROR {}: {}", file_path.display(), e);
                        continue;
                    }
                }
            }
            "pdf" => {
                let n = get_pdf_page_count(file_path);
                (1..=n).map(|p| (render_pdf_to_png(file_path, p), p)).collect()
            }
            "png" | "jpg" | "jpeg" => {
                vec![(file_path.clone(), 1)]
            }
            _ => continue,
        };

        for (png_path, page_num) in &pages {
            let result = ocr_import::process_page(png_path, file_path, *page_num as usize);

            if let Some(ref res) = result {
                let fname = file_path.file_name().unwrap().to_str().unwrap();
                let pt = format!("{:?}", res.printout_type);
                let qa = match &res.qa_status {
                    pentacam_types::QaStatus::Ok => "ok".to_string(),
                    pentacam_types::QaStatus::Incomplete { reason } => format!("incomplete: {}", reason),
                };
                let n = res.fields.len();
                total_fields += n as u32;

                let mut row = format!("{},{},{},{},{}", fname, page_num, pt, n, qa);
                for f in &all_fields {
                    row.push(',');
                    if let Some(v) = res.fields.get(*f) {
                        row.push_str(&format!("{}", v));
                    }
                }
                for f in &all_fields {
                    row.push(',');
                    if let Some(v) = res.confidences.get(*f) {
                        row.push_str(&format!("{:.4}", v));
                    }
                }
                writeln!(csv, "{}", row).unwrap();
            }

            total_pages += 1;
        }

        if (i + 1) % 10 == 0 || i + 1 == files.len() {
            let elapsed = start.elapsed().as_secs_f64();
            eprintln!("[{}/{}] {:.1}s elapsed, {} pages, {} fields",
                i + 1, files.len(), elapsed, total_pages, total_fields);
        }
    }

    csv.flush().unwrap();
    let elapsed = start.elapsed().as_secs_f64();
    eprintln!("\nDone: {} files, {} pages, {} fields in {:.1}s",
        files.len(), total_pages, total_fields, elapsed);
    eprintln!("Output: {}", output_csv.display());
}

fn discover_files(input: &Path) -> Vec<PathBuf> {
    // If it's a .txt file, treat as a file list (one path per line)
    if input.is_file() {
        if input.extension().and_then(|e| e.to_str()) == Some("txt") {
            return fs::read_to_string(input)
                .expect("Can't read file list")
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| PathBuf::from(l.trim()))
                .filter(|p| p.exists())
                .collect();
        }
        return vec![input.to_path_buf()];
    }
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(input)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let ext = e.path().extension().and_then(|e| e.to_str()).unwrap_or("");
            matches!(ext.to_lowercase().as_str(), "dcm" | "pdf" | "png" | "jpg" | "jpeg")
        })
        .map(|e| e.path().to_path_buf())
        .collect();
    files.sort();
    files
}

fn extract_and_render_pdf(dcm_path: &Path) -> Result<Vec<(PathBuf, u32)>, String> {
    let pdf_bytes = dicom_import::extract_pdf_bytes(dcm_path)?
        .ok_or_else(|| "No embedded PDF".to_string())?;
    let pdf_path = PathBuf::from("/tmp/_batch_pentacam.pdf");
    fs::write(&pdf_path, &pdf_bytes).map_err(|e| format!("Write PDF: {}", e))?;
    let n_pages = get_pdf_page_count(&pdf_path);
    let mut pages = Vec::new();
    for p in 1..=n_pages {
        pages.push((render_pdf_to_png(&pdf_path, p), p));
    }
    Ok(pages)
}

fn get_pdf_page_count(pdf_path: &Path) -> u32 {
    let output = Command::new("pdfinfo").arg(pdf_path).output().unwrap_or_else(|_| {
        std::process::Command::new("true").output().unwrap()
    });
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.starts_with("Pages:") {
            if let Some(n) = line.split_whitespace().last() {
                return n.parse().unwrap_or(1);
            }
        }
    }
    1
}

fn render_pdf_to_png(pdf_path: &Path, page: u32) -> PathBuf {
    let out_prefix = "/tmp/_batch_pentacam_page";
    if let Ok(entries) = glob::glob(&format!("{}*.png", out_prefix)) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry);
        }
    }
    let _ = Command::new("pdftoppm")
        .args(["-r", "300", "-png", "-f", &page.to_string(), "-l", &page.to_string(),
               pdf_path.to_str().unwrap(), out_prefix])
        .status();
    let mut pages: Vec<PathBuf> = glob::glob(&format!("{}*.png", out_prefix))
        .unwrap()
        .filter_map(|p| p.ok())
        .collect();
    pages.sort();
    pages.first().cloned().unwrap_or_else(|| PathBuf::from("/dev/null"))
}
