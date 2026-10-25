# pdf-cli — AI Agent Skill Document

Non-interactive PDF toolkit. Every input is a CLI flag; there are **no prompts**.
Binary: `pdf-cli.exe`

## Core Contract

- **Non-interactive**: all inputs via flags.
- **`--json`** (global) — machine-readable output:
  ```json
  {"success": true, "command": "info", "output": "human text", "details": {...}}
  ```
- **`--dry-run`** (global) — preview, no writes. `details.dry_run` is `true` and the
  report is fully populated, so a dry run tells you exactly what a real run would do.
- **`--quiet`** (global) — suppress the human-readable summary on **stdout**. Machine
  output (`--json`, `--json-lines`) is never suppressed: silencing it would leave you
  with no result at all.
- **`--password <pwd>`** (global) — unlocks encrypted input for *every* command.
  Put it before or after the subcommand.
- **`--json-lines`** (global) — one **compact** JSON object per line instead of a
  pretty-printed object. Errors follow the same rule, so a pipeline can read one line
  per result without a JSON parser that tolerates multi-line input.
- Exit `0` = success, `1` = error. With `--json`, errors are:
  ```json
  {"error": true, "code": "FILE_NOT_FOUND", "message": "...\n  -> hint"}
  ```
  Error JSON goes to **stdout**, human errors to **stderr**.

Error codes: `FILE_NOT_FOUND`, `UNSUPPORTED_FORMAT`, `PARSE_FAILED`,
`PASSWORD_REQUIRED`, `WRONG_PASSWORD`, `OPERATION_FAILED`, `INVALID_PAGE_RANGE`,
`MISSING_ARG`, `BROWSER_NOT_FOUND`, `PDFIUM_NOT_FOUND`.

Page-range grammar (used by `extract`, `delete-pages`, `rotate`, `split`,
`pdf-to-image`, `watermark-remove`, `crop`, `search`): `1-3,5,7-9`.
Open-ended forms `5-` (to the end) and `-3` (from the start) also work.
Specs are 1-based and **strictly validated**: an out-of-range page raises
`INVALID_PAGE_RANGE` naming the page and the document page count, rather than
silently returning nothing.

### Input / output conventions

`--input` accepts a file, `-` (stdin), a directory (all `*.pdf`, sorted) or a
glob such as `"docs/**/*.pdf"`.

There are two families of command, and the difference decides whether a pattern
matching several files is an error or a batch:

- **Single-document** (`info`, `crop`, `form`, `pagenum`, `diff`, `organize`, …) read one
  document. A directory or glob matching several files fails with `OPERATION_FAILED`
  stating how many matched — it never quietly picks one.
- **Batch** (`extract`, `convert`, `merge`) take several documents, as a comma list
  (`--input a.pdf,b.pdf`) or as a directory/glob expanded to many. `extract` reports
  per-document counts under `details.documents`.

`--output -` writes to stdout. **Do not** combine it with `--json`/`--json-lines`:
both would target stdout and the stream would be neither valid JSON nor clean text.
The combination is rejected up front with `OPERATION_FAILED`.

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
pdf-cli convert --input cn.txt --output cn.pdf --font C:\Windows\Fonts\simhei.ttf
```
Inputs: jpg, jpeg, png, bmp, webp, tiff, gif, md, txt.

**Text and fonts.** ASCII text renders with the built-in Helvetica, no font file needed.
Non-ASCII text (Chinese, Japanese, …) requires `--font <truetype file>`: the font is
subset and embedded, and the generated text stays extractable because a `/ToUnicode`
map is written alongside it. Asking for non-ASCII without `--font` is a hard error
naming the offending characters rather than a PDF full of blanks. A leading UTF-8 BOM
is stripped automatically and reported as `details.bom_stripped`.

Common CJK fonts on Windows: `simhei.ttf` (黑体), `Deng.ttf` (等线), `msyh.ttc` (雅黑),
`simsun.ttc` (宋体).

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

### extract — text from PDF
```
pdf-cli extract --input doc.pdf
pdf-cli extract --input doc.pdf --output text.txt
pdf-cli extract --input doc.pdf --pages 1-5 --json
```

**Scanned PDFs have no text layer, and pdf-cli does not do OCR.** `extract`, `search`
and `to-markdown` all fail loudly with `OPERATION_FAILED` ("No text layer found…")
instead of returning an empty success — that error is your signal to reach for an OCR
tool, not a bug. Compose the tools instead of expecting one to do everything — see
**OCR — get a local OCR service** below for a ready-to-run service:

```
pdf-cli pdf-to-image --input scan.pdf --output-dir ./pages --width 2400
#  -> point the OCR service at ./pages/*.png (see the OCR section below)
#  → feed the resulting text back:
pdf-cli convert --input ocr.txt --output searchable-notes.pdf
```
If the OCR tool emits a searchable PDF of its own, skip the last step — `search` and
`extract` read that file directly.

### merge / split / delete-pages / rotate
```
pdf-cli merge  --input a.pdf,b.pdf --output merged.pdf
pdf-cli split  --input doc.pdf --output-dir ./pages
pdf-cli split  --input doc.pdf --ranges 1-3,4-6 --output-dir ./parts
pdf-cli split  --input doc.pdf --every 10 --output-dir ./chunks
pdf-cli split  --input scan.pdf --odd-even --output-dir ./sides
pdf-cli delete-pages --input doc.pdf --pages 2,5-7 --output clean.pdf
pdf-cli rotate --input doc.pdf --angle 90 --output rotated.pdf
```
`rotate` angle: 90, 180, or 270 (cumulative).

`split` grouping strategies — pick **at most one**; giving two raises an error naming
the ones you passed rather than silently choosing:

| Flag | Result on a 7-page document |
|---|---|
| *(none)* | 7 files, one page each |
| `--ranges "1-3,4-6,7"` | 3 files with 3, 3 and 1 pages |
| `--every 3` | 3 files with 3, 3 and 1 pages (the short tail is kept) |
| `--odd-even` | 2 files: pages 1,3,5,7 then pages 2,4,6 |

`details.strategy` reports which grouping ran and `details.pages_per_file` gives each
output's page count, so a caller can verify the split without reopening the files.

`merge` carries over the document outline: each input contributes a top-level bookmark
named after the file, with that file's own entries nested beneath it, and destinations
remapped onto the merged page numbers. `details.outline_merged` / `details.outline_items`
report whether an outline was written and how many entries it holds. Inputs without an
outline produce no `/Outlines` key at all.

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
A wrong password returns `WRONG_PASSWORD`; omitting `--password` on `decrypt`
returns `MISSING_ARG`.

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
`geometry_available` before relying on coordinates. A scanned PDF with no text layer
returns `OPERATION_FAILED` — pdf-cli has no OCR, so route it through the local OCR
service described in **OCR — get a local OCR service**.

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

## OCR — get a local OCR service

pdf-cli does **not** recognise text. When `extract` / `search` / `to-markdown` return
`OPERATION_FAILED` with "No text layer found", the page is a scan: you need OCR. The
supported composition is a self-contained local OCR service that pdf-cli renders into:

```
pdf-cli pdf-to-image  →  OCR service  →  text
```

A ready-to-run build (PP-OCRv6, ONNX Runtime, self-contained, Windows x64) is published
with this project:

```
https://cnb.cool/free_llm/vibe-coding/pdf-cli/-/releases/download/skill/ocr-rust.zip
```

### Install

Download and extract; the archive contains a single `ocr-rust/` folder (~90 MB, 48 entries).
Nothing else to install — the ONNX runtime, DirectML provider, both model sets and the
browser UI all ship inside.

```powershell
Invoke-WebRequest -Uri https://cnb.cool/free_llm/vibe-coding/pdf-cli/-/releases/download/skill/ocr-rust.zip -OutFile ocr-rust.zip
Expand-Archive ocr-rust.zip -DestinationPath .
cd ocr-rust
```

Keep the directory layout intact: the executable looks for `models/` relative to its own
location, so do not move `ocr-rust.exe` out on its own. What you must have side by side:

```
ocr-rust.exe
onnxruntime.dll
onnxruntime_providers_shared.dll
DirectML.dll
libomp140.x86_64.dll
models\PP-OCRv6_small_det_onnx\
models\PP-OCRv6_small_rec_onnx\
models\PP-OCRv6_tiny_det_onnx_infer\
models\PP-OCRv6_tiny_rec_onnx_infer\
static\
```

### Start the service

```powershell
.\ocr-rust.exe
```

- Listens on **18004** by default; override with the `OCR_PORT` environment variable.
- Binds `0.0.0.0`, so other machines on the LAN can reach it.
- Preloads the `small` model at startup; `tiny` lazy-loads on first request.

```powershell
$env:OCR_PORT = 18080
.\ocr-rust.exe
```

Stop it with `Ctrl+C` or `Stop-Process -Name ocr-rust`.

### Browser page

The executable serves a bundled comparison UI at **`http://127.0.0.1:18004/`** — open it in
a browser, drop in an image, pick `small` or `tiny`, and it runs both the Rust server
and an in-browser PaddleOCR.js side by side, showing latency, line count, mean
confidence and the recognised text from each. It also opens a browser window on launch.

The page needs the `static/` folder that ships inside the archive; **do not delete it**
or move `ocr-rust.exe` away from it, or `GET /` returns 404 (every `/ocr*` endpoint keeps
working regardless, so scripted use is unaffected).

### HTTP endpoints

| Method | Path | Notes |
|---|---|---|
| `GET` | `/health` | `{"status":"ok","version":"0.1.0"}` |
| `GET` | `/` | bundled comparison UI |
| `POST` | `/ocr` | `multipart/form-data`, first non-empty field wins |
| `POST` | `/ocr/raw` | raw image bytes in the body |
| `POST` | `/ocr/base64` | JSON `{"image":"<standard base64>"}` |

All three POST routes accept `?model=small|tiny` (`small` is the default and is more
accurate but slower; `tiny` is a good deal faster and fine for clean text). Request bodies
are capped at 10 MB; concurrency is capped at `max(cores/2, 2)`, with extra requests queued.

Response shape:

```jsonc
{"success": true,
 "text": "Hello OCR World\npdf-cli test 12345",
 "regions": [{"text": "Hello OCR World", "confidence": 0.986,
              "bounding_box": [[105.0,719.0],[602.0,719.0],[602.0,771.0],[105.0,771.0]]}],
 "processing_time_ms": 50.7, "model": "tiny"}
```

`text` is all lines joined by `\n` (`""` when nothing is found); `regions[].bounding_box`
is four `[x, y]` pixel corners with the origin at the top-left (`null` when fewer than four
points). Regions without text are dropped, so every entry has a non-empty `text`.

Errors are `{"success": false, "error": "..."}`: `400` for a bad request (empty body,
missing `image`, undecodable base64), `500` when the image itself cannot be processed.

### Use it from the CLI

Rendering a scan and posting the pages is the whole loop:

```powershell
pdf-cli pdf-to-image --input scan.pdf --output-dir ./pages --format png --width 2400 --pdfium-path "$PWD\pdfium.dll"

# one page, raw bytes  (fastest path, no base64 overhead)
curl.exe -s -X POST "http://127.0.0.1:18004/ocr/raw?model=small" -H "Content-Type: application/octet-stream" --data-binary "@./pages/scan_001.png"

# multipart  (this is the form shape the web UI uses)
curl.exe -s -X POST "http://127.0.0.1:18004/ocr?model=tiny" -F "file=@./pages/scan_001.png"
```

```powershell
# batch: every page, base64 through PowerShell, one text file out
$out = foreach ($png in Get-ChildItem ./pages/*.png | Sort-Object Name) {
  $b64  = [Convert]::ToBase64String([IO.File]::ReadAllBytes($png.FullName))
  $body = @{ image = $b64 } | ConvertTo-Json -Compress
  $r    = Invoke-RestMethod "http://127.0.0.1:18004/ocr/base64?model=small" `
            -Method Post -ContentType "application/json" -Body $body
  "--- $($png.Name) ---"; $r.text
}
$out | Set-Content ./ocr.txt -Encoding utf8
pdf-cli convert --input ./ocr.txt --output searchable.pdf
```

Two details worth knowing:

- **`Invoke-WebRequest -InFile` with `-ContentType "multipart/form-data"` fails with 400** —
  PowerShell does not build a real multipart body. Either use `curl.exe -F` (as above) or
  post raw bytes / base64. The `400` is the client's fault, not the service's.
- **You get a rebuilt text PDF, not the original scan.** The OCR service returns plain
  text, so `convert` produces a fresh PDF containing that text as real, selectable text —
  `extract` and `search` read it back fine, but the original page images and layout are
  gone. If you need the scan's appearance kept, OCR the pages for text and keep the
  `pdf-to-image` output as the visual side; assembling the two into one invisible-text-
  over-image PDF is not something pdf-cli does.

### Health check before use

A failure to start is silent from the client side, so probe before a batch:

```powershell
try   { (Invoke-RestMethod "http://127.0.0.1:18004/health").status }  # "ok"
catch { "OCR service is not running - start ocr-rust.exe first" }
```

Never let a missing OCR service turn into "the document is empty": that is exactly the
failure mode `extract` refuses to produce.

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
# pdf-cli has no OCR. Render the pages, hand them to the local OCR service, then re-import.
pdf-cli pdf-to-image --input scan.pdf --output-dir ./pages --width 2400 --json
#  -> POST each page to http://127.0.0.1:18004/ocr/raw (see the OCR section)
pdf-cli convert --input ocr.txt --output recognised.pdf --json
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

User: "这份手册里哪一页提到退款政策"
```powershell
pdf-cli search --input manual.pdf --pattern "退款|refund" --regex --json
```

User: "把这个PDF转成我能读的markdown"
```powershell
pdf-cli to-markdown --input report.pdf --mode doc --output report.md --pdfium-path <dir>\pdfium.dll --json
```
Use `--mode doc` for structure (headings, lists, tables); `--mode plain` gives one
paragraph per text line with no heuristics. This command needs `pdfium.dll` even for
`plain`.

User: "上周那版和这版合同有什么不同"
```powershell
pdf-cli diff --input old.pdf --input2 new.pdf --json
```
Read `details.different`, **not** the exit code: `diff` exits 0 whenever both files
are readable, so a caller that branches on `$LASTEXITCODE` silently treats every
change as "no change".

User: "把这个表单的姓名栏填上张三"
```powershell
pdf-cli form --input form.pdf --action list --json
pdf-cli form --input form.pdf --action set --values "name=张三" --output filled.pdf --json
```

User: "把这一份合成长报告，顺便把书签也带上"
```powershell
pdf-cli merge --input "cover.pdf,ch1.pdf,ch2.pdf" --output report.pdf --json
```

User: "这个扫描件识别完还要能搜索"
The bundled OCR service returns plain text, not a searchable PDF, so rebuild one with
`convert` and search that. If you use an OCR tool that does emit a searchable PDF
(image + invisible text layer), read it back directly - `extract` and `search` work on it:
```powershell
pdf-cli search --input recognised.pdf --pattern "invoice total" --json
```
pdf-cli cannot add that text layer itself.

User: "每10页拆一份"
```powershell
pdf-cli split --input doc.pdf --every 10 --output-dir ./parts --json
```

User: "这个中文PDF转成文本但中文都变方框了"
```powershell
pdf-cli extract --input doc.pdf --output doc.txt --json
```
`extract` reads the text layer straight out of the PDF, so it never re-renders CJK
characters and cannot produce boxes. Boxes mean the *source* PDF has no usable
`/ToUnicode` map: the glyphs are there, but the mapping back to characters is not.
pdf-cli cannot recover a missing map. Run `doctor` to see the font list, or treat the
file as an image (`pdf-to-image`). Note that `convert` cannot help here - it writes
text *into* PDFs and does not read PDFs at all.

## Safety

- `--dry-run` available for all mutating commands.
- Output files are overwritten silently; use distinct output paths.

## Limitations

- **No OCR.** pdf-cli never recognises text from images. It says so explicitly (empty text
  layer -> `OPERATION_FAILED`) rather than returning empty output. Pair it with the local
  OCR service documented above (`ocr-rust.zip`) when you hit a scan.
- `pdf-to-image` requires `pdfium.dll` (not bundled). `search`, `form`, `diff` and
  `to-markdown` use it when present and degrade gracefully without it — except `form`,
  which needs it outright.
- `web-to-pdf` requires Microsoft Edge.
- **`watermark-remove`** only removes **burned-in image** watermarks on raster/scanned pages;
  it does **not** remove vector or text-layer watermarks.
- **Digital signature / PDF→Word / PDF→PPT / PDF→Excel**: not supported (see ROADMAP.md).
  Removing watermarks from a text layer is likewise not attempted.
- **SVG → PDF** goes through headless Edge.
- `merge` preserves bookmark titles, hierarchy, ordering and destinations, but normalises
  every destination to `[page /Fit]` and drops `/F`, `/C` styling keys. Non-GoTo actions
  (`/URI`, `/Launch`, `/GoToR`, JavaScript) keep their title but fall back to the file's
  first page.
- `convert --font` embeds one subset per document; text mixing scripts that no single
  font covers must be split into separate conversions.

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
