# pdf-cli smoke test — self-contained end-to-end check of every command.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\smoke_test.ps1
#   powershell -ExecutionPolicy Bypass -File tests\smoke_test.ps1 -Exe path\to\pdf-cli.exe
#
# It generates its own test images (via .NET System.Drawing), so no external
# fixtures are required. Commands that need pdfium.dll / Edge are skipped
# automatically when the dependency is missing.

param(
    [string]$Exe = "",
    [string]$PdfiumPath = ""
)

$ErrorActionPreference = 'Continue'

# ---------- locate the binary ----------
function Find-Exe {
    param([string]$explicit)
    if ($explicit -and (Test-Path $explicit)) { return (Resolve-Path $explicit).Path }
    $candidates = @(
        "$PSScriptRoot\..\pdf-cli.exe",
        "$PSScriptRoot\..\target\release\pdf-cli.exe",
        "$PSScriptRoot\..\target\debug\pdf-cli.exe",
        "F:\rust\build-target-all\release\pdf-cli.exe",
        "F:\rust\build-target-all\debug\pdf-cli.exe"
    )
    foreach ($c in $candidates) { if (Test-Path $c) { return (Resolve-Path $c).Path } }
    return $null
}

$dbg = Find-Exe $Exe
if (-not $dbg) {
    Write-Host "ERROR: pdf-cli.exe not found. Build it first (cargo build) or pass -Exe <path>." -ForegroundColor Red
    exit 2
}
Write-Host "Using binary: $dbg"

# ---------- work dir ----------
$work = Join-Path $env:TEMP ("pdf-cli-smoke-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force -Path $work | Out-Null

# ---------- generate test images ----------
Add-Type -AssemblyName System.Drawing
function New-TestImage([string]$path, [string]$title) {
    $bmp = New-Object System.Drawing.Bitmap(800, 1100)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::White)
    $fBig = New-Object System.Drawing.Font("Arial", 36)
    $fSm = New-Object System.Drawing.Font("Arial", 20)
    $g.DrawString($title, $fBig, [System.Drawing.Brushes]::Black, 80, 140)
    $g.DrawString("Body text line one", $fSm, [System.Drawing.Brushes]::Black, 80, 240)
    $g.DrawString("Body text line two", $fSm, [System.Drawing.Brushes]::Black, 80, 280)
    $g.DrawRectangle([System.Drawing.Pens]::Black, 80, 340, 640, 300)
    $g.Dispose(); $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
}
$img1 = Join-Path $work "page1.png"
$img2 = Join-Path $work "page2.png"
New-TestImage $img1 "TEST DOCUMENT ONE"
New-TestImage $img2 "TEST DOCUMENT TWO"

# ---------- pdfium detection ----------
if (-not $PdfiumPath) {
    foreach ($c in @(
        "$PSScriptRoot\..\testdata\pdfium\bin\pdfium.dll",
        "$PSScriptRoot\..\pdfium.dll"
    )) { if (Test-Path $c) { $PdfiumPath = (Resolve-Path $c).Path; break } }
}
$havePdfium = [bool]($PdfiumPath -and (Test-Path $PdfiumPath))
if ($havePdfium) { Write-Host "pdfium: $PdfiumPath" } else { Write-Host "pdfium: not found (pdf-to-image will be skipped)" -ForegroundColor Yellow }

# ---------- helpers ----------
$script:pass = 0
$script:fail = 0
$script:skip = 0
function Check($name, $cond, $extra) {
    if ($cond) { Write-Host ("  [PASS] $name  $extra") -ForegroundColor Green; $script:pass++ }
    else       { Write-Host ("  [FAIL] $name  $extra") -ForegroundColor Red;   $script:fail++ }
}
function Skip($name, $why) {
    Write-Host ("  [SKIP] $name  ($why)") -ForegroundColor DarkYellow; $script:skip++
}
function RJ([string[]]$cmd) {
    $out = & $dbg @cmd --json 2>$null | Out-String
    try { return $out | ConvertFrom-Json } catch { return $null }
}

$c1 = Join-Path $work "c1.pdf"
$c2 = Join-Path $work "c2.pdf"

Write-Host "`n== convert =="
$r = RJ @('convert','--input',$img1,'--output',$c1)
Check "convert single" ($r.success -and $r.details.pages_created -eq 1) "pages=$($r.details.pages_created)"
$r = RJ @('convert','--input',"$img1,$img2",'--output',$c2)
Check "convert multi" ($r.success -and $r.details.pages_created -eq 2) "pages=$($r.details.pages_created)"

# --- text input, CJK font embedding, BOM handling ---
$noBom = New-Object System.Text.UTF8Encoding($false)
$withBom = New-Object System.Text.UTF8Encoding($true)
$asciiTxt = Join-Path $work "ascii.txt"
[System.IO.File]::WriteAllText($asciiTxt, "Plain ASCII text.`r`nSecond line.", $noBom)
$r = RJ @('convert','--input',$asciiTxt,'--output',(Join-Path $work "ascii.pdf"))
Check "convert ASCII needs no --font" ($r.success -and $r.details.font_embedded -eq $false) "font_embedded=$($r.details.font_embedded)"

$bomTxt = Join-Path $work "bom.txt"
[System.IO.File]::WriteAllText($bomTxt, "BOM-prefixed ASCII text.", $withBom)
$r = RJ @('convert','--input',$bomTxt,'--output',(Join-Path $work "bom.pdf"))
Check "convert strips a UTF-8 BOM instead of rejecting it" ($r.success -and $r.details.bom_stripped -eq $true) "bom_stripped=$($r.details.bom_stripped)"

$cjkTxt = Join-Path $work "cjk.txt"
[System.IO.File]::WriteAllText($cjkTxt, "你好，世界！`r`n这是一个中文测试。", $noBom)
# Non-ASCII without a font cannot be rendered, so it must fail loudly rather than
# silently emit a document full of blanks.
$r = RJ @('convert','--input',$cjkTxt,'--output',(Join-Path $work "cjk_nofont.pdf"))
Check "convert CJK without --font fails clearly" ($r.error) "code=$($r.code)"

$cjkFont = "C:\Windows\Fonts\simhei.ttf"
if (Test-Path $cjkFont) {
    $cjkPdf = Join-Path $work "cjk.pdf"
    $r = RJ @('convert','--input',$cjkTxt,'--output',$cjkPdf,'--font',$cjkFont)
    Check "convert CJK with --font embeds it" ($r.success -and $r.details.font_embedded -eq $true -and $r.details.glyphs_subset -gt 0) "glyphs=$($r.details.glyphs_subset)"
    # The real acceptance test: the characters must survive a round-trip.
    $back = (& $dbg extract --input $cjkPdf 2>$null | Out-String)
    Check "convert CJK round-trips through extract" ($back -match '你好，世界') "got=$($back.Trim())"
    $s = RJ @('search','--input',$cjkPdf,'--pattern','你好')
    Check "convert CJK is searchable" ($s.success -and $s.details.total_matches -ge 1) "matches=$($s.details.total_matches)"
} else {
    Skip "convert CJK with --font" "simhei.ttf not present"
}

Write-Host "`n== info / doctor =="
$r = RJ @('info','--input',$c2)
Check "info pages" ($r.details.pages -eq 2) "pages=$($r.details.pages)"
$r = RJ @('doctor','--input',$c2)
Check "doctor images" ($r.details.total_images -eq 2) "imgs=$($r.details.total_images)"

Write-Host "`n== merge / split =="
# A five-page document for the grouping strategies: repeat one page five times.
$fivePdf = Join-Path $work "five.pdf"
& $dbg organize --input $c1 --order "1,1,1,1,1" --output $fivePdf --quiet 2>$null | Out-Null

$r = RJ @('merge','--input',"$c1,$c1",'--output',(Join-Path $work "m.pdf"))
Check "merge" ($r.details.total_pages -eq 2) "pages=$($r.details.total_pages)"
$r = RJ @('split','--input',$c2,'--output-dir',(Join-Path $work "split"))
$n = @(Get-ChildItem (Join-Path $work "split\*.pdf") -ErrorAction SilentlyContinue).Count
Check "split" ($n -eq 2) "files=$n"

# split variants: --every groups by count, --odd-even separates the two sides.
$evDir = Join-Path $work "split_every"
$r = RJ (@('split','--input',$fivePdf,'--every','2','--output-dir',$evDir))
$n = @(Get-ChildItem "$evDir\*.pdf" -ErrorAction SilentlyContinue).Count
Check "split --every 2 on 5 pages -> 3 files" ($r.success -and $n -eq 3) "files=$n"
Check "split --every reports strategy" ($r.details.strategy -eq 'every') "strategy=$($r.details.strategy)"

$oeDir = Join-Path $work "split_oddeven"
$r = RJ (@('split','--input',$fivePdf,'--odd-even','--output-dir',$oeDir))
$n = @(Get-ChildItem "$oeDir\*.pdf" -ErrorAction SilentlyContinue).Count
Check "split --odd-even -> 2 files (odd, even)" ($r.success -and $n -eq 2) "files=$n"

# Two strategies at once is ambiguous, so it must be refused rather than guessed.
$r = RJ (@('split','--input',$fivePdf,'--every','2','--odd-even','--output-dir',(Join-Path $work "split_both")))
Check "split with two strategies -> clear error" ($r.error -or -not $r.success) "ok"

# merge must carry the outline across, remapped onto the merged page numbers.
# `testdata\outline.pdf` is a two-page document with two bookmarks; merging two
# copies should produce one parent per file, each holding the original two.
$outlinePdf = Join-Path $PSScriptRoot "..\testdata\outline.pdf"
if (Test-Path $outlinePdf) {
    $r = RJ (@('merge','--input',"$outlinePdf,$outlinePdf",'--output',(Join-Path $work "bm.pdf")))
    Check "merge migrates an outline" ($r.success -and $r.details.outline_merged -eq $true) "outline_merged=$($r.details.outline_merged)"
    Check "merge outline keeps every bookmark plus a parent per file" ($r.details.outline_items -eq 6) "items=$($r.details.outline_items)"
    # A document with no outline must not gain an empty /Outlines key.
    $r = RJ (@('merge','--input',"$c1,$c2",'--output',(Join-Path $work "noout.pdf")))
    Check "merge of outline-free inputs reports no outline" ($r.success -and $r.details.outline_merged -eq $false) "outline_merged=$($r.details.outline_merged)"
} else {
    Skip "merge migrates an outline" "testdata\outline.pdf missing"
}

Write-Host "`n== delete-pages / rotate / pagenum =="
$r = RJ @('delete-pages','--input',$c2,'--pages','1','--output',(Join-Path $work "del.pdf"))
Check "delete-pages" ($r.details.remaining -eq 1) "remaining=$($r.details.remaining)"
$r = RJ @('rotate','--input',$c1,'--angle','90','--output',(Join-Path $work "rot.pdf"))
Check "rotate" ($r.success) "ok"
$r = RJ @('pagenum','--input',$c2,'--output',(Join-Path $work "pn.pdf"))
Check "pagenum" ($r.success) "ok"

Write-Host "`n== meta =="
$metaOut = Join-Path $work "meta.pdf"
$r = RJ @('meta','--input',$c1,'--output',$metaOut,'--title','T','--author','A')
$r2 = RJ @('info','--input',$metaOut)
Check "meta title roundtrip" ($r2.details.metadata.title -eq 'T') "title=$($r2.details.metadata.title)"

Write-Host "`n== watermark-add / watermark-remove =="
$wm = Join-Path $work "wm.pdf"
$r = RJ @('watermark-add','--input',$c1,'--text','WM','--output',$wm)
$r2 = RJ @('doctor','--input',$wm)
Check "watermark-add keeps image" ($r2.details.total_images -eq 1) "imgs=$($r2.details.total_images)"

# closed loop: burn the vector watermark into pixels, then remove it
if ($havePdfium) {
    $raster = Join-Path $work "raster"
    & $dbg pdf-to-image --input $wm --output-dir $raster --format png --width 800 --pdfium-path $PdfiumPath --quiet 2>$null | Out-Null
    $rast = Get-ChildItem "$raster\*.png" | Select-Object -First 1
    $burned = Join-Path $work "burned.pdf"
    & $dbg convert --input $rast.FullName --output $burned --quiet 2>$null | Out-Null
    $wmr = Join-Path $work "wmr.pdf"
    $r = RJ @('watermark-remove','--input',$burned,'--output',$wmr,'--threshold','212')
    Check "watermark-remove processes image" ($r.success -and $r.details.images_processed -ge 1) "processed=$($r.details.images_processed)"
    $r2 = RJ @('doctor','--input',$wmr)
    Check "watermark-remove keeps image" ($r2.details.total_images -eq 1) "imgs=$($r2.details.total_images)"
} else {
    Skip "watermark-remove closed loop" "pdfium.dll not found"
}

Write-Host "`n== pdf-to-image / extract-images =="
if ($havePdfium) {
    $r = RJ @('pdf-to-image','--input',$c1,'--output-dir',(Join-Path $work "imgs"),'--format','png','--pdfium-path',$PdfiumPath)
    Check "pdf-to-image" ($r.details.page_count -eq 1) "count=$($r.details.page_count)"
} else {
    Skip "pdf-to-image" "pdfium.dll not found"
}
$r = RJ @('extract-images','--input',$c1,'--output-dir',(Join-Path $work "xi"))
Check "extract-images" ($r.details.image_count -ge 1) "count=$($r.details.image_count)"

Write-Host "`n== extract text =="
# The generated images have no text layer, so build a real text PDF for this check.
$txtSrc = Join-Path $work "text.txt"
Set-Content -Path $txtSrc -Value "Smoke test document`nSecond line for extraction." -Encoding UTF8
$txtPdf = Join-Path $work "text.pdf"
& $dbg convert --input $txtSrc --output $txtPdf --quiet 2>$null | Out-Null
$r = RJ @('extract','--input',$txtPdf)
Check "extract runs" ($r.success -and $r.details.char_count -gt 0) "chars=$($r.details.char_count)"
$r = RJ @('extract','--input',$c1)
Check "extract rejects image-only PDF" ($r.error -and $r.code -eq 'OPERATION_FAILED') "code=$($r.code)"

Write-Host "`n== encrypt / decrypt =="
$enc = Join-Path $work "enc.pdf"
$r = RJ @('encrypt','--input',$c1,'--user-password','pw','--output',$enc)
Check "encrypt" ($r.success) "ok"
$r = RJ @('decrypt','--input',$enc,'--password','pw','--output',(Join-Path $work "dec.pdf"))
Check "decrypt" ($r.success) "ok"

Write-Host "`n== compress / invoice =="
$r = RJ @('compress','--input',$c1,'--output',(Join-Path $work "cmp.pdf"))
Check "compress" ($r.success) "ok"
$r = RJ @('invoice','--input',"$img1,$img2",'--output',(Join-Path $work "inv.pdf"))
Check "invoice" ($r.success) "ok"

Write-Host "`n== dry-run / errors =="
$noWrite = Join-Path $work "no_write.pdf"
$r = RJ @('delete-pages','--input',$c2,'--pages','1','--output',$noWrite,'--dry-run')
Check "dry-run no write" (-not (Test-Path $noWrite)) "dry_run=$($r.details.dry_run)"
$r = RJ @('info','--input',(Join-Path $work "nope.pdf"))
Check "missing file -> FILE_NOT_FOUND" ($r.error -and $r.code -eq 'FILE_NOT_FOUND') "code=$($r.code)"
$r = RJ @('rotate','--input',$c1,'--angle','45','--output',(Join-Path $work "bad.pdf"))
Check "bad angle rejected" ($r.error) "code=$($r.code)"
$r = RJ @('delete-pages','--input',$c1,'--pages','1','--output',(Join-Path $work "bad.pdf"))
Check "delete-all rejected" ($r.error) "code=$($r.code)"

Write-Host "`n== search / to-markdown / diff / form / crop / organize =="
$PdfiumArg = if ($havePdfium) { @('--pdfium-path', $PdfiumPath) } else { @() }

# search: use the text PDF, since the image fixtures carry no text layer.
$r = RJ (@('search','--input',$txtPdf,'--pattern','Smoke') + $PdfiumArg)
Check "search finds text" ($r.success -and $r.details.total_matches -ge 1) "matches=$($r.details.total_matches)"
$r = RJ (@('search','--input',$txtPdf,'--pattern','zzz_no_such_text_zzz') + $PdfiumArg)
Check "search no-match still succeeds" ($r.success -and $r.details.total_matches -eq 0) "matches=$($r.details.total_matches)"
if ($havePdfium) {
    $r = RJ (@('search','--input',$txtPdf,'--pattern','Smoke') + $PdfiumArg)
    Check "search reports geometry" ($r.details.geometry_available -eq $true) "geo=$($r.details.geometry_available)"
}

# to-markdown: must produce a non-empty document.
$mdOut = Join-Path $work "out.md"
$r = RJ (@('to-markdown','--input',$txtPdf,'--output',$mdOut) + $PdfiumArg)
Check "to-markdown writes file" ($r.success -and (Test-Path $mdOut) -and (Get-Item $mdOut).Length -gt 0) "len=$((Get-Item $mdOut -ErrorAction SilentlyContinue).Length)"

# diff: identical files must report no difference.
$r = RJ (@('diff','--input',$txtPdf,'--input2',$txtPdf,'--mode','text') + $PdfiumArg)
Check "diff identical -> not different" ($r.success -and $r.details.different -eq $false) "different=$($r.details.different)"
$otherSrc = Join-Path $work "other.txt"
Set-Content -Path $otherSrc -Value "Completely unrelated content." -Encoding UTF8
$otherPdf = Join-Path $work "other.pdf"
& $dbg convert --input $otherSrc --output $otherPdf --quiet 2>$null | Out-Null
$r = RJ (@('diff','--input',$txtPdf,'--input2',$otherPdf,'--mode','text') + $PdfiumArg)
Check "diff altered -> different" ($r.success -and $r.details.different -eq $true) "different=$($r.details.different)"

# form: a PDF with no form is a success with has_form=false, not an error.
$r = RJ (@('form','--input',$txtPdf,'--action','list') + $PdfiumArg)
Check "form on non-form is success" ($r.success -and $r.details.has_form -eq $false) "has_form=$($r.details.has_form)"

# crop: explicit box, then confirm the page count survives.
$cropOut = Join-Path $work "crop.pdf"
$r = RJ @('crop','--input',$c1,'--box','0,0,200,200','--output',$cropOut)
Check "crop with --box" ($r.success -and (Test-Path $cropOut)) "affected=$($r.details.pages_affected)"

# organize: reverse a two-page document and confirm the order is reported.
$orgOut = Join-Path $work "org.pdf"
$r = RJ @('organize','--input',$c2,'--order','2,1','--output',$orgOut)
Check "organize reorder" ($r.success -and $r.details.pages_out -eq 2) "pages_out=$($r.details.pages_out)"
$r = RJ @('info','--input',$orgOut)
Check "organize output is valid" ($r.success) "ok"

# ---------- redact / overlay ----------
Write-Host "`n== redact / overlay =="
# overlay works without pdfium: it takes coordinates, it does not measure them.
$ovOut = Join-Path $work "overlay.pdf"
$r = RJ @('overlay','--input',$txtPdf,'--output',$ovOut,'--text','1:x=72,y=700,text=OVERLAYMARK,size=12')
Check "overlay draws text" ($r.success -and (Test-Path $ovOut) -and $r.details.items -eq 1) "items=$($r.details.items)"
$r = RJ @('extract','--input',$ovOut)
Check "overlay text is extractable" ($r.output -match 'OVERLAYMARK') "has_mark=$($r.output -match 'OVERLAYMARK')"
$r = RJ @('overlay','--input',$txtPdf,'--output',(Join-Path $work "ov2.pdf"),'--text','1:x=72,y=600,text=BOX,w=80,h=20,cover=1/1/1')
Check "overlay cover box + text" ($r.success) "ok"
$r = RJ @('overlay','--input',$txtPdf,'--output',(Join-Path $work "ov3.pdf"),'--text','1:x=72,y=600,nope=1')
Check "overlay rejects unknown key" ($r.error -and $r.code -eq 'OPERATION_FAILED') "code=$($r.code)"
$r = RJ @('overlay','--input',$txtPdf,'--output',(Join-Path $work "ov4.pdf"),'--text','1:y=600,text=X')
Check "overlay requires x and y" ($r.error -and $r.code -eq 'OPERATION_FAILED') "code=$($r.code)"

# redact needs a measurable rectangle, so it is pdfium-only.
if ($havePdfium) {
    $rdOut = Join-Path $work "redact.pdf"
    $r = RJ (@('redact','--input',$txtPdf,'--output',$rdOut,'--find','Smoke','--replace','ReplacedMk') + $PdfiumArg)
    Check "redact replaces text" ($r.success -and (Test-Path $rdOut) -and $r.details.occurrences -ge 1) "occ=$($r.details.occurrences)"
    $r = RJ @('extract','--input',$rdOut)
    Check "redact replacement is extractable" ($r.output -match 'ReplacedMk') "has_mk=$($r.output -match 'ReplacedMk')"
    $r = RJ (@('redact','--input',$txtPdf,'--output',(Join-Path $work "rd2.pdf"),'--find','Smoke','--replace','X') + $PdfiumArg + @('--dry-run'))
    Check "redact --dry-run writes nothing" ($r.success -and $r.details.dry_run -eq $true -and -not (Test-Path (Join-Path $work "rd2.pdf"))) "dry=$($r.details.dry_run)"
    $r = RJ (@('redact','--input',$txtPdf,'--output',(Join-Path $work "rd3.pdf"),'--find','zz_no_such_zz','--replace','X') + $PdfiumArg)
    Check "redact no-match writes nothing" ($r.success -and $r.details.written -eq $false) "written=$($r.details.written)"
} else {
    $r = RJ @('redact','--input',$txtPdf,'--output',(Join-Path $work "rd0.pdf"),'--find','Smoke','--replace','X')
    Check "redact without pdfium fails clearly" ($r.error -and $r.code -eq 'OPERATION_FAILED') "code=$($r.code)"
}

Write-Host "`n== global password / stdin =="
$r = RJ @('info','--input',$enc)
Check "encrypted without --password -> PASSWORD_REQUIRED" ($r.error -and $r.code -eq 'PASSWORD_REQUIRED') "code=$($r.code)"
$r = RJ @('--password','pw','info','--input',$enc)
Check "global --password unlocks" ($r.success) "ok"
$r = RJ @('--password','wrong','info','--input',$enc)
Check "wrong password -> WRONG_PASSWORD" ($r.error -and $r.code -eq 'WRONG_PASSWORD') "code=$($r.code)"

Write-Host ""
$color = if ($script:fail -eq 0) { "Green" } else { "Red" }
Write-Host ("TOTAL: PASS=$($script:pass) FAIL=$($script:fail) SKIP=$($script:skip)") -ForegroundColor $color
Write-Host "work dir: $work"
if ($script:fail -eq 0) { exit 0 } else { exit 1 }
