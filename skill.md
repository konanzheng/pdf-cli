# pdf-cli — AI Agent Skill Document

Non-interactive PDF toolkit. Every input is a CLI flag; there are **no prompts**.
Binary: `pdf-cli.exe`

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

Page-range grammar (used by `extract`, `delete-pages`, `rotate`, `split`, `pdf-to-image`, `ocr`): `1-3,5,7-9`.

## Commands

### info — quick metadata
```
pdf-cli info --input doc.pdf --json
```

### doctor — deep structural analysis
```
pdf-cli doctor --input doc.pdf --json
```
Reports file size, version, encryption, per-page size/rotation/images/fonts/content bytes,
image filters, font types, metadata, warnings.

### convert — images / md / txt -> PDF (pure Rust)
```
pdf-cli convert --input photo.jpg --output out.pdf
pdf-cli convert --input "a.png,b.png" --output album.pdf --page-size a4
pdf-cli convert --input doc.md --output doc.pdf --font-size 12
```
Inputs: jpg, jpeg, png, bmp, webp, tiff, gif, md, txt.

### web-to-pdf — URL / HTML / SVG / image -> PDF (headless Edge)
```
pdf-cli web-to-pdf --input https://example.com --output page.pdf
pdf-cli web-to-pdf --input logo.svg --output logo.pdf
pdf-cli web-to-pdf --input photo.png --output photo.pdf --landscape
pdf-cli web-to-pdf --input a.html --output a.pdf --edge-path "D:\Edge\msedge.exe"
```
Requires Microsoft Edge (auto-detected; `--edge-path` to override). Missing -> `BROWSER_NOT_FOUND`.

### pdf-to-image — PDF pages -> PNG/JPEG
```
pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs
pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --format jpeg --width 1600
pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --pages 1-3
pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --pdfium-path C:\pdfium\pdfium.dll
```
**Requires pdfium.dll** (https://github.com/bblanchon/pdfium-binaries/releases).
Output: `<stem>_<page:03>.<ext>`.

### invoice — A4 2-up sheet (images + PDFs)
```
pdf-cli invoice --input "inv1.jpg,inv2.jpg" --output merged.pdf
pdf-cli invoice --input "a.pdf,b.png,c.pdf" --output bundle.pdf
```
2 items per A4 page (top / bottom), auto-scaled and centered. PDF inputs use only the first page.

### pagenum — add page numbers
```
pdf-cli pagenum --input doc.pdf --output numbered.pdf
pdf-cli pagenum --input doc.pdf --output numbered.pdf --position bottom-center --format "{n} / {total}"
pdf-cli pagenum --input doc.pdf --output numbered.pdf --format "第 {n} 页 / 共 {total} 页" --start 1
```
Positions: `bottom-center` (default), `bottom-left`, `bottom-right`, `top-center`, `top-left`, `top-right`.
Format template supports `{n}` (current) and `{total}` (last).

### meta — edit document metadata
```
pdf-cli meta --input doc.pdf --output out.pdf --title "Report" --author "Alice"
pdf-cli meta --input doc.pdf --output out.pdf --subject "Q3" --keywords "finance,report"
```
Options: `--title`, `--author`, `--subject`, `--keywords`, `--creator`, `--producer`. Only supplied fields change.

### extract-images — extract embedded images (original format)
```
pdf-cli extract-images --input doc.pdf --output-dir ./imgs
pdf-cli extract-images --input doc.pdf --output-dir ./imgs --min-size 200
```
Pulls embedded XObject images **without re-rendering**:
- DCTDecode -> `.jpg` (raw bytes), JPXDecode -> `.jp2`
- FlateDecode / raw -> `.png` (decoded and re-encoded)
Deduplicates images referenced by multiple pages. `--min-size` skips tiny images (larger side, px).

### ocr — OCR scanned PDFs via external ocr-rust service
```
pdf-cli ocr --input scan.pdf --output text.txt
pdf-cli ocr --input scan.pdf --pages 1-5 --service http://localhost:18004 --json
pdf-cli ocr --input scan.pdf --pdfium-path C:\pdfium\pdfium.dll --service http://localhost:18004
```
**Requires**: (1) `pdfium.dll` to render pages, (2) a running **ocr-rust** HTTP service.

Start the service first:
```
cd F:\rust\ocr-rust
.\ocr-rust.exe          # listens on http://localhost:18004, models must be alongside the exe
```
Options: `--service` (default `http://localhost:18004`), `--width` render width (default 2000),
`--pages`, `--pdfium-path`, `--output` (default: stdout).
If the service is unreachable -> `OPERATION_FAILED` with a hint to start ocr-rust.

### extract — text from PDF
```
pdf-cli extract --input doc.pdf
pdf-cli extract --input doc.pdf --output text.txt
pdf-cli extract --input doc.pdf --pages 1-5 --json
```

### merge / split / delete-pages / rotate
```
pdf-cli merge  --input a.pdf,b.pdf --output merged.pdf
pdf-cli split  --input doc.pdf --output-dir ./pages
pdf-cli split  --input doc.pdf --ranges 1-3,4-6 --output-dir ./parts
pdf-cli delete-pages --input doc.pdf --pages 2,5-7 --output clean.pdf
pdf-cli rotate --input doc.pdf --angle 90 --output rotated.pdf
```
`rotate` angle: 90, 180, or 270 (cumulative).

### watermark-add
```
pdf-cli watermark-add --input doc.pdf --text "CONFIDENTIAL" --output marked.pdf
pdf-cli watermark-add --input doc.pdf --text "DRAFT" --opacity 0.3 --rotation 45 --output out.pdf
```
`--opacity` 0.0–1.0 (default 0.2), `--rotation` degrees (default 45), `--font-size` (default 60).

### encrypt / decrypt
```
pdf-cli encrypt --input doc.pdf --user-password secret --output locked.pdf
pdf-cli decrypt --input locked.pdf --password secret --output unlocked.pdf
```
Wrong password -> `PASSWORD_REQUIRED`.

### compress
```
pdf-cli compress --input large.pdf --output small.pdf
pdf-cli compress --input large.pdf --output small.pdf --level maximum
```
Levels: fast, balanced (default), maximum.

## Agent Workflow Examples

User: "把这个PDF加页码"
```powershell
pdf-cli pagenum --input doc.pdf --output numbered.pdf --format "{n} / {total}" --json
```

User: "改一下这个PDF的标题和作者"
```powershell
pdf-cli meta --input doc.pdf --output out.pdf --title "季度报告" --author "张三" --json
```

User: "把PDF里的图片都提取出来"
```powershell
pdf-cli extract-images --input doc.pdf --output-dir ./imgs --json
```

User: "这是扫描件，帮我识别文字"
```powershell
pdf-cli ocr --input scan.pdf --output text.txt --json
```

User: "把这个网页保存成PDF"
```powershell
pdf-cli web-to-pdf --input "https://example.com" --output page.pdf --json
```

User: "把这些发票图片按A4上下拼起来"
```powershell
pdf-cli invoice --input "inv1.jpg,inv2.jpg,inv3.jpg" --output invoices.pdf --json
```

User: "这个PDF前3页提取成文本"
```powershell
pdf-cli extract --input doc.pdf --pages 1-3 --json
```

User: "这个PDF里有多少图片/字体/每页多大"
```powershell
pdf-cli doctor --input doc.pdf --json
```

## Safety

- `--dry-run` available for all mutating commands.
- Output files are overwritten silently; use distinct output paths.

## Limitations

- `pdf-to-image` and `ocr` require `pdfium.dll` (not bundled).
- `web-to-pdf` requires Microsoft Edge.
- `ocr` additionally requires a running ocr-rust service (separate project).
- **Watermark removal / font repair / digital signature / form fill / PDF→Word**: not supported (see ROADMAP.md).
- **SVG → PDF** goes through headless Edge.

## Token-Efficient Patterns

- Always pass `--json`; parse `details` for structured data.
- `output` is human text; `details` is machine-parseable.
- Pass comma-separated paths to `--input` (convert, merge, invoice).
