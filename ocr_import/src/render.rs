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

    // Render at 2x DPI then downscale with Lanczos — supersampling preserves
    // small features (decimal points, minus signs) that MuPDF's renderer
    // otherwise renders too thin for OCR to detect.
    let render_dpi = dpi * 2;
    let scale = render_dpi as f32 / 72.0;
    let ctm = Matrix::new_scale(scale, scale);
    let cs = Colorspace::device_rgb();
    let pixmap = pg.to_pixmap(&ctm, &cs, false, false)
        .map_err(|e| format!("MuPDF render: {}", e))?;

    // Convert MuPDF pixmap to image::RgbImage for downscaling
    let w = pixmap.width();
    let h = pixmap.height();
    let samples = pixmap.samples();
    let n_channels = pixmap.n() as u32;

    let rgb_img = if n_channels >= 3 {
        let mut img = image::RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) as usize * n_channels as usize;
                img.put_pixel(x, y, image::Rgb([
                    samples[idx], samples[idx + 1], samples[idx + 2]
                ]));
            }
        }
        image::DynamicImage::ImageRgb8(img)
    } else {
        return Err("Unexpected pixel format".to_string());
    };

    // Downscale to target DPI with Lanczos
    let target_w = w / 2;
    let target_h = h / 2;
    let downscaled = rgb_img.resize_exact(target_w, target_h, image::imageops::FilterType::Lanczos3);

    let out_path = PathBuf::from(format!("/tmp/_mupdf_render_p{}.png", page));
    downscaled.save(&out_path).map_err(|e| format!("Save PNG: {}", e))?;

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
