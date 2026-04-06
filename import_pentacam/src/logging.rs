//! Structured error and warning logging.
//!
//! One-line-per-event format for grep-ability. Summary counts at end.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

/// Log event categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogCategory {
    OcrFailure,
    UnrecognizedPrintout,
    SkippedPage,
    RenderFailure,
    DicomError,
    SrOcrMismatch,
    LowConfidence,
    QaIncomplete,
}

impl LogCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogCategory::OcrFailure => "OCR_FAILURE",
            LogCategory::UnrecognizedPrintout => "UNRECOGNIZED_PRINTOUT",
            LogCategory::SkippedPage => "SKIPPED_PAGE",
            LogCategory::RenderFailure => "RENDER_FAILURE",
            LogCategory::DicomError => "DICOM_ERROR",
            LogCategory::SrOcrMismatch => "SR_OCR_MISMATCH",
            LogCategory::LowConfidence => "LOW_CONFIDENCE",
            LogCategory::QaIncomplete => "QA_INCOMPLETE",
        }
    }
}

/// Structured log writer.
pub struct ErrorLog {
    writer: Option<BufWriter<File>>,
    counts: HashMap<LogCategory, u32>,
}

impl ErrorLog {
    /// Open the error log file. If path is None, logs only to stderr.
    pub fn open(path: Option<&Path>) -> std::io::Result<Self> {
        let writer = match path {
            Some(p) => {
                let file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)?;
                Some(BufWriter::new(file))
            }
            None => None,
        };
        Ok(ErrorLog {
            writer,
            counts: HashMap::new(),
        })
    }

    /// Log a warning event.
    pub fn warn(&mut self, category: LogCategory, file: &str, detail: &str) {
        *self.counts.entry(category).or_insert(0) += 1;
        let line = format!("[{}] {} — {}", category.as_str(), file, detail);
        eprintln!("  WARNING: {}", line);
        if let Some(ref mut w) = self.writer {
            let _ = writeln!(w, "{}", line);
        }
    }

    /// Flush the log file.
    pub fn flush(&mut self) {
        if let Some(ref mut w) = self.writer {
            let _ = w.flush();
        }
    }

    /// Print summary of all logged events to stderr.
    pub fn print_summary(&self) {
        if self.counts.is_empty() {
            eprintln!("No warnings or errors logged.");
            return;
        }
        eprintln!("\nWarning/error summary:");
        let mut sorted: Vec<_> = self.counts.iter().collect();
        sorted.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        for (cat, count) in sorted {
            eprintln!("  {}: {}", cat.as_str(), count);
        }
    }

    /// Total number of logged events.
    pub fn total_events(&self) -> u32 {
        self.counts.values().sum()
    }
}
