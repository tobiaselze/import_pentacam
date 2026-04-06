//! Core pipeline: dispatches input → DICOM/PDF/image processing → raw CSV output.
//!
//! Designed as a reusable struct (`PentacamPipeline`) that a CLI or future GUI
//! can call. The CLI is a thin wrapper around this module.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use pentacam_types::{DicomMeta, Laterality, PrintoutType, QaStatus};
use ocr_import::render::Renderer;

use crate::field_map::{self, DataSource};
use crate::logging::{ErrorLog, LogCategory};
use crate::raw_csv::{RawCsvWriter, RawRow, write_source_manifest};

/// Pentacam DICOM filename prefix (OID root for OCULUS devices).
const PENTACAM_PREFIX: &str = "1.3.6.1.4.1.34714.";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Pipeline configuration — set once at startup.
pub struct PipelineConfig {
    pub output_dir: PathBuf,
    pub omit_patient_names: bool,
    pub renderer: Renderer,
    pub raw_csv_path: PathBuf,
    pub compact_csv_path: PathBuf,
    pub processed_log_path: PathBuf,
    pub error_log_path: PathBuf,
    pub save_images: bool,
}

impl PipelineConfig {
    pub fn new(output_dir: PathBuf, omit_names: bool, renderer: Renderer) -> Self {
        let raw_csv_path = output_dir.join("pentacam_raw.csv");
        let compact_csv_path = output_dir.join("pentacam_compact.csv");
        let processed_log_path = output_dir.join("processed_folders.csv");
        let error_log_path = output_dir.join("errors.log");
        PipelineConfig {
            output_dir,
            omit_patient_names: omit_names,
            renderer,
            raw_csv_path,
            compact_csv_path,
            processed_log_path,
            error_log_path,
            save_images: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Processed folders log (for restart)
// ---------------------------------------------------------------------------

struct ProcessedLog {
    seen: HashSet<String>,
    writer: BufWriter<File>,
}

impl ProcessedLog {
    fn open(path: &Path) -> std::io::Result<Self> {
        let seen: HashSet<String> = if path.exists() {
            BufReader::new(File::open(path)?)
                .lines()
                .filter_map(|l| l.ok())
                .filter(|l| !l.trim().is_empty())
                .collect()
        } else {
            HashSet::new()
        };

        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        Ok(ProcessedLog {
            seen,
            writer: BufWriter::new(file),
        })
    }

    fn is_processed(&self, folder: &str) -> bool {
        self.seen.contains(folder)
    }

    fn mark_processed(&mut self, folder: &str) -> std::io::Result<()> {
        writeln!(self.writer, "{}", folder)?;
        self.writer.flush()?;
        self.seen.insert(folder.to_string());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Pipeline
// ---------------------------------------------------------------------------

/// The main pipeline struct. Holds all stateful resources.
pub struct PentacamPipeline {
    pub config: PipelineConfig,
    raw_csv: RawCsvWriter,
    processed_log: ProcessedLog,
    error_log: ErrorLog,
    // Per-scan source tracking: scan_hash → [(filename, page, printout_type, data_source)]
    scan_sources: HashMap<String, Vec<(String, u32, String, String)>>,
    // Buffered rows for current folder (written atomically after images are saved)
    pending_rows: Vec<RawRow>,
    // Counters
    pub files_processed: u32,
    pub folders_processed: u32,
    pub folders_skipped: u32,
    pub total_rows: u32,
}

impl PentacamPipeline {
    /// Create a new pipeline. Initializes OCR engine, opens output files.
    pub fn new(config: PipelineConfig) -> Result<Self, String> {
        // Create output directory
        fs::create_dir_all(&config.output_dir)
            .map_err(|e| format!("Create output dir: {}", e))?;

        // Create images directory
        if config.save_images {
            fs::create_dir_all(config.output_dir.join("images"))
                .map_err(|e| format!("Create images dir: {}", e))?;
        }

        // Open raw CSV writer
        let raw_csv = RawCsvWriter::open(&config.raw_csv_path)
            .map_err(|e| format!("Open raw CSV: {}", e))?;

        // Open processed log
        let processed_log = ProcessedLog::open(&config.processed_log_path)
            .map_err(|e| format!("Open processed log: {}", e))?;

        // Open error log
        let error_log = ErrorLog::open(Some(&config.error_log_path))
            .map_err(|e| format!("Open error log: {}", e))?;

        Ok(PentacamPipeline {
            config,
            raw_csv,
            processed_log,
            error_log,
            scan_sources: HashMap::new(),
            pending_rows: Vec::new(),
            files_processed: 0,
            folders_processed: 0,
            folders_skipped: 0,
            total_rows: 0,
        })
    }

    // -----------------------------------------------------------------------
    // Input dispatch
    // -----------------------------------------------------------------------

    /// Process any input: single file or directory (PACS mode).
    pub fn process_input(&mut self, input: &Path) {
        if input.is_dir() {
            self.process_pacs_directory(input);
        } else {
            self.process_single_file(input);
        }
    }

    /// Process a single DICOM, PDF, or image file.
    pub fn process_single_file(&mut self, path: &Path) {
        let ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "dcm" => self.process_dicom(path),
            "pdf" => self.process_pdf(path),
            "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" => {
                self.process_image(path);
            }
            _ => {
                self.error_log.warn(
                    LogCategory::DicomError,
                    &path.display().to_string(),
                    &format!("Unsupported file type: .{}", ext),
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // PACS directory mode
    // -----------------------------------------------------------------------

    /// Recursively scan a PACS directory, processing each subfolder.
    pub fn process_pacs_directory(&mut self, root: &Path) {
        eprintln!("Scanning PACS directory: {}", root.display());

        // Find all directories containing Pentacam DICOM files
        let folders = discover_pentacam_folders(root);
        eprintln!("Found {} folders with Pentacam files", folders.len());

        for (i, folder) in folders.iter().enumerate() {
            let folder_str = folder.display().to_string();

            if self.processed_log.is_processed(&folder_str) {
                self.folders_skipped += 1;
                continue;
            }

            let t0 = std::time::Instant::now();
            self.process_pacs_folder(folder);

            if let Err(e) = self.processed_log.mark_processed(&folder_str) {
                eprintln!("  WARNING: Failed to log processed folder: {}", e);
            }
            self.folders_processed += 1;

            if (i + 1) % 10 == 0 || i + 1 == folders.len() {
                eprintln!(
                    "[{}/{}] {:.1}s, {} files, {} rows (skipped {})",
                    i + 1, folders.len(),
                    t0.elapsed().as_secs_f64(),
                    self.files_processed, self.total_rows, self.folders_skipped,
                );
            }
        }
    }

    /// Process all Pentacam DICOM files in a single folder.
    fn process_pacs_folder(&mut self, folder: &Path) {
        let dcm_files: Vec<PathBuf> = fs::read_dir(folder)
            .into_iter()
            .flat_map(|rd| rd.into_iter())
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().and_then(|e| e.to_str()) == Some("dcm")
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with(PENTACAM_PREFIX))
                        .unwrap_or(false)
            })
            .collect();

        if dcm_files.is_empty() { return; }

        let folder_rel = folder.display().to_string();

        for dcm_path in &dcm_files {
            self.process_dicom_with_folder(dcm_path, &folder_rel);
        }

        // Atomic commit: images first, then manifests, then CSV, then mark processed
        // If we crash between any step, the folder is NOT marked processed and will
        // be fully re-done on restart.
        self.write_pending_manifests();  // 1. Source manifests to image dirs
        self.flush_pending_rows();       // 2. Buffered CSV rows + flush
        self.error_log.flush();
    }

    // -----------------------------------------------------------------------
    // DICOM processing
    // -----------------------------------------------------------------------

    fn process_dicom(&mut self, path: &Path) {
        let folder = path.parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        self.process_dicom_with_folder(path, &folder);
    }

    fn process_dicom_with_folder(&mut self, path: &Path, source_folder: &str) {
        let fname = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        // Open DICOM once
        let obj = match dicom_import::open(path) {
            Ok(o) => o,
            Err(e) => {
                self.error_log.warn(LogCategory::DicomError, fname, &e);
                return;
            }
        };

        // Extract metadata
        let meta = dicom_import::extract_metadata(&obj);
        let scan_hash = self.compute_scan_hash(&meta);

        // Base row template (shared across SR/SPR/OCR rows from this file)
        let base = self.make_base_row(&meta, source_folder, fname, &scan_hash);

        // --- SR extraction ---
        if let Some(sr) = dicom_import::extract_sr(&obj) {
            let (fields, confs) = field_map::map_sr_to_fields(&sr);
            if !fields.is_empty() {
                let mut row = base.clone();
                row.printout_type = "SR".to_string();
                row.page_number = 0;
                row.n_fields = fields.len() as u32;
                row.qa_status = "ok".to_string();
                row.fields = fields;
                row.confidences = confs;
                self.write_row(row);
            }
        }

        // --- SPR blob extraction ---
        if let Some(blob_bytes) = dicom_import::extract_blob(&obj) {
            match blob_import::extract_exact(&blob_bytes) {
                Ok(exact) => {
                    let mut row = base.clone();
                    row.printout_type = "SPR".to_string();
                    row.page_number = 0;
                    row.qa_status = "ok".to_string();
                    let mut fields = HashMap::new();
                    let mut confs = HashMap::new();
                    if let Some(hwtw) = exact.cornea_dia_mm {
                        fields.insert("HWTW".to_string(), hwtw);
                        confs.insert("HWTW".to_string(), 1.0);
                    }
                    if !exact.acd_mm.is_nan() {
                        fields.insert("AC_depth".to_string(), exact.acd_mm);
                        confs.insert("AC_depth".to_string(), 1.0);
                    }
                    row.n_fields = fields.len() as u32;
                    row.fields = fields;
                    row.confidences = confs;
                    if row.n_fields > 0 {
                        self.write_row(row);
                    }
                }
                Err(e) => {
                    self.error_log.warn(LogCategory::DicomError, fname, &format!("SPR: {}", e));
                }
            }
        }

        // --- PDF extraction + OCR ---
        let pdf_bytes = dicom_import::extract_pdf_bytes(&obj);
        drop(obj); // Free DICOM memory before GPU-intensive OCR

        if let Some(ref pdf) = pdf_bytes {
            match ocr_import::render::page_count(pdf, self.config.renderer) {
                Ok(n_pages) => {
                    for page in 1..=n_pages {
                        match ocr_import::render::render_pdf_page(pdf, page, 300, self.config.renderer) {
                            Ok(png_path) => {
                                // Run OCR
                                let ocr_items = ocr_import::ocr_engine::run_full_page(&png_path).ok();

                                if let Some(ref items) = ocr_items {
                                    // Detect printout type
                                    if let Some(pt) = ocr_import::printout_detect::detect_printout_type(items) {
                                        let pt_str = format!("{:?}", pt);

                                        // Save rendered page image
                                        self.save_page_image(&scan_hash, page, &pt_str, &png_path);

                                        // Extract maps
                                        match image::open(&png_path) {
                                            Ok(page_img) => {
                                                let maps = ocr_import::extract_maps::extract_maps(
                                                    &page_img, items, &pt_str,
                                                );
                                                if !maps.maps.is_empty() {
                                                    self.save_maps(&scan_hash, &maps);
                                                }
                                            }
                                            Err(e) => {
                                                eprintln!("    WARNING: failed to open page image for map extraction: {}", e);
                                            }
                                        }
                                    }
                                }

                                // Full pipeline processing (field extraction etc.)
                                if let Some(result) = ocr_import::process_page(
                                    &png_path, path, page as usize,
                                ) {
                                    let mut row = base.clone();
                                    row.page_number = page;
                                    row.printout_type = format!("{:?}", result.printout_type);
                                    row.qa_status = match &result.qa_status {
                                        QaStatus::Ok => "ok".to_string(),
                                        QaStatus::Incomplete { reason } => format!("incomplete: {}", reason),
                                    };
                                    row.n_fields = result.fields.len() as u32;
                                    row.fields = result.fields;
                                    row.confidences = result.confidences.into_iter()
                                        .map(|(k, v)| (k, v as f32))
                                        .collect();
                                    self.write_row(row);
                                }
                                let _ = fs::remove_file(&png_path);
                            }
                            Err(e) => {
                                self.error_log.warn(
                                    LogCategory::RenderFailure, fname,
                                    &format!("page {}: {}", page, e),
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    self.error_log.warn(LogCategory::RenderFailure, fname, &e);
                }
            }
        }

        self.files_processed += 1;
    }

    // -----------------------------------------------------------------------
    // PDF / image processing (standalone, no DICOM)
    // -----------------------------------------------------------------------

    fn process_pdf(&mut self, path: &Path) {
        let fname = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let folder = path.parent().map(|p| p.display().to_string()).unwrap_or_default();

        let pdf_bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.error_log.warn(LogCategory::DicomError, fname, &format!("Read: {}", e));
                return;
            }
        };

        let base = RawRow {
            patient_id: String::new(),
            patient_name: String::new(),
            dob: String::new(),
            sex: String::new(),
            eye: String::new(),
            exam_date: String::new(),
            exam_time: String::new(),
            source_folder: folder,
            source_file: fname.to_string(),
            page_number: 0,
            printout_type: String::new(),
            qa_status: String::new(),
            n_fields: 0,
            device_serial: String::new(),
            software_version: String::new(),
            scan_hash: String::new(),
            fields: HashMap::new(),
            confidences: HashMap::new(),
        };

        match ocr_import::render::page_count(&pdf_bytes, self.config.renderer) {
            Ok(n_pages) => {
                for page in 1..=n_pages {
                    match ocr_import::render::render_pdf_page(&pdf_bytes, page, 300, self.config.renderer) {
                        Ok(png_path) => {
                            if let Some(result) = ocr_import::process_page(
                                &png_path, path, page as usize,
                            ) {
                                let mut row = base.clone();
                                row.page_number = page;
                                row.printout_type = format!("{:?}", result.printout_type);
                                row.qa_status = match &result.qa_status {
                                    QaStatus::Ok => "ok".to_string(),
                                    QaStatus::Incomplete { reason } => format!("incomplete: {}", reason),
                                };
                                row.n_fields = result.fields.len() as u32;
                                row.fields = result.fields;
                                row.confidences = result.confidences.into_iter()
                                    .map(|(k, v)| (k, v as f32))
                                    .collect();
                                self.write_row(row);
                            }
                            let _ = fs::remove_file(&png_path);
                        }
                        Err(e) => {
                            self.error_log.warn(
                                LogCategory::RenderFailure, fname,
                                &format!("page {}: {}", page, e),
                            );
                        }
                    }
                }
            }
            Err(e) => {
                self.error_log.warn(LogCategory::RenderFailure, fname, &e);
            }
        }

        self.files_processed += 1;
    }

    fn process_image(&mut self, path: &Path) {
        let fname = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let folder = path.parent().map(|p| p.display().to_string()).unwrap_or_default();

        if let Some(result) = ocr_import::process_page(path, path, 1) {
            let mut row = RawRow {
                patient_id: String::new(),
                patient_name: String::new(),
                dob: String::new(),
                sex: String::new(),
                eye: String::new(),
                exam_date: String::new(),
                exam_time: String::new(),
                source_folder: folder,
                source_file: fname.to_string(),
                page_number: 1,
                printout_type: format!("{:?}", result.printout_type),
                qa_status: match &result.qa_status {
                    QaStatus::Ok => "ok".to_string(),
                    QaStatus::Incomplete { reason } => format!("incomplete: {}", reason),
                },
                n_fields: result.fields.len() as u32,
                device_serial: String::new(),
                software_version: String::new(),
                scan_hash: String::new(),
                fields: result.fields,
                confidences: result.confidences.into_iter()
                    .map(|(k, v)| (k, v as f32))
                    .collect(),
            };
            self.write_row(row);
        }
        self.files_processed += 1;
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn write_row(&mut self, row: RawRow) {
        // Track source for manifest
        if !row.scan_hash.is_empty() {
            self.scan_sources
                .entry(row.scan_hash.clone())
                .or_default()
                .push((
                    row.source_file.clone(),
                    row.page_number,
                    row.printout_type.clone(),
                    if row.printout_type == "SR" { "DICOM_SR".to_string() }
                    else if row.printout_type == "SPR" { "SPR_blob".to_string() }
                    else { "OCR".to_string() },
                ));
        }

        // Buffer the row — written to CSV after images are complete
        self.pending_rows.push(row);
    }

    /// Write all buffered rows to the raw CSV. Called after images are saved.
    fn flush_pending_rows(&mut self) {
        for row in self.pending_rows.drain(..) {
            if let Err(e) = self.raw_csv.write_row(&row) {
                eprintln!("  WARNING: Failed to write CSV row: {}", e);
            }
            self.total_rows += 1;
        }
        if let Err(e) = self.raw_csv.flush() {
            eprintln!("  WARNING: CSV flush failed: {}", e);
        }
    }

    /// Get or create the image directory for a scan hash.
    fn image_dir(&self, scan_hash: &str) -> PathBuf {
        let dir = self.config.output_dir.join("images").join(scan_hash);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    /// Save a rendered page image (de-identified) to the scan's image directory.
    fn save_page_image(
        &self,
        scan_hash: &str,
        page_num: u32,
        printout_type: &str,
        img_path: &Path,
    ) {
        if !self.config.save_images || scan_hash.is_empty() { return; }
        let dir = self.image_dir(scan_hash);
        // Use a short printout type name for the filename
        let type_short = printout_type.replace("FourMaps", "4maps_")
            .replace("Refractive", "refr")
            .replace("Selectable", "sel")
            .replace("TopometricKcStaging", "topo")
            .replace("BelinAmbrosio", "belin")
            .replace("BelinAbcdProgression", "belin_abcd")
            .replace(' ', "_")
            .to_lowercase();
        let dst = dir.join(format!("page{}_{}.png", page_num, type_short));
        let _ = fs::copy(img_path, &dst);
    }

    /// Save extracted map images to the scan's image directory.
    fn save_maps(
        &self,
        scan_hash: &str,
        maps: &ocr_import::extract_maps::ExtractedMaps,
    ) {
        if !self.config.save_images || scan_hash.is_empty() { return; }
        let dir = self.image_dir(scan_hash);
        for (name, img) in &maps.maps {
            let dst = dir.join(format!("map_{}.png", name));
            let _ = img.save(&dst);
        }
    }

    fn compute_scan_hash(&self, meta: &DicomMeta) -> String {
        use pentacam_types::EyeVisitKey;
        let patient_id = meta.patient_id.clone().unwrap_or_default();
        let eye = meta.laterality.unwrap_or(Laterality::OD);
        // Parse exam_date + exam_time to epoch
        let epoch = parse_exam_epoch(
            meta.exam_date.as_deref().unwrap_or(""),
            meta.exam_time.as_deref().unwrap_or(""),
        );
        let key = EyeVisitKey {
            patient_id,
            eye,
            exam_epoch_secs: epoch,
        };
        key.dir_hash()
    }

    fn make_base_row(&self, meta: &DicomMeta, source_folder: &str, source_file: &str, scan_hash: &str) -> RawRow {
        RawRow {
            patient_id: meta.patient_id.clone().unwrap_or_default(),
            patient_name: if self.config.omit_patient_names {
                String::new()
            } else {
                meta.patient_name.clone().unwrap_or_default()
            },
            dob: meta.date_of_birth.clone().unwrap_or_default(),
            sex: meta.sex.clone().unwrap_or_default(),
            eye: meta.laterality.as_ref().map(|l| format!("{}", l)).unwrap_or_default(),
            exam_date: meta.exam_date.clone().unwrap_or_default(),
            exam_time: meta.exam_time.clone().unwrap_or_default(),
            source_folder: source_folder.to_string(),
            source_file: source_file.to_string(),
            page_number: 0,
            printout_type: String::new(),
            qa_status: String::new(),
            n_fields: 0,
            device_serial: meta.device_serial.clone().unwrap_or_default(),
            software_version: meta.software_version.clone().unwrap_or_default(),
            scan_hash: scan_hash.to_string(),
            fields: HashMap::new(),
            confidences: HashMap::new(),
        }
    }

    /// Write source manifests for accumulated scans, then clear the buffer.
    fn write_pending_manifests(&mut self) {
        if !self.config.save_images { return; }
        for (hash, entries) in self.scan_sources.drain() {
            if entries.is_empty() { continue; }
            let dir = self.config.output_dir.join("images").join(&hash);
            let _ = fs::create_dir_all(&dir);
            if let Err(e) = write_source_manifest(&dir, &entries) {
                eprintln!("  WARNING: Failed to write source manifest: {}", e);
            }
        }
    }

    /// Print final summary and flush all logs.
    pub fn finish(&mut self) {
        self.write_pending_manifests();
        self.flush_pending_rows();
        self.error_log.flush();
        self.error_log.print_summary();
        eprintln!(
            "\nDone: {} files, {} folders ({} skipped), {} raw CSV rows",
            self.files_processed, self.folders_processed, self.folders_skipped, self.total_rows,
        );
        eprintln!("Output: {}", self.config.raw_csv_path.display());

        // Clean up temp directory
        ocr_import::cleanup_temp();
    }
}

// ---------------------------------------------------------------------------
// PACS folder discovery
// ---------------------------------------------------------------------------

/// Find all directories containing Pentacam DICOM files.
fn discover_pentacam_folders(root: &Path) -> Vec<PathBuf> {
    let mut folders = HashSet::new();

    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("dcm") {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with(PENTACAM_PREFIX) {
                    if let Some(parent) = path.parent() {
                        folders.insert(parent.to_path_buf());
                    }
                }
            }
        }
    }

    let mut sorted: Vec<PathBuf> = folders.into_iter().collect();
    sorted.sort();
    sorted
}

/// Parse exam date + time into epoch seconds (best effort).
fn parse_exam_epoch(date: &str, time: &str) -> i64 {
    // DICOM date: YYYYMMDD, time: HHMMSS or HHMMSS.ffffff
    if date.len() < 8 { return 0; }
    let y: i64 = date[0..4].parse().unwrap_or(2000);
    let m: i64 = date[4..6].parse().unwrap_or(1);
    let d: i64 = date[6..8].parse().unwrap_or(1);
    let h: i64 = if time.len() >= 2 { time[0..2].parse().unwrap_or(0) } else { 0 };
    let min: i64 = if time.len() >= 4 { time[2..4].parse().unwrap_or(0) } else { 0 };
    let s: i64 = if time.len() >= 6 { time[4..6].parse().unwrap_or(0) } else { 0 };

    // Rough epoch (not exact, but deterministic and unique enough for grouping)
    ((y - 1970) * 365 + (m - 1) * 30 + d) * 86400 + h * 3600 + min * 60 + s
}