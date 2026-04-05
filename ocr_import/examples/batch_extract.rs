//! Batch extraction: process DICOMs/PDFs/images end-to-end, output CSV.
//!
//! Usage:
//!   cargo run -p ocr_import --example batch_extract --release -- \
//!     <input_dir_or_file_or_list.txt> --output results.csv [--mupdf]

use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use pentacam_types::{DicomMeta, DicomSrValues};
use ocr_import::ocr_engine;
use ocr_import::render::{self, Renderer};

// ---------------------------------------------------------------------------
// Input abstraction — works for DICOM, PDF, and image files
// ---------------------------------------------------------------------------

struct InputFile {
    path: PathBuf,
    dicom_meta: Option<DicomMeta>,
    dicom_sr: Option<DicomSrValues>,
    blob_hwtw: Option<f64>,
    blob_acd: Option<f64>,
    pages: Vec<(image::RgbImage, u32)>,  // (image, page_number)
}

fn load_input(path: &Path, renderer: Renderer) -> Result<InputFile, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "dcm" => load_dicom(path, renderer),
        "pdf" => load_pdf(path, renderer),
        _ => load_image(path),
    }
}

fn load_dicom(path: &Path, renderer: Renderer) -> Result<InputFile, String> {
    // Open DICOM ONCE, extract everything from the same object
    let obj = dicom_import::open(path)?;
    let meta = dicom_import::extract_metadata(&obj);
    let sr = dicom_import::extract_sr(&obj);
    let pdf_bytes = dicom_import::extract_pdf_bytes(&obj);
    let blob_bytes = dicom_import::extract_blob(&obj);
    drop(obj); // done with DICOM

    let (blob_hwtw, blob_acd) = if let Some(ref blob) = blob_bytes {
        match blob_import::extract_exact(blob) {
            Ok(exact) => (exact.cornea_dia_mm,
                          if exact.acd_mm.is_nan() { None } else { Some(exact.acd_mm) }),
            Err(_) => (None, None),
        }
    } else { (None, None) };

    let pages = match pdf_bytes {
        Some(ref pdf) => render_pdf_pages(pdf, renderer)?,
        None => return Err("No embedded PDF".to_string()),
    };

    Ok(InputFile { path: path.to_path_buf(), dicom_meta: Some(meta), dicom_sr: sr,
                   blob_hwtw, blob_acd, pages })
}

fn load_pdf(path: &Path, renderer: Renderer) -> Result<InputFile, String> {
    let pdf_bytes = fs::read(path).map_err(|e| format!("Read PDF: {}", e))?;
    let pages = render_pdf_pages(&pdf_bytes, renderer)?;
    Ok(InputFile { path: path.to_path_buf(), dicom_meta: None, dicom_sr: None,
                   blob_hwtw: None, blob_acd: None, pages })
}

fn load_image(path: &Path) -> Result<InputFile, String> {
    let img = image::open(path).map_err(|e| format!("Load image: {}", e))?.to_rgb8();
    Ok(InputFile { path: path.to_path_buf(), dicom_meta: None, dicom_sr: None,
                   blob_hwtw: None, blob_acd: None, pages: vec![(img, 1)] })
}

fn render_pdf_pages(pdf_bytes: &[u8], renderer: Renderer) -> Result<Vec<(image::RgbImage, u32)>, String> {
    let n = render::page_count(pdf_bytes, renderer)?;
    let mut pages = Vec::new();
    for p in 1..=n {
        let png_path = render::render_pdf_page(pdf_bytes, p, 300, renderer)?;
        let img = image::open(&png_path).map_err(|e| format!("Load page: {}", e))?.to_rgb8();
        let _ = fs::remove_file(&png_path);
        pages.push((img, p));
    }
    Ok(pages)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: batch_extract <input> [--output results.csv] [--mupdf]");
        std::process::exit(1);
    }
    let input = PathBuf::from(&args[1]);
    let output_csv = args.iter().position(|a| a == "--output")
        .and_then(|i| args.get(i + 1)).map(|s| PathBuf::from(s))
        .unwrap_or_else(|| PathBuf::from("pentacam_results.csv"));
    let renderer = if args.iter().any(|a| a == "--mupdf") {
        eprintln!("Using MuPDF renderer"); Renderer::MuPdf
    } else {
        eprintln!("Using Poppler renderer"); Renderer::Poppler
    };

    let model_dir = PathBuf::from("models");
    ocr_engine::init(
        model_dir.join("pp-ocrv5_server_det.onnx").to_str().unwrap(),
        model_dir.join("en_pp-ocrv5_mobile_rec.onnx").to_str().unwrap(),
        model_dir.join("en_ppocrv5_dict.txt").to_str().unwrap(),
    ).expect("Failed to initialize OCR engine");

    let files = discover_files(&input);
    eprintln!("Found {} files to process", files.len());

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
        "TNP_Astig","TNP_K1","TNP_Axis","TNP_K2","TNP_PMax","TNP_Km",
        "Belin_K1","Belin_K2","Belin_KMax","Belin_Axis","Belin_Qval","Belin_QS",
        "Belin_PachyThin","Belin_DistVertex","Belin_F_Ele_Th","Belin_B_Ele_Th",
        "Belin_Prog_Min","Belin_Prog_Max","Belin_Prog_Avg","Belin_ARTmax",
        "Belin_Df","Belin_Db","Belin_Dp","Belin_Dt","Belin_Da","Belin_D_final",
    ];

    let mut csv = File::create(&output_csv).expect("Can't create output CSV");
    let mut header = "filename,page,printout_type,n_fields,qa_status,\
        patient_id,patient_name,dob,sex,exam_date,exam_time,\
        laterality,software_version,device_serial,sr_values,\
        blob_HWTW,blob_ACD".to_string();
    for f in &all_fields { header.push(','); header.push_str(f); }
    for f in &all_fields { header.push(','); header.push_str(f); header.push_str("_conf"); }
    writeln!(csv, "{}", header).unwrap();

    let start = Instant::now();
    let mut total_pages = 0u32;
    let mut total_fields = 0u32;

    for (i, file_path) in files.iter().enumerate() {
        let input_file = match load_input(file_path, renderer) {
            Ok(f) => f,
            Err(e) => { eprintln!("  ERROR {}: {}", file_path.display(), e); continue; }
        };

        let meta_str = format_meta(&input_file.dicom_meta);
        let sr_str = format_sr(&input_file.dicom_sr);
        let blob_str = format_blob(input_file.blob_hwtw, input_file.blob_acd);

        for (page_img, page_num) in &input_file.pages {
            // Save to temp for OCR (TODO: use run_full_page_mem)
            let tmp = ocr_import::temp_path(&format!("batch_p{}.png", page_num));
            image::DynamicImage::ImageRgb8(page_img.clone()).save(&tmp).unwrap();
            let result = ocr_import::process_page(&tmp, file_path, *page_num as usize);
            let _ = fs::remove_file(&tmp);

            if let Some(ref res) = result {
                let fname = file_path.file_name().unwrap().to_str().unwrap();
                let pt = format!("{:?}", res.printout_type);
                let qa = match &res.qa_status {
                    pentacam_types::QaStatus::Ok => "ok".to_string(),
                    pentacam_types::QaStatus::Incomplete { reason } => format!("incomplete: {}", reason),
                };
                total_fields += res.fields.len() as u32;

                let mut row = format!("{},{},{},{},{},{},{},{}",
                    fname, page_num, pt, res.fields.len(), qa, meta_str, sr_str, blob_str);
                for f in &all_fields {
                    row.push(',');
                    if let Some(v) = res.fields.get(*f) { row.push_str(&format!("{}", v)); }
                }
                for f in &all_fields {
                    row.push(',');
                    if let Some(v) = res.confidences.get(*f) { row.push_str(&format!("{:.4}", v)); }
                }
                writeln!(csv, "{}", row).unwrap();
            }
            total_pages += 1;
        }

        if (i + 1) % 10 == 0 || i + 1 == files.len() {
            eprintln!("[{}/{}] {:.1}s, {} pages, {} fields",
                i + 1, files.len(), start.elapsed().as_secs_f64(), total_pages, total_fields);
        }
    }

    csv.flush().unwrap();
    eprintln!("\nDone: {} files, {} pages, {} fields in {:.1}s\nOutput: {}",
        files.len(), total_pages, total_fields, start.elapsed().as_secs_f64(), output_csv.display());

    // Clean up session temp directory
    ocr_import::cleanup_temp();
}

fn format_meta(meta: &Option<DicomMeta>) -> String {
    if let Some(m) = meta {
        format!("{},{},{},{},{},{},{},{},{}",
            m.patient_id.as_deref().unwrap_or(""),
            m.patient_name.as_deref().unwrap_or(""),
            m.date_of_birth.as_deref().unwrap_or(""),
            m.sex.as_deref().unwrap_or(""),
            m.exam_date.as_deref().unwrap_or(""),
            m.exam_time.as_deref().unwrap_or(""),
            m.laterality.as_ref().map(|l| format!("{}", l)).unwrap_or_default(),
            m.software_version.as_deref().unwrap_or(""),
            m.device_serial.as_deref().unwrap_or(""))
    } else { ",,,,,,,,".to_string() }
}

fn format_sr(sr: &Option<DicomSrValues>) -> String {
    if let Some(vals) = sr {
        let s = vals.iter().map(|(k, v)| format!("{}={}", k, v)).collect::<Vec<_>>().join(";");
        format!("\"{}\"", s)
    } else { String::new() }
}

fn format_blob(hwtw: Option<f64>, acd: Option<f64>) -> String {
    let h = hwtw.map(|v| format!("{:.2}", v)).unwrap_or_default();
    let a = acd.map(|v| format!("{:.2}", v)).unwrap_or_default();
    format!("{},{}", h, a)
}

fn discover_files(input: &Path) -> Vec<PathBuf> {
    if input.is_file() {
        if input.extension().and_then(|e| e.to_str()) == Some("txt") {
            return fs::read_to_string(input).expect("Can't read file list")
                .lines().filter(|l| !l.trim().is_empty())
                .map(|l| PathBuf::from(l.trim())).filter(|p| p.exists()).collect();
        }
        return vec![input.to_path_buf()];
    }
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(input).into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| matches!(e.path().extension().and_then(|e| e.to_str())
            .unwrap_or("").to_lowercase().as_str(), "dcm"|"pdf"|"png"|"jpg"|"jpeg"))
        .map(|e| e.path().to_path_buf()).collect();
    files.sort(); files
}
