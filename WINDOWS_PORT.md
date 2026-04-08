# Windows Port Roadmap

This document contains everything needed to port `import_pentacam` to Windows.
It is written for a fresh development environment (or AI assistant) that has
not seen the Linux development history.

## Project Overview

`import_pentacam` extracts clinical measurements from Pentacam ophthalmic
DICOM, PDF, and image files using OCR. It produces CSV output compatible with
`import_spectralis`. It runs on Linux and needs to be ported to Windows.

**Repo**: https://github.com/tobiaselze/import_pentacam (private)

Run `import_pentacam --help` for full documentation of all features and options.

## Architecture

5-crate Rust workspace:

```
pentacam_types/     Shared types (EyeVisitKey, PrintoutResult, etc.)
dicom_import/       DICOM tag + SR parsing, PDF/blob extraction
blob_import/        SPR proprietary binary readout
ocr_import/         OCR pipeline (PaddleOCR v5 via ONNX Runtime)
import_pentacam/    Production binary with CLI
```

Key dependencies:
- `oar-ocr` 0.6 — PaddleOCR wrapper using ONNX Runtime for inference
- `mupdf` 0.6 — MuPDF wrapper for PDF rendering (C library)
- `image` 0.25 — Image loading, resizing (pure Rust)
- `clap` — CLI argument parsing
- `chrono` — Date/time handling
- `blake3` — Hashing for image directory names
- `walkdir` — Directory traversal

## Current Linux Build System

```bash
./configure              # Downloads ORT + OCR models, detects CUDA, writes config.mk
make                     # Builds release binary
make dist                # Assembles self-contained dist/ folder
make deb                 # Creates .deb package
make install             # Installs to ~/.local
```

The binary uses RUNPATH=$ORIGIN to find .so files next to itself. On Windows,
DLLs next to .exe are found automatically — this is actually easier.

## What Needs to Change for Windows

### 1. Temp File Paths (EASY — do this first)

**File**: `ocr_import/src/lib.rs`

Currently hardcodes Linux temp path:
```rust
lazy_static! {
    static ref TEMP_DIR: PathBuf = {
        let dir = PathBuf::from(format!("/tmp/pentacam_ocr_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        dir
    };
}
```

**Fix**: Use `std::env::temp_dir()`:
```rust
let dir = std::env::temp_dir().join(format!("pentacam_ocr_{}", std::process::id()));
```

This returns `C:\Users\<user>\AppData\Local\Temp` on Windows, `/tmp` on Linux.

Also check `ocr_import/src/render.rs` for any hardcoded `/tmp` paths.

### 2. MuPDF Compilation (HARD — main blocker)

The `mupdf` crate (version 0.6) wraps the MuPDF C library. It uses a build
script that compiles MuPDF from source. On Windows, this requires:

- MSVC (Visual Studio Build Tools) with C/C++ workload
- Or MinGW-w64 GCC

The mupdf crate's build.rs should handle Windows, but may need:
- CMake installed and in PATH
- Correct MSVC environment (run from "Developer Command Prompt" or set up vcvars)

**If MuPDF compilation fails**, we have a fallback: the `--poppler` flag uses
`pdftoppm` (Poppler) for PDF rendering via subprocess. Poppler is available
for Windows. As a last resort, we could make MuPDF optional:
```toml
[dependencies]
mupdf = { version = "0.6", optional = true }

[features]
default = ["mupdf"]
```

But most input files are DICOMs (which embed PDFs) or images, so PDF rendering
is needed for the core workflow.

### 3. ONNX Runtime for Windows

Download from: https://github.com/microsoft/onnxruntime/releases

- CPU: `onnxruntime-win-x64-1.20.1.zip`
- GPU: `onnxruntime-win-x64-gpu-1.20.1.zip`

The build needs:
```
ORT_LIB_LOCATION=C:\path\to\onnxruntime\lib
ORT_PREFER_DYNAMIC_LINK=1
```

For distribution, place next to .exe:
```
import_pentacam.exe
onnxruntime.dll
onnxruntime_providers_cuda.dll      (GPU only)
onnxruntime_providers_shared.dll    (GPU only)
models/
  pp-ocrv5_server_det.onnx
  en_pp-ocrv5_mobile_rec.onnx
  en_ppocrv5_dict.txt
```

Windows finds DLLs next to the .exe automatically — no RUNPATH equivalent needed.

### 4. Path Handling

**Tilde expansion** (`~/`): Used in CSV input mode. On Windows, `~` isn't a
shell concept. The code in `pipeline.rs` `resolve_csv_path()` expands it via
`$HOME` env var. On Windows, use `USERPROFILE` instead:

```rust
let expanded = if raw.starts_with("~/") {
    if let Ok(home) = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
    {
        PathBuf::from(format!("{}{}", home, &raw[1..]))
    } else {
        PathBuf::from(raw)
    }
} else {
    PathBuf::from(raw)
};
```

**PACS directory prefix**: Linux uses `1.3.6.1.4.1.34714.` to identify
Pentacam DICOM files by filename. This should work on Windows too (it's just
a string prefix check on filenames).

**Backslashes in CSV**: The CSV `filename` column may contain Windows paths
with backslashes. The `Path` type handles this, but string matching for
`processed_files.csv` restart needs to be consistent.

### 5. Build Script / Installer for Windows

The `./configure` bash script won't run natively on Windows (needs Git Bash).
Options:

a. **Git Bash** (recommended for developers): Already documented in INSTALL.
   `./configure` and `make` work in Git Bash.

b. **PowerShell script**: `configure.ps1` equivalent for native Windows builds.
   Lower priority — Git Bash is sufficient.

c. **MSI installer**: For end-user distribution. Can be created with
   [WiX Toolset](https://wixtoolset.org/) or cargo-wix. Would install to
   `C:\Program Files\import_pentacam\` with PATH entry.

d. **Self-extracting exe**: Bundle everything with `include_bytes!` in a Rust
   installer binary. Cross-platform approach.

For the first Windows build, just use Git Bash + make. Installer later.

### 6. Poppler on Windows

If `--poppler` is used, the binary calls `pdftoppm` via subprocess. On Windows,
Poppler binaries are available from:
https://github.com/oschwartz10612/poppler-windows/releases

The user would need to install it and add to PATH. Since MuPDF is the default
renderer, this is only needed as a fallback.

### 7. CUDA on Windows

ONNX Runtime CUDA provider works on Windows with:
- NVIDIA GPU driver installed
- CUDA Toolkit (matching the ORT version's CUDA requirement)
- cuDNN

The ORT Windows GPU package bundles its own CUDA runtime, so typically just
the NVIDIA driver is needed. Same as Linux — GPU is optional, CPU fallback
works everywhere.

## Step-by-Step Build Instructions for Windows

### Prerequisites

1. Install [Rust](https://rustup.rs) (use MSVC toolchain, the default on Windows)
2. Install [Git for Windows](https://git-scm.com) (includes Git Bash)
3. Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/)
   with "Desktop development with C++" workload (needed for MuPDF)
4. Install [CMake](https://cmake.org/download/) and add to PATH

### Build

Open Git Bash:
```bash
git clone git@github.com:tobiaselze/import_pentacam.git
cd import_pentacam
./configure --cpu-only    # Start with CPU-only (simpler)
make dist
```

If MuPDF fails to compile, try:
1. Run from "Developer Command Prompt for VS" instead of plain Git Bash
2. Ensure CMake is in PATH: `cmake --version`
3. Check that `cl.exe` (MSVC compiler) is accessible

### Test
```
cd dist/import_pentacam
./import_pentacam.exe --help
```

## Files Most Likely to Need Changes

| File | What to change |
|------|---------------|
| `ocr_import/src/lib.rs` | Temp dir path (use `std::env::temp_dir()`) |
| `ocr_import/src/render.rs` | PDF rendering temp files, subprocess calls |
| `import_pentacam/src/pipeline.rs` | Tilde expansion (add USERPROFILE fallback) |
| `import_pentacam/src/pipeline.rs` | Path separator handling in CSV restart log |
| `Makefile` | Windows dist target (.exe, .dll instead of .so) |
| `configure` | Windows ORT download URL (zip instead of tgz) |
| `ocr_import/Cargo.toml` | Possibly make mupdf optional if it won't compile |

## Files That Should Work As-Is

| File | Why |
|------|-----|
| `import_pentacam/src/main.rs` | Pure Rust, clap is cross-platform |
| `import_pentacam/src/compact_csv.rs` | Pure Rust string processing |
| `import_pentacam/src/raw_csv.rs` | Pure Rust I/O |
| `import_pentacam/src/field_map.rs` | Constants only |
| `import_pentacam/src/logging.rs` | Pure Rust I/O |
| `ocr_import/src/belin.rs` | Pure Rust math |
| `ocr_import/src/field_locate.rs` | Pure Rust math |
| `ocr_import/src/label_match.rs` | Pure Rust |
| `ocr_import/src/extract_maps.rs` | Uses image crate (cross-platform) |
| `ocr_import/src/demographics.rs` | Pure Rust string matching |
| `ocr_import/src/printout_detect.rs` | Pure Rust string matching |
| `pentacam_types/` | Pure Rust types |
| `dicom_import/` | Uses dicom crate (cross-platform) |
| `blob_import/` | Pure Rust binary parsing |

## Testing on Windows

1. **Single image**: Find any Pentacam JPEG, run:
   ```
   import_pentacam.exe image.jpg -o output -x
   ```

2. **File list**: Create a .txt with one path per line, run:
   ```
   import_pentacam.exe files.txt -o output -x
   ```

3. **DICOM**: If you have a .dcm file:
   ```
   import_pentacam.exe scan.dcm -o output -x
   ```

4. **CSV input**: The main use case — a CSV with 221k files from the PACS.
   Test with a small subset first.

Expected output: `pentacam_raw.csv`, `pentacam_compact.csv`,
`pentacam_detailed.csv`, `images/` directory with extracted maps.

## Known Issues to Watch For

- **Line endings**: Git might convert LF to CRLF on Windows. The CSV parser
  should handle both (it uses `.trim()` on lines).
- **Long paths**: Windows has a 260-char path limit by default. PACS paths
  can be long. May need to enable long paths in Windows registry or use `\\?\`
  prefix.
- **File locking**: Windows locks files more aggressively than Linux. The
  incremental CSV writer opens files in append mode — should be fine, but
  watch for "file in use" errors if the output CSV is open in Excel.
- **Console encoding**: OCR model paths with non-ASCII characters. Use UTF-8
  everywhere (Rust default).

## After the Port Works

1. Add Windows dist target to Makefile (copies .exe + .dll + models)
2. Create MSI or self-extracting installer for end users
3. Set up GitHub Actions CI for automated Windows builds
4. Test with PACS data accessible from Windows (SAMBA mount or local copy)
