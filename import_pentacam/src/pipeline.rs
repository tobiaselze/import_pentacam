//! Core pipeline: dispatches input → DICOM/PDF/image processing → raw CSV output.
//!
//! Designed as a reusable struct (`PentacamPipeline`) that a CLI or future GUI
//! can call. The CLI is a thin wrapper around this module.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use image::GenericImageView;
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
    pub detailed_csv_path: PathBuf,
    pub compact_csv_path: PathBuf,
    pub processed_log_path: PathBuf,
    pub error_log_path: PathBuf,
    pub save_maps: bool,
    pub save_pages: bool,
}

impl PipelineConfig {
    pub fn new(output_dir: PathBuf, omit_names: bool, renderer: Renderer, save_pages: bool, save_maps: bool) -> Self {
        let raw_csv_path = output_dir.join("pentacam_raw.csv");
        let detailed_csv_path = output_dir.join("pentacam_detailed.csv");
        let compact_csv_path = output_dir.join("pentacam_compact.csv");
        let processed_log_path = output_dir.join("processed_folders.csv");
        let error_log_path = output_dir.join("errors.log");
        PipelineConfig {
            output_dir,
            omit_patient_names: omit_names,
            renderer,
            raw_csv_path,
            detailed_csv_path,
            compact_csv_path,
            processed_log_path,
            error_log_path,
            save_maps,
            save_pages,
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

/// CSV input row — parsed from a structured CSV file list.
struct CsvInputRow {
    filename: String,
    patient_id: Option<String>,
    family_name: Option<String>,
    given_name: Option<String>,
    dob: Option<String>,
    exam_date: Option<String>,
    exam_time: Option<String>,
    laterality: Option<String>,
    printout_type_hint: Option<String>,
}

/// Metadata from CSV to pass to image processing, avoiding OCR demographics.
pub struct CsvMeta {
    pub original_filename: String,
    pub patient_id: Option<String>,
    pub family_name: Option<String>,
    pub given_name: Option<String>,
    pub dob: Option<String>,
    pub exam_date: Option<String>,
    pub exam_time: Option<String>,
    pub laterality: Option<String>,
}

impl CsvMeta {
    /// Check if we have enough metadata to skip demographics OCR.
    /// Requires at least: id + dob + laterality + exam_date + exam_time.
    fn has_core_demographics(&self) -> bool {
        self.patient_id.is_some()
            && self.dob.is_some()
            && self.laterality.is_some()
            && self.exam_date.is_some()
            && self.exam_time.is_some()
    }
}

/// Action to take for a CSV row based on its printout type hint.
enum CsvPrintoutAction {
    Process,
    Skip,
    DetectFromFile,
}

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
        if config.save_maps || config.save_pages {
            fs::create_dir_all(config.output_dir.join("images"))
                .map_err(|e| format!("Create images dir: {}", e))?;
        }

        // Open raw CSV writer
        let raw_csv = RawCsvWriter::open(&config.raw_csv_path, config.omit_patient_names)
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

    /// Process any input: single file, file list (.txt), CSV file list (.csv),
    /// or directory (PACS mode).
    pub fn process_input(&mut self, input: &Path) {
        if input.is_dir() {
            self.process_pacs_directory(input);
        } else if input.extension().and_then(|e| e.to_str()) == Some("csv") {
            self.process_csv_file_list(input);
        } else if input.extension().and_then(|e| e.to_str()) == Some("txt") {
            self.process_file_list(input);
        } else {
            self.process_single_file(input);
        }
    }

    /// Process a file list (.txt with one path per line).
    /// Tracks processed files for restart.
    pub fn process_file_list(&mut self, list_path: &Path) {
        let content = match fs::read_to_string(list_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("ERROR: Cannot read file list {}: {}", list_path.display(), e);
                return;
            }
        };

        let files: Vec<PathBuf> = content.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| PathBuf::from(l.trim()))
            .filter(|p| p.exists())
            .collect();

        eprintln!("File list: {} files from {}", files.len(), list_path.display());

        // Use processed_files tracking (separate from folder tracking)
        let processed_files_path = self.config.output_dir.join("processed_files.csv");
        let processed: HashSet<String> = if processed_files_path.exists() {
            fs::read_to_string(&processed_files_path).unwrap_or_default()
                .lines().filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string()).collect()
        } else {
            HashSet::new()
        };

        let mut processed_log = fs::OpenOptions::new()
            .create(true).append(true)
            .open(&processed_files_path)
            .expect("Cannot open processed_files.csv");

        let t_start = std::time::Instant::now();

        for (i, file_path) in files.iter().enumerate() {
            let file_str = file_path.display().to_string();
            if processed.contains(&file_str) {
                self.folders_skipped += 1; // reuse counter for "skipped"
                continue;
            }

            self.process_single_file(file_path);
            self.flush_pending_rows();
            self.write_pending_manifests();

            // Mark file as processed
            let _ = writeln!(processed_log, "{}", file_str);
            let _ = processed_log.flush();

            if (i + 1) % 10 == 0 || i + 1 == files.len() {
                eprintln!(
                    "[{}/{}] {:.1}s, {} rows (skipped {})",
                    i + 1, files.len(),
                    t_start.elapsed().as_secs_f64(),
                    self.total_rows, self.folders_skipped,
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // CSV file list mode
    // -----------------------------------------------------------------------

    /// Process a CSV file list with optional metadata columns.
    ///
    /// Expected CSV header (only `filename` is required):
    /// `filename,id,dob,examdate,examtime,laterality,printouttype`
    ///
    /// Pre-filters rows by printout type before opening files.
    /// For images, CSV metadata is used in place of OCR demographics when complete.
    pub fn process_csv_file_list(&mut self, csv_path: &Path) {
        let rows = match Self::parse_csv_file(csv_path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("ERROR: Cannot parse CSV {}: {}", csv_path.display(), e);
                return;
            }
        };

        let total = rows.len();
        eprintln!("CSV file list: {} rows from {}", total, csv_path.display());

        // Load processed-files log for restart support
        let processed_files_path = self.config.output_dir.join("processed_files.csv");
        let processed: HashSet<String> = if processed_files_path.exists() {
            fs::read_to_string(&processed_files_path).unwrap_or_default()
                .lines().filter(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string()).collect()
        } else {
            HashSet::new()
        };

        let mut processed_log = fs::OpenOptions::new()
            .create(true).append(true)
            .open(&processed_files_path)
            .expect("Cannot open processed_files.csv");

        let t_start = std::time::Instant::now();
        let mut csv_skipped: u32 = 0;

        for (i, row) in rows.into_iter().enumerate() {
            // Skip already-processed files (use original filename with ~)
            if processed.contains(&row.filename) {
                self.folders_skipped += 1;
                continue;
            }

            // Pre-filter by printout type — skip unsupported types without opening file
            match Self::csv_printout_filter(&row.printout_type_hint) {
                CsvPrintoutAction::Skip => {
                    csv_skipped += 1;
                    // Mark as processed so we don't re-check on restart
                    let _ = writeln!(processed_log, "{}", row.filename);
                    let _ = processed_log.flush();
                    continue;
                }
                CsvPrintoutAction::Process | CsvPrintoutAction::DetectFromFile => {}
            }

            // Resolve path: expand ~ and try _1 suffix
            let resolved = match Self::resolve_csv_path(&row.filename) {
                Some(p) => p,
                None => {
                    self.error_log.warn(
                        LogCategory::DicomError,
                        &row.filename,
                        "File not found (tried ~ expansion and _1 suffix)",
                    );
                    csv_skipped += 1;
                    let _ = writeln!(processed_log, "{}", row.filename);
                    let _ = processed_log.flush();
                    continue;
                }
            };

            // Dispatch by file extension
            let ext = resolved.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            let csv_meta = CsvMeta {
                original_filename: row.filename.clone(),
                patient_id: row.patient_id,
                family_name: row.family_name,
                given_name: row.given_name,
                dob: row.dob,
                exam_date: row.exam_date,
                exam_time: row.exam_time,
                laterality: row.laterality,
            };

            match ext.as_str() {
                "dcm" => {
                    // For DICOMs: ignore CSV metadata, use DICOM tags
                    self.process_dicom(&resolved);
                }
                "pdf" => {
                    self.process_pdf(&resolved);
                }
                "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" => {
                    self.process_image_with_csv(&resolved, Some(&csv_meta));
                }
                _ => {
                    // Try opening as image by magic bytes (handles .JPG_1 etc.)
                    if image::io::Reader::open(&resolved)
                        .and_then(|r| r.with_guessed_format())
                        .ok()
                        .and_then(|r| r.format())
                        .is_some()
                    {
                        self.process_image_with_csv(&resolved, Some(&csv_meta));
                    } else {
                        self.error_log.warn(
                            LogCategory::DicomError,
                            &row.filename,
                            &format!("Unsupported file type: .{}", ext),
                        );
                    }
                }
            }

            self.flush_pending_rows();
            self.write_pending_manifests();

            // Mark as processed using original filename (with ~)
            let _ = writeln!(processed_log, "{}", row.filename);
            let _ = processed_log.flush();

            if (i + 1) % 100 == 0 || i + 1 == total {
                eprintln!(
                    "[{}/{}] {:.1}s, {} rows (skipped {})",
                    i + 1, total,
                    t_start.elapsed().as_secs_f64(),
                    self.total_rows, csv_skipped,
                );
            }
        }
    }

    /// Parse a CSV file into CsvInputRow structs.
    /// Detects column indices from header; only `filename` is required.
    fn parse_csv_file(csv_path: &Path) -> Result<Vec<CsvInputRow>, String> {
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(true)
            .from_path(csv_path)
            .map_err(|e| format!("Open CSV: {}", e))?;

        // Find column indices by name
        let headers = rdr.headers().map_err(|e| format!("Read headers: {}", e))?.clone();
        let col = |name: &str| -> Option<usize> {
            headers.iter().position(|h| h == name)
        };

        let i_filename = col("filename")
            .ok_or_else(|| "Missing required column: filename".to_string())?;
        let i_id = col("id");
        let i_dob = col("dob");
        let i_examdate = col("examdate");
        let i_examtime = col("examtime");
        let i_laterality = col("laterality");
        let i_printouttype = col("printouttype");
        let i_family_name = col("family_name");
        let i_given_name = col("given_name");

        let get = |record: &csv::StringRecord, idx: Option<usize>| -> Option<String> {
            idx.and_then(|i| record.get(i))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };

        let mut rows = Vec::new();
        for result in rdr.records() {
            let record = match result {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("  WARNING: Skipping malformed CSV row: {}", e);
                    continue;
                }
            };

            let filename = match record.get(i_filename) {
                Some(f) if !f.trim().is_empty() => f.trim().to_string(),
                _ => continue,
            };

            rows.push(CsvInputRow {
                filename,
                patient_id: get(&record, i_id),
                family_name: get(&record, i_family_name),
                given_name: get(&record, i_given_name),
                dob: get(&record, i_dob).map(|d| normalize_csv_date(&d)),
                exam_date: get(&record, i_examdate).map(|d| normalize_csv_date(&d)),
                exam_time: get(&record, i_examtime).map(|t| normalize_csv_time(&t)),
                laterality: get(&record, i_laterality),
                printout_type_hint: get(&record, i_printouttype),
            });
        }

        Ok(rows)
    }

    /// Determine whether to process or skip a file based on CSV printout type.
    fn csv_printout_filter(hint: &Option<String>) -> CsvPrintoutAction {
        match hint.as_deref() {
            None | Some("") => CsvPrintoutAction::DetectFromFile,
            Some(s) => {
                // Use starts_with to handle noisy suffixes from PACS database
                if s.starts_with("4 Maps Refr") { CsvPrintoutAction::Process }
                else if s.starts_with("4 Maps Select") { CsvPrintoutAction::Process }
                else if s.contains("Enhanced Ectasia") { CsvPrintoutAction::Process }
                else if s.starts_with("Topometric") || s.starts_with("4 Maps Topo") { CsvPrintoutAction::Process }
                else { CsvPrintoutAction::Skip }
            }
        }
    }

    /// Resolve a CSV filename to an actual file path.
    /// Expands `~` to $HOME; tries appending `_1` suffix if not found.
    fn resolve_csv_path(raw: &str) -> Option<PathBuf> {
        let expanded = if raw.starts_with("~/") {
            if let Ok(home) = std::env::var("HOME") {
                PathBuf::from(format!("{}{}", home, &raw[1..]))
            } else {
                PathBuf::from(raw)
            }
        } else {
            PathBuf::from(raw)
        };

        if expanded.exists() {
            return Some(expanded);
        }

        // Try _1 suffix (Harmony PACS quirk)
        let suffixed = PathBuf::from(format!("{}_1", expanded.display()));
        if suffixed.exists() {
            return Some(suffixed);
        }

        None
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
                // Try opening as image by magic bytes (handles .JPG_1 etc.)
                if image::io::Reader::open(path)
                    .and_then(|r| r.with_guessed_format())
                    .ok()
                    .and_then(|r| r.format())
                    .is_some()
                {
                    self.process_image(path);
                } else {
                    self.error_log.warn(
                        LogCategory::DicomError,
                        &path.display().to_string(),
                        &format!("Unsupported file type: .{}", ext),
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // PACS directory mode
    // -----------------------------------------------------------------------

    /// Recursively scan a PACS directory, processing each subfolder incrementally.
    /// Skips already-processed folders without pre-scanning the entire tree.
    pub fn process_pacs_directory(&mut self, root: &Path) {
        eprintln!("Scanning PACS directory: {}", root.display());

        let t_start = std::time::Instant::now();

        // Walk incrementally — check each directory as we encounter it
        for entry in walkdir::WalkDir::new(root)
            .sort_by_file_name() // deterministic order, subfolders before parents
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_dir() { continue; }

            let folder = entry.path();
            let folder_str = folder.display().to_string();

            // Skip already processed
            if self.processed_log.is_processed(&folder_str) {
                self.folders_skipped += 1;
                continue;
            }

            // Check if this directory has Pentacam DICOM files (without recursing)
            let has_pentacam = fs::read_dir(folder)
                .into_iter()
                .flat_map(|rd| rd.into_iter())
                .filter_map(|e| e.ok())
                .any(|e| {
                    e.path().extension().and_then(|ext| ext.to_str()) == Some("dcm")
                        && e.file_name().to_str()
                            .map(|n| n.starts_with(PENTACAM_PREFIX))
                            .unwrap_or(false)
                });

            if !has_pentacam { continue; }

            let t0 = std::time::Instant::now();
            self.process_pacs_folder(folder);

            if let Err(e) = self.processed_log.mark_processed(&folder_str) {
                eprintln!("  WARNING: Failed to log processed folder: {}", e);
            }
            self.folders_processed += 1;

            if self.folders_processed % 10 == 0 {
                eprintln!(
                    "[{}] {:.1}s total, {} files, {} rows (skipped {})",
                    self.folders_processed,
                    t_start.elapsed().as_secs_f64(),
                    self.files_processed, self.total_rows, self.folders_skipped,
                );
            }
        }

        eprintln!(
            "Scan complete: {} folders processed, {} skipped",
            self.folders_processed, self.folders_skipped,
        );
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
                                // Run OCR ONCE for both map extraction and field extraction
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

                                // Field extraction using the SAME OCR items (no second OCR run)
                                let result_opt = if let Some(items) = ocr_items {
                                    ocr_import::process_page_with_items(
                                        &png_path, path, page as usize, items,
                                    )
                                } else {
                                    None
                                };
                                if let Some(result) = result_opt {
                                    // Only emit rows for supported printout types
                                    if Self::is_supported_printout(&result.printout_type) {
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
            family_name: String::new(),
            given_name: String::new(),
            dob: String::new(),
            
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
            imagedir: String::new(),
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
        self.process_image_with_csv(path, None);
    }

    fn process_image_with_csv(&mut self, path: &Path, csv_meta: Option<&CsvMeta>) {
        // Use original CSV filename for source_file if available
        let fname = csv_meta.as_ref()
            .map(|m| m.original_filename.as_str())
            .unwrap_or_else(|| path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown"));
        let folder = path.parent().map(|p| p.display().to_string()).unwrap_or_default();

        // Upscale low-resolution images to ~300 DPI before OCR.
        // A Pentacam printout at 300 DPI is ~3500px wide.
        // Use guessed format to handle non-standard extensions (.JPG_1 etc.)
        let decoded = image::io::Reader::open(path)
            .and_then(|r| r.with_guessed_format())
            .map_err(|e| image::ImageError::IoError(e))
            .and_then(|r| r.decode());

        let (upscaled_rgb, upscaled_tmp, page_img) = match decoded {
            Ok(img) => {
                let (w, _h) = img.dimensions();
                if w < 2800 {
                    // Upscale: target ~3500px wide
                    let scale = 3500.0 / w as f64;
                    let new_w = (w as f64 * scale) as u32;
                    let new_h = (img.height() as f64 * scale) as u32;
                    let resized = img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3);
                    eprintln!("  upscaled {}x{} → {}x{}", w, img.height(), new_w, new_h);
                    // Save to temp file for crop rescue (which loads regions from disk)
                    let tmp = ocr_import::temp_path("upscaled.png");
                    let _ = resized.save(&tmp);
                    (Some(resized.to_rgb8()), Some(tmp), Some(resized))
                } else {
                    (None, None, Some(img))
                }
            }
            Err(_) => (None, None, None),
        };

        // The file path crop rescue will use (upscaled temp or original)
        let effective_path = upscaled_tmp.as_deref().unwrap_or(path);

        // Run OCR ONCE: use in-memory path for upscaled, file path for original
        let ocr_items = if let Some(rgb) = upscaled_rgb {
            ocr_import::ocr_engine::run_full_page_mem(rgb).ok()
        } else {
            ocr_import::ocr_engine::run_full_page(effective_path).ok()
        };

        // Extract maps from the single OCR run
        let map_data = if let Some(ref items) = ocr_items {
            if let Some(ref pimg) = page_img {
                if let Some(pt) = ocr_import::printout_detect::detect_printout_type(items) {
                    let pt_str = format!("{:?}", pt);
                    let maps = ocr_import::extract_maps::extract_maps(pimg, items, &pt_str);
                    if !maps.maps.is_empty() { Some(maps) } else { None }
                } else { None }
            } else { None }
        } else { None };

        // Use the SAME OCR items for field extraction (no second OCR run)
        let result_opt = if let Some(items) = ocr_items {
            ocr_import::process_page_with_items(effective_path, path, 1, items)
        } else {
            None
        };

        if let Some(result) = result_opt {
            // Determine demographics: prefer CSV metadata when available,
            // fall back to OCR header extraction.
            let (patient_id, family_name, given_name, dob, eye, exam_date, exam_time) =
                if let Some(meta) = csv_meta.filter(|m| m.has_core_demographics()) {
                    // CSV provides core demographics — use them, skip OCR demographics
                    let fam = if self.config.omit_patient_names {
                        String::new()
                    } else {
                        meta.family_name.clone().unwrap_or_default()
                    };
                    let giv = if self.config.omit_patient_names {
                        String::new()
                    } else {
                        meta.given_name.clone().unwrap_or_default()
                    };
                    (
                        meta.patient_id.clone().unwrap_or_default(),
                        fam,
                        giv,
                        meta.dob.clone().unwrap_or_default(),
                        meta.laterality.clone().unwrap_or_default(),
                        meta.exam_date.clone().unwrap_or_default(),
                        meta.exam_time.clone().unwrap_or_default(),
                    )
                } else if let Some(meta) = csv_meta {
                    // CSV has partial metadata — use what we have, fill gaps from OCR
                    let ocr = result.demographics.as_ref();
                    let ocr_eye = ocr.and_then(|d| d.eye.as_ref()).map(|e| match e {
                        Laterality::OD => "OD".to_string(),
                        Laterality::OS => "OS".to_string(),
                    });
                    let (ocr_fam, ocr_giv) = ocr.and_then(|d| d.patient_name.as_ref())
                        .map(|name| {
                            let parts: Vec<&str> = name.splitn(2, '^').collect();
                            (parts.first().unwrap_or(&"").to_string(),
                             parts.get(1).unwrap_or(&"").to_string())
                        })
                        .unwrap_or_default();
                    let fam = if self.config.omit_patient_names { String::new() }
                              else { meta.family_name.clone().unwrap_or(ocr_fam) };
                    let giv = if self.config.omit_patient_names { String::new() }
                              else { meta.given_name.clone().unwrap_or(ocr_giv) };
                    (
                        meta.patient_id.clone()
                            .or_else(|| ocr.and_then(|d| d.patient_id.clone()))
                            .unwrap_or_default(),
                        fam,
                        giv,
                        meta.dob.clone()
                            .or_else(|| ocr.and_then(|d| d.date_of_birth.clone()))
                            .unwrap_or_default(),
                        meta.laterality.clone()
                            .or(ocr_eye)
                            .unwrap_or_default(),
                        meta.exam_date.clone()
                            .or_else(|| ocr.and_then(|d| d.exam_date.clone()))
                            .unwrap_or_default(),
                        meta.exam_time.clone()
                            .or_else(|| ocr.and_then(|d| d.exam_time.clone()))
                            .unwrap_or_default(),
                    )
                } else if let Some(ref demo) = result.demographics {
                    // No CSV metadata — use OCR demographics
                    let (fam, giv) = if let Some(ref name) = demo.patient_name {
                        let parts: Vec<&str> = name.splitn(2, '^').collect();
                        (
                            parts.first().unwrap_or(&"").to_string(),
                            parts.get(1).unwrap_or(&"").to_string(),
                        )
                    } else {
                        (String::new(), String::new())
                    };
                    let eye_str = match demo.eye {
                        Some(Laterality::OD) => "OD".to_string(),
                        Some(Laterality::OS) => "OS".to_string(),
                        _ => String::new(),
                    };
                    (
                        demo.patient_id.clone().unwrap_or_default(),
                        fam,
                        giv,
                        demo.date_of_birth.clone().unwrap_or_default(),
                        eye_str,
                        demo.exam_date.clone().unwrap_or_default(),
                        demo.exam_time.clone().unwrap_or_default(),
                    )
                } else {
                    (String::new(), String::new(), String::new(), String::new(),
                     String::new(), String::new(), String::new())
                };

            // Compute imagedir hash if we have enough metadata
            let imagedir = if !patient_id.is_empty() && !eye.is_empty() {
                let lat = if eye == "OD" { Laterality::OD } else { Laterality::OS };
                let epoch = parse_exam_epoch(&exam_date, &exam_time);
                let key = pentacam_types::EyeVisitKey {
                    patient_id: patient_id.clone(),
                    eye: lat,
                    exam_epoch_secs: epoch,
                };
                key.dir_hash()
            } else {
                String::new()
            };

            let pt_str = format!("{:?}", result.printout_type);

            // Save maps if we have an imagedir
            if !imagedir.is_empty() {
                if let Some(ref maps) = map_data {
                    self.save_maps(&imagedir, maps);
                }
            }

            let row = RawRow {
                patient_id,
                family_name,
                given_name,
                dob,
                eye,
                exam_date,
                exam_time,
                source_folder: folder,
                source_file: fname.to_string(),
                page_number: 1,
                printout_type: pt_str,
                qa_status: match &result.qa_status {
                    QaStatus::Ok => "ok".to_string(),
                    QaStatus::Incomplete { reason } => format!("incomplete: {}", reason),
                },
                n_fields: result.fields.len() as u32,
                device_serial: String::new(),
                software_version: String::new(),
                imagedir,
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
        if !row.imagedir.is_empty() {
            self.scan_sources
                .entry(row.imagedir.clone())
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
    /// Returns (path, is_new) — if the directory already existed, callers
    /// should skip saving images (first run wins).
    fn image_dir(&self, scan_hash: &str) -> (PathBuf, bool) {
        let dir = self.config.output_dir.join("images").join(scan_hash);
        let is_new = !dir.exists();
        let _ = fs::create_dir_all(&dir);
        (dir, is_new)
    }

    /// Save a rendered page image (de-identified) to the scan's image directory.
    fn save_page_image(
        &self,
        scan_hash: &str,
        page_num: u32,
        printout_type: &str,
        img_path: &Path,
    ) {
        if !self.config.save_pages || scan_hash.is_empty() { return; }
        let (dir, _is_new) = self.image_dir(scan_hash);
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
        if !dst.exists() { // per-file first-run-wins
            let _ = fs::copy(img_path, &dst);
        }
    }

    /// Save extracted map images to the scan's image directory.
    fn save_maps(
        &self,
        scan_hash: &str,
        maps: &ocr_import::extract_maps::ExtractedMaps,
    ) {
        if !self.config.save_maps || scan_hash.is_empty() { return; }
        let (dir, _is_new) = self.image_dir(scan_hash);
        for (name, img) in &maps.maps {
            let dst = dir.join(format!("map_{}.png", name));
            if !dst.exists() { // per-file first-run-wins
                let _ = img.save(&dst);
            }
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
        // Split DICOM PatientName (FamilyName^GivenName^...) into parts
        let full_name = meta.patient_name.clone().unwrap_or_default();
        let name_parts: Vec<&str> = full_name.split('^').collect();
        let (family_name, given_name) = if self.config.omit_patient_names {
            (String::new(), String::new())
        } else {
            (
                name_parts.first().unwrap_or(&"").to_string(),
                name_parts.get(1).unwrap_or(&"").to_string(),
            )
        };

        RawRow {
            patient_id: meta.patient_id.clone().unwrap_or_default(),
            family_name,
            given_name,
            dob: meta.date_of_birth.clone().unwrap_or_default(),
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
            imagedir: scan_hash.to_string(),
            fields: HashMap::new(),
            confidences: HashMap::new(),
        }
    }

    /// Write source manifests for accumulated scans, then clear the buffer.
    fn write_pending_manifests(&mut self) {
        if !self.config.save_maps && !self.config.save_pages { return; }
        for (hash, entries) in self.scan_sources.drain() {
            if entries.is_empty() { continue; }
            let dir = self.config.output_dir.join("images").join(&hash);
            // Skip if manifest already exists (first run wins)
            if dir.join("sources.csv").exists() { continue; }
            let _ = fs::create_dir_all(&dir);
            if let Err(e) = write_source_manifest(&dir, &entries) {
                eprintln!("  WARNING: Failed to write source manifest: {}", e);
            }
        }
    }

    /// Check if a printout type is supported for extraction.
    fn is_supported_printout(pt: &PrintoutType) -> bool {
        match pt {
            PrintoutType::FourMapsRefractive
            | PrintoutType::FourMapsSelectable
            | PrintoutType::TopometricKcStaging
            | PrintoutType::BelinAmbrosio => true,
            PrintoutType::Other(s) if s == "Refractive (gen1)" => true,
            _ => false,
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

/// Normalize a date from CSV to YYYYMMDD.
/// Accepts: YYYY-MM-DD, MM/DD/YYYY, YYYYMMDD.
fn normalize_csv_date(s: &str) -> String {
    let s = s.trim();

    // YYYY-MM-DD
    if s.len() == 10 && s.chars().nth(4) == Some('-') {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() == 3 {
            if let (Ok(y), Ok(m), Ok(d)) = (
                parts[0].parse::<u32>(),
                parts[1].parse::<u32>(),
                parts[2].parse::<u32>(),
            ) {
                return format!("{:04}{:02}{:02}", y, m, d);
            }
        }
    }

    // MM/DD/YYYY
    if s.contains('/') {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 3 {
            if let (Ok(m), Ok(d), Ok(y)) = (
                parts[0].parse::<u32>(),
                parts[1].parse::<u32>(),
                parts[2].parse::<u32>(),
            ) {
                if y > 1900 {
                    return format!("{:04}{:02}{:02}", y, m, d);
                }
            }
        }
    }

    // Already YYYYMMDD
    if s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()) {
        return s.to_string();
    }

    s.to_string()
}

/// Normalize a time string to HHMMSS. Removes colons/dots.
fn normalize_csv_time(s: &str) -> String {
    let s = s.trim();
    // Already HHMMSS (6 digits)
    if s.len() == 6 && s.chars().all(|c| c.is_ascii_digit()) {
        return s.to_string();
    }
    // HH:MM:SS → HHMMSS
    s.replace(':', "").replace('.', "")
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