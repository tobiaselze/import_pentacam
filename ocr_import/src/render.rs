//! PDF page rendering — MuPDF (in-process) or Poppler (subprocess).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Renderer {
    MuPdf,
    Poppler,
}

/// Render a PDF page to a PNG file at the given DPI.
/// Returns the path to the rendered PNG.
pub fn render_pdf_page(
    pdf_bytes: &[u8],
    page: u32,      // 1-based
    dpi: u32,
    renderer: Renderer,
) -> Result<PathBuf, String> {
    match renderer {
        Renderer::MuPdf => render_mupdf(pdf_bytes, page, dpi),
        Renderer::Poppler => render_poppler(pdf_bytes, page, dpi),
    }
}

/// Get page count from PDF bytes.
pub fn page_count(pdf_bytes: &[u8], renderer: Renderer) -> Result<u32, String> {
    match renderer {
        Renderer::MuPdf => page_count_mupdf(pdf_bytes),
        Renderer::Poppler => page_count_poppler(pdf_bytes),
    }
}

// ---------------------------------------------------------------------------
// MuPDF renderer
// ---------------------------------------------------------------------------

fn render_mupdf(pdf_bytes: &[u8], page: u32, dpi: u32) -> Result<PathBuf, String> {
    use mupdf::{Colorspace, Document, Matrix};

    let doc = Document::from_bytes(pdf_bytes, "application/pdf")
        .map_err(|e| format!("MuPDF open: {}", e))?;

    let pg = doc.load_page((page - 1) as i32)
        .map_err(|e| format!("MuPDF load page {}: {}", page, e))?;

    let scale = dpi as f32 / 72.0;
    let ctm = Matrix::new_scale(scale, scale);
    let cs = Colorspace::device_rgb();
    let pixmap = pg.to_pixmap(&ctm, &cs, false, false)
        .map_err(|e| format!("MuPDF render: {}", e))?;

    let out_path = PathBuf::from(format!("/tmp/_mupdf_render_p{}.png", page));
    pixmap.save_as(&out_path.to_str().unwrap(), mupdf::pixmap::ImageFormat::PNG)
        .map_err(|e| format!("MuPDF save PNG: {}", e))?;

    Ok(out_path)
}

fn page_count_mupdf(pdf_bytes: &[u8]) -> Result<u32, String> {
    use mupdf::Document;
    let doc = Document::from_bytes(pdf_bytes, "application/pdf")
        .map_err(|e| format!("MuPDF open: {}", e))?;
    doc.page_count()
        .map(|n| n as u32)
        .map_err(|e| format!("MuPDF page count: {}", e))
}

// ---------------------------------------------------------------------------
// Poppler renderer (pdftoppm subprocess)
// ---------------------------------------------------------------------------

fn render_poppler(pdf_bytes: &[u8], page: u32, dpi: u32) -> Result<PathBuf, String> {
    let pdf_path = PathBuf::from(format!("/tmp/_poppler_render_p{}.pdf", page));
    fs::write(&pdf_path, pdf_bytes).map_err(|e| format!("Write PDF: {}", e))?;

    let out_prefix = format!("/tmp/_poppler_render_p{}", page);
    // Clean old files
    if let Ok(entries) = glob::glob(&format!("{}*.png", out_prefix)) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry);
        }
    }

    let status = Command::new("pdftoppm")
        .args([
            "-r", &dpi.to_string(), "-png",
            "-f", &page.to_string(), "-l", &page.to_string(),
            pdf_path.to_str().unwrap(), &out_prefix,
        ])
        .status()
        .map_err(|e| format!("pdftoppm: {}", e))?;

    if !status.success() {
        return Err("pdftoppm failed".to_string());
    }

    let mut pages: Vec<PathBuf> = glob::glob(&format!("{}*.png", out_prefix))
        .map_err(|e| format!("glob: {}", e))?
        .filter_map(|p| p.ok())
        .collect();
    pages.sort();

    pages.first().cloned().ok_or_else(|| "pdftoppm produced no output".to_string())
}

fn page_count_poppler(pdf_bytes: &[u8]) -> Result<u32, String> {
    let pdf_path = PathBuf::from("/tmp/_poppler_pagecount.pdf");
    fs::write(&pdf_path, pdf_bytes).map_err(|e| format!("Write PDF: {}", e))?;

    let output = Command::new("pdfinfo")
        .arg(&pdf_path)
        .output()
        .map_err(|e| format!("pdfinfo: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.starts_with("Pages:") {
            if let Some(n) = line.split_whitespace().last() {
                return n.parse().map_err(|e| format!("parse pages: {}", e));
            }
        }
    }
    Ok(1)
}
