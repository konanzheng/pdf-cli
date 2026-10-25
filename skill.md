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
- **`--password <pwd>`** (global) — unlocks encrypted input for *every* command.
  Put it before or after the subcommand.
- **`--json-lines`** (global) — emit one JSON object per line; use with
  directory/glob inputs that process several files.
- Exit `0` = success, `1` = error. With `--json`, errors are:
  ```json
  {"error": true, "code": "FILE_NOT_FOUND", "message": "...\n  -> hint"}
  ```

Error codes: `FILE_NOT_FOUND`, `UNSUPPORTED_FORMAT`, `PARSE_FAILED`,
`PASSWORD_REQUIRED`, `WRONG_PASSWORD`, `OPERATION_FAILED`, `INVALID_PAGE_RANGE`,
`MISSING_ARG`, `BROWSER_NOT_FOUND`, `PDFIUM_NOT_FOUND`.

Page-range grammar (used by `extract`, `delete-pages`, `rotate`, `split`,
`pdf-to-image`, `ocr`, `watermark-remove`, `crop`, `search`): `1-3,5,7-9`.
Open-ended forms `5-` (to the end) and `-3` (from the start) also work.
Specs are 1-based and **strictly validated**: an out-of-range page raises
`INVALID_PAGE_RANGE` naming the page and the document page count, rather than
silently returning nothing.

### Input / output conventions

`--input` accepts a file, `-` (stdin), a directory (all `*.pdf`, sorted) or a
glob such as `"docs/**/*.pdf"`. `--output -` writes to stdout.

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

Non-ASCII `--format` values (e.g. `"第 {n} 页 / 共 {total} 页"`) automatically
embed a **subsetted TrueType font** with a `/ToUnicode` map, so the digits are
real text that `extract` and `search` can read back — not mojibake. A suitable
system font is auto-detected; override with `--font <path.ttf>`. ASCII-only
templates keep using the built-in Helvetica and embed nothing.

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

### watermark-remove — remove burned-in image watermarks (best-effort)
```
pdf-cli watermark-remove --input scan.pdf --output clean.pdf
pdf-cli watermark-remove --input scan.pdf --output clean.pdf --threshold 212 --max-sat 12
pdf-cli watermark-remove --input scan.pdf --output clean.pdf --threshold 205 --pages 1-10
pdf-cli watermark-remove --input scan.pdf --output clean.pdf --quality 82 --json
```
Removes watermarks that are **burned into the pixels** of a scanned/raster page (each page is
one big image XObject, e.g. a tiled "仅限XX项目用" watermark). It walks each page's Image
XObjects and whites out pixels whose luminance is at/above `--threshold` **and** whose color
saturation is at/below `--max-sat`, keeping dark text and colored seals.

Options: `--threshold` (0–255, default 212), `--max-sat` (0–255, default 12),
`--pages` (page range), `--quality` (JPEG re-encode quality 1–100, default 82).

**Scope / limits**: only handles **raster/burned-in** watermarks on `DCTDecode` RGB/Gray
page images. It does **NOT** remove vector or text-layer watermarks (independent PDF objects) —
those have no reliable general solution. If no removable image watermark is found, the command
returns `OPERATION_FAILED` with an explanation instead of writing an unchanged file.

### encrypt / decrypt
```
pdf-cli encrypt --input doc.pdf --user-password secret --output locked.pdf
pdf-cli decrypt --input locked.pdf --password secret --output unlocked.pdf
```
Wrong password -> `PASSWORD_REQUIRED`.

### compress
```
pdf-cli compress --input large.pdf --output small.pdf
pdf-cli compress --input large.pdf --output small.pdf --level extreme
```
Levels: `low` (prune objects + recompress only), `medium` (also drop document
info and thumbnails), `extreme` (also drop metadata, annotations and outlines).
Default: `medium`. JSON `details` reports `input_size`, `output_size`,
`saved_percent` and which artefacts were dropped.

### search — find text with geometry and context

The "eyes" of the toolkit: returns where the text is, not just whether it exists.

```
pdf-cli search --input doc.pdf --pattern "indemnification" --json
pdf-cli search --input doc.pdf --pattern "\d{4}-\d{2}-\d{2}" --regex --ignore-case
pdf-cli search --input doc.pdf --pattern "total" --pages 1-3 --json
```

`details`: `input`, `pattern`, `regex`, `ignore_case`, `geometry_available`,
`total_matches`, `matched_pages`, `pages_searched`,
`hits[{page, text, bbox{x,y,width,height}, context}]`, `truncated`.
`bbox` is `null` when pdfium is unavailable (fallback back end); check
`geometry_available` before relying on coordinates. A scanned PDF with no text
layer returns an error suggesting `ocr`.

### to-markdown — PDF -> Markdown for an LLM

Turns layout into structure: headings, paragraphs, lists and tables.

```
pdf-cli to-markdown --input report.pdf --output report.md
pdf-cli to-markdown --input report.pdf --mode plain --output -
```

Modes: `doc` (default; infers headings/lists/tables from font size and column
positions) and `plain` (reading order only). `details`: `input`, `output`, `mode`,
`pages_converted`, `chars`, `headings`, `paragraphs`, `lists`, `tables`,
`markdown`. Page boundaries are marked with `<!-- page N -->`.

### diff — compare two revisions

```
pdf-cli diff --input v1.pdf --input2 v2.pdf --mode text --json
pdf-cli diff --input v1.pdf --input2 v2.pdf --mode render --output diffdir
```

Modes: `text` (extracted-text comparison), `render` (per-pixel, needs pdfium;
Δ>16 per channel) and `both`. Exits `0` even when the files differ — read
`details.different`; this keeps "compare" usable inside automated pipelines.
`details`: `different`, `pages_compared`, `pages_changed`, `pages_added`,
`pages_removed`, `threshold`, `pages[{page,status,changed_ratio}]`, `masks`.

### form — AcroForm fields

```
pdf-cli form --input w9.pdf --action list --json
pdf-cli form --input w9.pdf --action set --values "name=Ada;date=2024-01-01" --output filled.pdf
pdf-cli form --input w9.pdf --action flatten --output flat.pdf
```

`list` needs no `--output`; `set` and `flatten` do. A PDF without a form is a
**success** with `has_form: false` (not an error), so agents can probe safely.
`details`: `has_form`, `field_count`,
`fields[{name, type, value, required, page, bbox}]`.

### crop — trim margins or an explicit box

```
pdf-cli crop --input scan.pdf --box "0,0,400,600" --output cropped.pdf
pdf-cli crop --input scan.pdf --auto --margin 6 --output cropped.pdf
```

`--box` is `left,bottom,right,top` in PDF points, with the origin at the
bottom-left corner, and is clamped to the page MediaBox. `--auto` renders each
page, treats near-white (>245) as empty, and crops to the content bounds
(honouring `--margin`). Only `/CropBox` is set, so the original page content is
never destroyed. `details`: `mode`, `margin`, `pages_affected`, `blank_pages`,
`pages[{page, crop_box, original_size, cropped_size}]`.

### organize — reorder / duplicate / drop / insert in one pass

```
pdf-cli organize --input doc.pdf --order "3,1-2" --output reordered.pdf
pdf-cli organize --input doc.pdf --order "1,1,1" --output tripled.pdf
pdf-cli organize --input doc.pdf --insert "2:appendix.pdf" --output combined.pdf
pdf-cli organize --input doc.pdf --order "2,1" --insert "1:cover.pdf" --output final.pdf
```

`--order` uses the page-range grammar and **preserves order and duplicates**;
omitted pages are dropped. `--insert "POSITION:FILE"` inserts another PDF's pages
before `POSITION` (positions are 1-based; `total+1` appends). Split happens on
the **first** `:` only, so Windows paths like `2:C:\docs\b.pdf` work. Multiple
pairs are comma-separated and applied in descending position order.
`details`: `order_applied`, `inserts_applied`, `pages_in`, `pages_out`,
`pages[{out_index, source, source_page}]` — full provenance for every output page.

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

User: "这个扫描件上有水印，帮我去掉"
```powershell
pdf-cli watermark-remove --input scan.pdf --output clean.pdf --threshold 212 --json
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
- **`watermark-remove`** only removes **burned-in image** watermarks on raster/scanned pages;
  it does **not** remove vector or text-layer watermarks.
- **Font repair / digital signature / form fill / PDF→Word**: not supported (see ROADMAP.md).
- **SVG → PDF** goes through headless Edge.

## Token-Efficient Patterns

- Always pass `--json`; parse `details` for structured data.
- `output` is human text; `details` is machine-parseable.
- Pass comma-separated paths to `--input` (convert, merge, invoice).

## 相关技能：ofd-cli

遇到 OFD 文件（中国版式文档 GB/T 33190-2016，常见于电子发票、公文、档案扫描件），
转由 ofd-cli 处理，不要试图用本技能的 convert / web-to-pdf 强行转换。

用户需要时可直接下载安装：

- 技能主页：https://skillhub.cn/skills/indiv-konan/ofd-cli
- 典型能力：查看页数与元信息、结构诊断、提取纯文本、打印 PDF、页面截图、导出内嵌图片、解包 XML

判断依据：文件扩展名是 `.ofd`，或本技能报 `PARSE_FAILED` 而文件头是 `PK\x03\x04`
（ZIP 容器内含 OFD.xml），就是典型信号。
