# Windows Port Roadmap

This document contains everything needed to port `import_pentacam` to Windows.
It is written for a fresh development environment (or AI assistant) that has
not seen the Linux development history.

## Port Status: COMPLETE (2026-04-08)

The Windows port has been completed. The binary compiles and runs on Windows
x86_64 with MSVC. All changes are backward-compatible with Linux.

### What Was Done

| Item | Status | Details |
|------|--------|---------|
| Temp file paths | Already done | `ocr_import/src/lib.rs` already used `std::env::temp_dir()` |
| MuPDF compilation | FIXED | Patched `max_align_t` missing from MSVC bindgen output (see `patches/mupdf-0.6.0/`) |
| ONNX Runtime | FIXED | `configure` downloads win-x64 .zip on Windows |
| Tilde expansion | FIXED | `pipeline.rs` now falls back to `USERPROFILE` if `HOME` is unset |
| Build system | FIXED | `configure` detects Windows, LLVM, MSVC; `Makefile` handles .exe/.dll |
| LIBCLANG_PATH | FIXED | `configure` auto-detects or downloads LLVM, writes `LIBCLANG_PATH` to `config.mk` |
| Path handling | NOT NEEDED | `render.rs` uses `temp_path()`, processed_files.csv uses consistent string keys |
| Poppler on Windows | NOT NEEDED | MuPDF is default renderer; Poppler subprocess calls work if installed |
| MSI installer | DONE | `make msi` via cargo-wix + WiX v3 (both auto-downloaded by `configure`) |

### Prerequisites for Windows Build

1. Rust (MSVC toolchain, the default on Windows) — https://rustup.rs
2. Git for Windows (includes Git Bash) — https://git-scm.com
3. Visual Studio Build Tools with "Desktop development with C++" workload
4. LLVM (provides libclang.dll for bindgen) — `./configure` will auto-download
   libclang.dll into a local `llvm/` directory if not found. No admin required.
   Or install LLVM system-wide from https://github.com/llvm/llvm-project/releases.

### Build Commands (Git Bash)

```bash
./configure --cpu-only    # or omit --cpu-only for GPU support
make dist                 # requires make; see INSTALL for manual alternative
```

### Output

```
dist/import_pentacam/
  import_pentacam.exe
  onnxruntime.dll
  models/
    pp-ocrv5_server_det.onnx
    en_pp-ocrv5_mobile_rec.onnx
    en_ppocrv5_dict.txt
```

---

## Project Overview

`import_pentacam` extracts clinical measurements from Pentacam ophthalmic
DICOM, PDF, and image files using OCR. It produces CSV output compatible with
`import_spectralis`. It runs on Linux and Windows.

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
- `mupdf` 0.6 — MuPDF wrapper for PDF rendering (C library, patched for Windows)
- `image` 0.25 — Image loading, resizing (pure Rust)
- `clap` — CLI argument parsing
- `chrono` — Date/time handling
- `blake3` — Hashing for image directory names
- `walkdir` — Directory traversal

## Current Build System

```bash
./configure              # Downloads ORT + OCR models, detects CUDA, writes config.mk
make                     # Builds release binary
make dist                # Assembles self-contained dist/ folder
make dist-tar            # Creates .tar.gz (Linux) or .zip (Windows) archive
make deb                 # Creates .deb package (Linux only)
make install             # Installs to ~/.local (Linux) or PREFIX (Windows)
```

The binary uses RUNPATH=$ORIGIN on Linux to find .so files next to itself.
On Windows, DLLs next to .exe are found automatically.

## Changes Made for Windows

### 1. MuPDF max_align_t Patch

**Files**: `patches/mupdf-0.6.0/src/device/native.rs`, `Cargo.toml`

The `mupdf` crate v0.6 uses `max_align_t` from bindgen-generated FFI
bindings. On MSVC, bindgen does not emit this type because MSVC headers
handle `max_align_t` differently (it's a compiler intrinsic, not a typedef).

**Fix**: Added a `#[cfg(windows)]` stand-in struct with `f64` alignment (8
bytes, matching x86_64 MSVC max fundamental alignment). Applied via
`[patch.crates-io]` in the workspace `Cargo.toml`. The `#[cfg]` gate ensures
zero impact on Linux builds.

### 2. Configure Script — Windows Detection

**File**: `configure`

- Detects MINGW/MSYS/CYGWIN as Windows
- Downloads `onnxruntime-win-x64-*.zip` instead of `-linux-*.tgz`
- Uses `unzip` instead of `tar xz` for extraction
- Detects MSVC (`cl.exe`), CMake, and LLVM (`libclang.dll`)
- Auto-downloads `libclang.dll` (~96 MB) from LLVM tar.xz release if not found
  (extracted to local `llvm/` directory, no admin required)
- Writes `LIBCLANG_PATH` to `config.mk` for bindgen

### 3. Makefile — Cross-Platform dist/install

**File**: `Makefile`

- Detects Windows via `uname -s` pattern matching
- Copies `.exe` + `.dll` on Windows instead of ELF + `.so` + symlinks
- `dist-tar` creates `.zip` on Windows
- `install` uses flat directory layout on Windows (no wrapper script)
- `deb` target errors on Windows with clear message

### 4. Tilde Expansion — USERPROFILE Fallback

**File**: `import_pentacam/src/pipeline.rs`

`resolve_csv_path()` now tries `USERPROFILE` if `HOME` is unset. On Linux,
`HOME` is always set so the fallback never triggers.

## Files That Needed Changes

| File | What changed |
|------|-------------|
| `configure` | Windows platform detection, ORT download, LLVM auto-download |
| `Makefile` | Windows dist/install targets, .exe/.dll handling |
| `Cargo.toml` | `[patch.crates-io]` for mupdf Windows fix |
| `patches/mupdf-0.6.0/src/device/native.rs` | `max_align_t` stand-in for MSVC |
| `import_pentacam/src/pipeline.rs` | `USERPROFILE` fallback for tilde expansion |

## Files That Worked As-Is

| File | Why |
|------|-----|
| `import_pentacam/src/main.rs` | Pure Rust, clap is cross-platform |
| `import_pentacam/src/compact_csv.rs` | Pure Rust string processing |
| `import_pentacam/src/raw_csv.rs` | Pure Rust I/O |
| `import_pentacam/src/field_map.rs` | Constants only |
| `import_pentacam/src/logging.rs` | Pure Rust I/O |
| `ocr_import/src/lib.rs` | Already used `std::env::temp_dir()` |
| `ocr_import/src/render.rs` | Uses `temp_path()`, subprocess calls are cross-platform |
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
- **make not in Git Bash**: Git Bash doesn't include `make` by default.
  Install via MSYS2 (`pacman -S make`) or use the manual build steps in INSTALL.

## MSI Installer

An MSI installer is built via `make msi` (requires `make dist` first).

**How it works**:
- `configure` auto-downloads WiX Toolset v3 portable binaries (~39 MB) to `wix/`
- `configure` installs `cargo-wix` via `cargo install`
- `cargo wix` compiles `import_pentacam/wix/main.wxs` into an MSI
- The MSI sources files from `dist/import_pentacam/` (built by `make dist`)

**What the MSI installs**:
- `C:\Program Files\import_pentacam\import_pentacam.exe`
- `C:\Program Files\import_pentacam\onnxruntime.dll`
- `C:\Program Files\import_pentacam\models\` (3 OCR model files)
- Adds install directory to system PATH
- Registers in "Add/Remove Programs"

**Silent install**: `msiexec /i import_pentacam-*.msi /quiet`

**WiX template**: `import_pentacam/wix/main.wxs` — edit to add GPU DLLs,
icons, or additional features.

## Future Improvements

1. GitHub Actions CI for automated Windows builds
2. Test with PACS data accessible from Windows (SAMBA mount or local copy)
3. Add GPU DLL components to MSI (conditional on CUDA_PROVIDER)
