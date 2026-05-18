# import_pentacam

Extract clinical measurements from Pentacam ophthalmic imaging files into CSV.

Reads DICOM (including Structured Reports), proprietary SPR binary blobs, PDF
printouts, and image printouts (JPEG / PNG / TIFF, including low-resolution
PACS exports). Output is compatible with `import_spectralis` conventions.

## Install (Windows)

A prebuilt MSI installer is available for direct download:

  <https://my.hidrive.com/lnk/X6xcOOowV>

Install by double-clicking, or silently:

```
msiexec /i import_pentacam-0.1.0-win-x64.msi /quiet
```

The installer places files under `C:\Program Files\import_pentacam\`, adds
that directory to the system `PATH`, and registers in Add/Remove Programs.

## Build from source (Linux or Windows)

See [INSTALL](INSTALL) for prerequisites and full instructions. Quick version:

```sh
./configure              # add --cpu-only on machines without an NVIDIA GPU
make install             # Linux. On Windows, follow the cargo / cargo-wix
                         # steps in INSTALL if your shell lacks `make`.
```

`./configure` automatically downloads ONNX Runtime, the PaddleOCR v5 models,
and (on Windows) `libclang.dll` and the WiX Toolset. No admin privileges
required.

OCR uses PaddleOCR v5 via ONNX Runtime. CPU works out of the box; an NVIDIA
GPU with CUDA drivers gives a substantial speedup.

## Usage

```sh
import_pentacam --help              # full reference
import_pentacam SCAN.dcm  -o out    # single file
import_pentacam scans/    -o out    # directory (recursive)
import_pentacam list.csv  -o out    # CSV of file paths with optional metadata
```

See `--help` for the full list of supported printout types and CLI options.

## Outputs

Written to the output directory:

- `pentacam_raw.csv` — one row per measurement, long format
- `pentacam_compact.csv` — one row per scan, columns by field
- `pentacam_detailed.csv` — extended fields (topometric, Belin/Ambrósio, pachymetry)
- `images/` — extracted map crops from printouts

## License

[AGPL-3.0-or-later](LICENSE). This project links to MuPDF, which is itself
AGPL-licensed, so all distribution must include source access.

## Citation

If you use `import_pentacam` in academic work, please cite:

```bibtex
@software{elze_import_pentacam,
  author = {Tobias Elze},
  title  = {import_pentacam: Clinical data extraction from Pentacam DICOM, PDF, and image files},
  year   = {2026},
  url    = {https://github.com/tobiaselze/import_pentacam},
}
```

## Contact

Tobias Elze — <tobias-elze@tobias-elze.de>
