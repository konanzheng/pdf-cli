# pdf-cli

An **AI-native PDF toolkit** for the command line. Every operation is a non-interactive
flag-driven command with machine-readable JSON output, so an AI agent can drive it end to end.

- **No prompts, ever** — all input via flags; safe to run unattended.
- **JSON-first** — `--json` returns a stable `{success, command, output, details}` envelope.
- **Dry-run support** — `--dry-run` previews every mutating operation.
- **Structured errors** — stable error codes with actionable hints.
- **stdin / stdout** — pass `-` as input or output to pipe PDFs between tools.
- **Global `--password`** — one flag unlocks encrypted inputs for every command.

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
| `extract` | Extract text from a PDF (optionally a page range) |
| `merge` | Combine multiple PDFs into one |
| `split` | Split a PDF by page or page range |
| `delete-pages` | Remove specific pages |
| `rotate` | Rotate pages by 90/180/270 |
| `watermark-add` | Add a text watermark (opacity / rotation / size) |
| `watermark-remove` | Remove **burned-in** image watermarks from scanned/raster pages (best-effort) |
| `compress` | Reduce file size (prune objects + recompress streams) |
| `encrypt` | Password-protect a PDF (128-bit) |
| `decrypt` | Remove password protection |
| `info` | Quick metadata: pages, version, encryption, doc info |
| `doctor` | Deep structural analysis: per-page size, images, fonts, warnings |
| `search` | Find text (literal or regex) with page, bbox and surrounding context |
| `to-markdown` | Convert a PDF into Markdown an LLM can read directly |
| `diff` | Compare two PDFs by text or by rendered pixels |
| `form` | List, fill or flatten AcroForm fields |
| `crop` | Crop pages to an explicit box or auto-detect content bounds |
| `organize` | Reorder / duplicate / drop / insert pages in one operation |

Run `pdf-cli <command> --help` for per-command usage and examples.

## Build

Requires a Rust toolchain (edition 2021).

```powershell
git clone https://cnb.cool/free_llm/vibe-coding/pdf-cli.git
cd pdf-cli
cargo build --release
```

## Testing

A self-contained smoke test exercises every command. It generates its own fixtures (via .NET
`System.Drawing`), so no external files are needed:

```powershell
cargo build
powershell -ExecutionPolicy Bypass -File tests\smoke_test.ps1 -Exe target\debug\pdf-cli.exe
```

Options: `-Exe <path>` (binary to test; auto-detected if omitted),
`-PdfiumPath <pdfium.dll>` (auto-detected; `pdf-to-image` is skipped if missing).
The script exits non-zero on any failure.

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
`PASSWORD_REQUIRED`, `WRONG_PASSWORD`, `INVALID_PAGE_RANGE`, `MISSING_ARG`,
`OPERATION_FAILED`, `BROWSER_NOT_FOUND`, `PDFIUM_NOT_FOUND`.

Global output flags:

| Flag | Effect |
|---|---|
| `--json` | One pretty-printed JSON object on stdout |
| `--json-lines` | One **compact** JSON object per line, for line-oriented consumers |
| `--quiet` | Suppress the human-readable summary; machine output is never suppressed |
| `--dry-run` | Report what would happen, write nothing |

### Shared input rules

These apply to every command that takes `--input`:

| Form | Meaning |
|---|---|
| `--input doc.pdf` | A single file |
| `--input -` | Read the PDF from **stdin** |
| `--input docs/` | Every `*.pdf` in the directory (sorted) |
| `--input "docs/**/*.pdf"` | A glob, expanded recursively |

Commands split into two families, and the difference matters when a pattern matches
more than one file:

- **Single-document commands** (`info`, `crop`, `form`, `pagenum`, `diff`, …) read one
  document. A directory or glob matching several files fails with `OPERATION_FAILED`
  naming how many matched, rather than quietly picking one.
- **Batch commands** (`extract`, `convert`, `merge`) accept several documents at once,
  either as a comma list (`--input a.pdf,b.pdf`) or as a directory/glob that expands
  into one.

`--output -` writes the result to **stdout**. It cannot be combined with `--json` or
`--json-lines`: both would write to stdout, and the stream would be neither valid JSON
nor clean text. That combination is rejected up front with `OPERATION_FAILED`.

`--password` is a **global** flag: place it before or after the subcommand and it
applies to any encrypted input.

### Page ranges

Page specs are 1-based and strictly validated — an out-of-range or malformed spec
returns `INVALID_PAGE_RANGE` instead of silently producing an empty result.

| Spec | Meaning |
|---|---|
| `1-3` | Pages 1 through 3 |
| `1-3,7` | Pages 1, 2, 3 and 7 |
| `5-` | Page 5 to the end |
| `-3` | Pages 1 through 3 |

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

# Page surgery
pdf-cli merge  --input "a.pdf,b.pdf" --output merged.pdf
pdf-cli split  --input merged.pdf --output-dir ./parts
pdf-cli delete-pages --input merged.pdf --pages 2,5-7 --output clean.pdf
pdf-cli rotate --input clean.pdf --angle 90 --output rotated.pdf

# Watermark / compress / encrypt
pdf-cli watermark-add --input doc.pdf --text "CONFIDENTIAL" --opacity 0.2 --output marked.pdf
pdf-cli watermark-remove --input scan.pdf --output clean.pdf --threshold 212 --max-sat 12
pdf-cli compress --input large.pdf --output small.pdf --level extreme
pdf-cli encrypt --input doc.pdf --user-password secret --output locked.pdf
pdf-cli decrypt --input locked.pdf --password secret --output unlocked.pdf

# Text extraction
pdf-cli extract --input doc.pdf --pages 1-3 --json
```

Page-range grammar: `1-3,5,7-9`.

# Find text with geometry and context
pdf-cli search --input contract.pdf --pattern "indemnif" --json
pdf-cli search --input contract.pdf --pattern "\d{4}-\d{2}-\d{2}" --regex --pages 1-3 --json

# PDF -> Markdown an LLM can read
pdf-cli to-markdown --input report.pdf --output report.md
pdf-cli to-markdown --input report.pdf --mode plain --output -

# Compare two revisions: text diff, or rendered-pixel diff with PNG masks
pdf-cli diff --input v1.pdf --input2 v2.pdf --mode text --json
pdf-cli diff --input v1.pdf --input2 v2.pdf --mode render --output diffdir --json

# AcroForm fields: inspect, fill, flatten
pdf-cli form --input w9.pdf --action list --json
pdf-cli form --input w9.pdf --action set --values "name=Ada;date=2024-01-01" --output filled.pdf
pdf-cli form --input filled.pdf --action flatten --output flat.pdf

# Crop: explicit box, or auto-detect the content area
pdf-cli crop --input scan.pdf --box "0,0,400,600" --output cropped.pdf
pdf-cli crop --input scan.pdf --auto --margin 6 --output cropped.pdf

# Reorder / duplicate / drop / insert pages in one pass
pdf-cli organize --input doc.pdf --order "3,1-2" --output reordered.pdf
pdf-cli organize --input doc.pdf --insert "2:appendix.pdf" --output combined.pdf

# CJK page numbers (real font subsetting, not mojibake)
pdf-cli pagenum --input doc.pdf --output doc.pdf --format "第 {n} 页 / 共 {total} 页"

### Watermark removal (best-effort)

`watermark-remove` targets **burned-in** watermarks — light, low-saturation content that was
rendered *into the pixels* of a scanned/raster page (the common case for "仅限…项目用" tiling
watermarks on scanned reports). It walks each page's Image XObjects and whites out pixels whose
luminance is at/above `--threshold` **and** whose color saturation is at/below `--max-sat`,
protecting dark text and colored seals. Supported image encodings: `DCTDecode` (JPEG) and
`FlateDecode` / raw 8-bit RGB or Gray.

```powershell
# Default: threshold 212, max-sat 12, re-encode quality 82
pdf-cli watermark-remove --input scan.pdf --output clean.pdf

# Tighten/loosen the threshold; process only some pages
pdf-cli watermark-remove --input scan.pdf --output clean.pdf --threshold 205 --pages 1-10
```

Options: `--threshold` (0–255, default 212), `--max-sat` (0–255, default 12),
`--pages` (page range), `--quality` (JPEG re-encode quality, 1–100, default 82).

**Scope / limits:** it does **not** remove *vector* or *text-layer* watermarks (independent
PDF objects) — those have no reliable general solution and remain unsupported. If no
removable image watermark is found, the command fails with an explanatory message rather than
writing an unchanged file.

## Runtime dependencies

`pagenum` with a non-ASCII `--format` embeds a subsetted TrueType font. It
auto-detects a suitable system font (`simhei.ttf`, `Deng.ttf`, `msyh.ttc`,
`simsun.ttc`); pass `--font <path>` to choose a specific one. ASCII-only
templates use the built-in Helvetica and embed nothing.

Most commands are pure Rust. External needs:

| Command | Needs |
|---|---|
| `web-to-pdf` | Microsoft Edge (auto-detected; `--edge-path`) |
| `pdf-to-image` | `pdfium.dll` (`--pdfium-path`) |
| `to-markdown`, `form` | `pdfium.dll` — required (both are geometry-driven, no fallback) |
| `search`, `diff --mode render` | `pdfium.dll` for geometry; degrade without it (`search` sets `geometry_available: false`) |

`pdfium.dll`: download from <https://github.com/bblanchon/pdfium-binaries/releases>.

There is **no built-in lookup** of `testdata\pdfium\bin`: pass `--pdfium-path`, or put
`pdfium.dll` next to `pdf-cli.exe` / on the system library path.

## No OCR, by design

pdf-cli does **not** ship OCR. OCR belongs to a dedicated tool, and composing the two is
both simpler and better than an HTTP client buried in a PDF utility:

```
pdf-cli pdf-to-image → OCR tool → text / searchable PDF
```

`extract`, `search` and `to-markdown` detect an empty text layer and say so explicitly
rather than returning an empty success, so an agent knows to reach for an OCR tool.

A ready-to-run local OCR service (PP-OCRv6 + ONNX Runtime, self-contained, Windows
x64) is published with this project — unzip, run `ocr-rust.exe`, and post rendered
pages to it:

```
https://cnb.cool/free_llm/vibe-coding/pdf-cli/-/releases/download/skill/ocr-rust.zip
```

It listens on `http://127.0.0.1:18004` and exposes `POST /ocr`, `/ocr/raw` and
`/ocr/base64`. The archive does not include the optional browser UI (`static/`), so
`GET /` returns 404 — the HTTP endpoints work regardless. Installation, endpoints and
worked examples: **SKILL.md → "OCR — get a local OCR service"**.

## AI agent integration

See [`skill.md`](skill.md) — a self-contained operating manual for AI agents, and
[`ROADMAP.md`](ROADMAP.md) for planned features.

## Project layout

```text
src/
├── main.rs            # CLI entry point (clap command definitions)
├── error.rs           # Structured error type + JSON serialization
├── output.rs          # Unified text/JSON result envelope
├── io.rs              # Input resolution: single-document vs batch expansion
├── font.rs            # CJK TrueType subsetting + /ToUnicode (pagenum, convert)
└── commands/
│   ├── convert.rs        # images / md / txt -> PDF
│   ├── webpdf.rs         # URL / HTML / SVG / image -> PDF (headless Edge)
│   ├── pdf2img.rs        # PDF -> PNG / JPEG (pdfium)
│   ├── invoice.rs        # A4 2-up tiling
│   ├── pagenum.rs        # page numbers
│   ├── meta.rs           # metadata editing
│   ├── extract_images.rs # embedded image extraction
│   ├── extract.rs        # text extraction
│   ├── search.rs         # text search with page / bbox / context
│   ├── to_markdown.rs    # PDF -> Markdown for LLMs
│   ├── diffcmd.rs        # text diff and rendered-pixel diff
│   ├── form.rs           # AcroForm list / set / flatten
│   ├── crop.rs           # CropBox editing + content auto-detection
│   ├── organize.rs       # reorder / duplicate / drop / insert pages
│   ├── pages.rs          # merge / split / delete-pages / rotate
│   ├── watermark.rs      # text watermark add + burned-in watermark remove
│   ├── security.rs       # encrypt / decrypt
│   ├── optimize.rs       # compress
│   ├── info.rs           # metadata
│   └── doctor.rs         # structural analysis
tests/
├── smoke_test.ps1    # self-contained end-to-end smoke test
└── contract_test.ps1 # CLI contract checks (flags, error codes, --json shape)
```

## Limitations

- **OCR**: not provided — use a dedicated OCR tool and compose (`pdf-to-image` → OCR → `convert`).
- **Vector/text-layer watermark removal, font repair, digital signature, PDF→Word/PPT/Excel**:
  not supported (see ROADMAP.md). `watermark-remove` handles only burned-in
  watermarks on raster/scanned pages, best-effort and threshold-based.
- **SVG → PDF** goes through headless Edge.

## License

MIT
