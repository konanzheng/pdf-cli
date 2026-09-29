# pdf-cli

An **AI-native PDF toolkit** for the command line. Every operation is a non-interactive
flag-driven command with machine-readable JSON output, so an AI agent can drive it end to end.

- **No prompts, ever** — all input via flags; safe to run unattended.
- **JSON-first** — `--json` returns a stable `{success, command, output, details}` envelope.
- **Dry-run support** — `--dry-run` previews every mutating operation.
- **Structured errors** — stable error codes with actionable hints.

## Features

| Command | What it does |
|---|---|
| `convert` | Images (jpg/png/bmp/webp/tiff/gif) and Markdown/text → PDF |
| `web-to-pdf` | URL / HTML / SVG / image → PDF via headless Microsoft Edge |
| `pdf-to-image` | PDF pages → PNG / JPEG (via pdfium) |
| `invoice` | Tile images/PDFs onto A4, 2 per page (top/bottom) |
| `pagenum` | Add page numbers (position / format / start configurable) |
| `meta` | Edit document metadata (title / author / subject / keywords / …) |
| `extract-images` | Extract embedded images in their original format (no re-render) |
| `ocr` | OCR scanned PDFs via an external ocr-rust HTTP service |
| `extract` | Extract text from a PDF (optionally a page range) |
| `merge` | Combine multiple PDFs into one |
| `split` | Split a PDF by page or page range |
| `delete-pages` | Remove specific pages |
| `rotate` | Rotate pages by 90/180/270 |
| `watermark-add` | Add a text watermark (opacity / rotation / size) |
| `compress` | Reduce file size (prune objects + recompress streams) |
| `encrypt` | Password-protect a PDF (128-bit) |
| `decrypt` | Remove password protection |
| `info` | Quick metadata: pages, version, encryption, doc info |
| `doctor` | Deep structural analysis: per-page size, images, fonts, warnings |

Run `pdf-cli <command> --help` for per-command usage and examples.

## Build

Requires a Rust toolchain (edition 2021).

```powershell
git clone https://cnb.cool/free_llm/vibe-coding/pdf-cli.git
cd pdf-cli
cargo build --release
```

## Usage

### JSON contract

Every command accepts the global flags `--json`, `--dry-run`, and `--quiet`.

```jsonc
// success
{"success": true, "command": "info", "output": "human text", "details": { /* ... */ }}

// error (exit code 1)
{"error": true, "code": "FILE_NOT_FOUND", "message": "...\n  -> hint"}
```

Error codes: `FILE_NOT_FOUND`, `UNSUPPORTED_FORMAT`, `PARSE_FAILED`,
`PASSWORD_REQUIRED`, `OPERATION_FAILED`, `INVALID_PAGE_RANGE`, `MISSING_ARG`,
`BROWSER_NOT_FOUND`.

### Examples

```powershell
# Inspect a PDF
pdf-cli doctor --input doc.pdf --json

# Images -> PDF
pdf-cli convert --input "a.png,b.png" --output album.pdf --page-size a4

# Web page / SVG -> PDF (headless Edge)
pdf-cli web-to-pdf --input https://example.com --output page.pdf
pdf-cli web-to-pdf --input logo.svg --output logo.pdf

# PDF -> images (needs pdfium.dll)
pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --format png --width 2000

# Extract embedded images (original format)
pdf-cli extract-images --input doc.pdf --output-dir ./imgs

# A4 invoice sheet, 2 items per page
pdf-cli invoice --input "inv1.jpg,inv2.pdf,inv3.png" --output invoices.pdf

# Page numbers / metadata
pdf-cli pagenum --input doc.pdf --output numbered.pdf --format "第 {n} 页 / 共 {total} 页"
pdf-cli meta --input doc.pdf --output out.pdf --title "报告" --author "张三"

# OCR a scanned PDF (requires running ocr-rust service + pdfium)
pdf-cli ocr --input scan.pdf --output text.txt --service http://localhost:18004

# Page surgery
pdf-cli merge  --input "a.pdf,b.pdf" --output merged.pdf
pdf-cli split  --input merged.pdf --output-dir ./parts
pdf-cli delete-pages --input merged.pdf --pages 2,5-7 --output clean.pdf
pdf-cli rotate --input clean.pdf --angle 90 --output rotated.pdf

# Watermark / compress / encrypt
pdf-cli watermark-add --input doc.pdf --text "CONFIDENTIAL" --opacity 0.2 --output marked.pdf
pdf-cli compress --input large.pdf --output small.pdf --level maximum
pdf-cli encrypt --input doc.pdf --user-password secret --output locked.pdf
pdf-cli decrypt --input locked.pdf --password secret --output unlocked.pdf

# Text extraction
pdf-cli extract --input doc.pdf --pages 1-3 --json
```

Page-range grammar: `1-3,5,7-9`.

## Runtime dependencies

Most commands are pure Rust. External needs:

| Command | Needs |
|---|---|
| `web-to-pdf` | Microsoft Edge (auto-detected; `--edge-path`) |
| `pdf-to-image` | `pdfium.dll` (`--pdfium-path`) |
| `ocr` | `pdfium.dll` **and** a running ocr-rust service |

`pdfium.dll`: download from <https://github.com/bblanchon/pdfium-binaries/releases>.

## OCR integration

`ocr` is a thin HTTP client — it does **not** bundle any OCR engine. It renders PDF pages
(via pdfium) and POSTs them to a running **ocr-rust** service (separate project, PP-OCRv6 ONNX).

```powershell
# 1. start the OCR service (from the ocr-rust project)
cd F:\rust\ocr-rust
.\ocr-rust.exe        # http://localhost:18004

# 2. run OCR from pdf-cli
pdf-cli ocr --input scan.pdf --output text.txt --service http://localhost:18004 --json
```

If the service is unreachable, the command fails with a clear hint.

## AI agent integration

See [`skill.md`](skill.md) — a self-contained operating manual for AI agents, and
[`ROADMAP.md`](ROADMAP.md) for planned features.

## Project layout

```text
src/
├── main.rs            # CLI entry point (clap command definitions)
├── error.rs           # Structured error type + JSON serialization
├── output.rs          # Unified text/JSON result envelope
└── commands/
    ├── convert.rs        # images / md / txt -> PDF
    ├── webpdf.rs         # URL / HTML / SVG / image -> PDF (headless Edge)
    ├── pdf2img.rs        # PDF -> PNG / JPEG (pdfium)
    ├── invoice.rs        # A4 2-up tiling
    ├── pagenum.rs        # page numbers
    ├── meta.rs           # metadata editing
    ├── extract_images.rs # embedded image extraction
    ├── ocr.rs            # OCR via external ocr-rust service
    ├── extract.rs        # text extraction
    ├── pages.rs          # merge / split / delete-pages / rotate
    ├── watermark.rs      # text watermark
    ├── security.rs       # encrypt / decrypt
    ├── optimize.rs       # compress
    ├── info.rs           # metadata
    └── doctor.rs         # structural analysis
```

## Limitations

- **OCR** requires an external ocr-rust service.
- **Watermark removal / font repair / digital signature / form fill / PDF→Word**: not supported (see ROADMAP.md).
- **SVG → PDF** goes through headless Edge.

## License

MIT
