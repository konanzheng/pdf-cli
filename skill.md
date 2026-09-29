# pdf-cli — AI Agent Skill Document

Non-interactive PDF toolkit. Every input is a CLI flag; there are **no prompts**.
Binary: `F:\rust\cnb\pdf-cli\pdf-cli.exe`

## Core Contract

- **Non-interactive**: all inputs via flags.
- **`--json`** (global) — machine-readable output:
  ```json
  {"success": true, "command": "info", "output": "human text", "details": {...}}
  ```
- **`--dry-run`** (global) — preview, no writes.
- **`--quiet`** (global) — suppress stderr progress.
- Exit `0` = success, `1` = error. With `--json`, errors are:
  ```json
  {"error": true, "code": "FILE_NOT_FOUND", "message": "...\n  -> hint"}
  ```

Error codes: `FILE_NOT_FOUND`, `UNSUPPORTED_FORMAT`, `PARSE_FAILED`,
`PASSWORD_REQUIRED`, `OPERATION_FAILED`, `INVALID_PAGE_RANGE`, `MISSING_ARG`,
`BROWSER_NOT_FOUND`.

Page-range grammar (used by `extract`, `delete-pages`, `rotate`, `split`, `pdf-to-image`): `1-3,5,7-9`.

## Commands

### info — quick metadata
```
pdf-cli info --input doc.pdf
pdf-cli info --input doc.pdf --json
```
Returns page count, encryption flag, PDF version, metadata.

### doctor — deep structural analysis
```
pdf-cli doctor --input doc.pdf
pdf-cli doctor --input doc.pdf --json
```
Reports file size, PDF version, linearization, encryption, per-page size (pt/mm/standard),
rotation, image count, fonts, content bytes; image filters; font types; metadata; warnings
(empty pages, mixed sizes, encryption). Run this before any edit.

### convert — images / md / txt -> PDF (pure Rust)
```
pdf-cli convert --input photo.jpg --output out.pdf
pdf-cli convert --input "a.png,b.png" --output album.pdf --page-size a4
pdf-cli convert --input doc.md --output doc.pdf --font-size 12
```
Inputs: jpg, jpeg, png, bmp, webp, tiff, gif, md, txt.
Page sizes: a4 (default), a3, a5, letter. Images scaled to fit and centered.

### web-to-pdf — URL / HTML / SVG / image -> PDF (headless Edge)
```
pdf-cli web-to-pdf --input https://example.com --output page.pdf
pdf-cli web-to-pdf --input logo.svg --output logo.pdf
pdf-cli web-to-pdf --input photo.png --output photo.pdf --landscape
pdf-cli web-to-pdf --input a.html --output a.pdf --edge-path "D:\Edge\msedge.exe"
```
Uses Microsoft Edge headless mode (`--print-to-pdf`). Edge is auto-detected from standard
install paths; override with `--edge-path`. If Edge is not found -> error code `BROWSER_NOT_FOUND`.
- URLs: rendered directly.
- HTML: rendered directly.
- SVG / raster images: wrapped in an A4 HTML shell and centered.
- `--landscape` prints in landscape.

### pdf-to-image — PDF pages -> PNG/JPEG
```
pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs
pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --format jpeg --width 1600
pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --pages 1-3
pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --pdfium-path C:\pdfium\pdfium.dll
```
**Requires the pdfium dynamic library.** If not on the system, download from
https://github.com/bblanchon/pdfium-binaries/releases and pass `--pdfium-path <pdfium.dll>`.
Options: `--format png|jpeg` (default png), `--width` px (default 2000), `--pages`.
Output naming: `<stem>_<page:03>.<ext>`.

### invoice — A4 2-up sheet (images + PDFs)
```
pdf-cli invoice --input "inv1.jpg,inv2.jpg" --output merged.pdf
pdf-cli invoice --input "a.pdf,b.png,c.pdf" --output bundle.pdf
```
Places items onto A4 pages, **2 per page (top / bottom)**. Each item is auto-scaled to fit
its half and centered. For PDF inputs, only the first page is used. Images and PDFs can be mixed.
Items are processed in the order given, filling top slot then bottom slot per page.

### extract — text from PDF
```
pdf-cli extract --input doc.pdf
pdf-cli extract --input doc.pdf --output text.txt
pdf-cli extract --input doc.pdf --pages 1-5 --json
```

### merge — combine PDFs
```
pdf-cli merge --input a.pdf,b.pdf,c.pdf --output merged.pdf
```

### split — split PDF
```
pdf-cli split --input doc.pdf --output-dir ./pages
pdf-cli split --input doc.pdf --ranges 1-3,4-6 --output-dir ./parts
```
Without `--ranges` -> one file per page.

### delete-pages — remove pages
```
pdf-cli delete-pages --input doc.pdf --pages 2,5-7 --output clean.pdf
```
Refuses to delete all pages.

### rotate — rotate pages
```
pdf-cli rotate --input doc.pdf --angle 90 --output rotated.pdf
pdf-cli rotate --input doc.pdf --angle 180 --pages 1-3 --output out.pdf
```
Angle: 90, 180, or 270. Cumulative (mod 360).

### watermark-add — text watermark
```
pdf-cli watermark-add --input doc.pdf --text "CONFIDENTIAL" --output marked.pdf
pdf-cli watermark-add --input doc.pdf --text "DRAFT" --opacity 0.3 --rotation 45 --output out.pdf
```
`--opacity` 0.0–1.0 (default 0.2), `--rotation` degrees (default 45), `--font-size` (default 60).

### encrypt — password protect
```
pdf-cli encrypt --input doc.pdf --user-password secret --output locked.pdf
pdf-cli encrypt --input doc.pdf --owner-password admin --output locked.pdf
```
At least one of `--user-password` / `--owner-password` required.

### decrypt — remove protection
```
pdf-cli decrypt --input locked.pdf --password secret --output unlocked.pdf
```
Wrong password -> `PASSWORD_REQUIRED`.

### compress — reduce size
```
pdf-cli compress --input large.pdf --output small.pdf
pdf-cli compress --input large.pdf --output small.pdf --level maximum
```
Levels: fast, balanced (default), maximum.

## Agent Workflow Examples

User: "把这个PDF前3页提取成文本"
```powershell
pdf-cli extract --input doc.pdf --pages 1-3 --json
```

User: "把这几张图片做成PDF"
```powershell
pdf-cli convert --input "img1.jpg,img2.png,img3.jpg" --output album.pdf --page-size a4 --json
```

User: "把这个网页保存成PDF"
```powershell
pdf-cli web-to-pdf --input "https://example.com" --output page.pdf --json
```

User: "把这个SVG转成PDF"
```powershell
pdf-cli web-to-pdf --input logo.svg --output logo.pdf --json
```

User: "把这个PDF每页导成图片"
```powershell
pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --format png --json
```

User: "把这些发票图片按A4上下拼起来"
```powershell
pdf-cli invoice --input "inv1.jpg,inv2.jpg,inv3.jpg" --output invoices.pdf --json
```

User: "这个PDF太大了"
```powershell
pdf-cli compress --input large.pdf --output small.pdf --json
```

User: "加个'机密'水印"
```powershell
pdf-cli watermark-add --input doc.pdf --text "机密" --opacity 0.15 --rotation 45 --output marked.pdf --json
```

User: "这个PDF里有多少图片/字体/每页多大"
```powershell
pdf-cli doctor --input doc.pdf --json
```

## Safety

- `--dry-run` available for all mutating commands.
- Output files are overwritten silently; use distinct output paths.
- `compress --level maximum` may reduce quality.

## Limitations

- **pdf-to-image** requires `pdfium.dll` at runtime (not bundled). Pass `--pdfium-path` or install system-wide.
- **web-to-pdf** requires Microsoft Edge installed. Pass `--edge-path` if non-standard.
- **OCR**: NOT supported. Scanned PDFs -> extraction may be empty or garbled.
- **去水印 / 字体修复**: no dedicated command; `compress` only prunes + recompresses.
- **PDF-to-Word**: not supported.

## Token-Efficient Patterns

- Always pass `--json`; parse `details` for structured data.
- `output` is human text; `details` is machine-parseable.
- Pass comma-separated paths to `--input` (convert, merge, invoice) instead of shell globs.
