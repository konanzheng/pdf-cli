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

Write-Host "`n== info / doctor =="
$r = RJ @('info','--input',$c2)
Check "info pages" ($r.details.pages -eq 2) "pages=$($r.details.pages)"
$r = RJ @('doctor','--input',$c2)
Check "doctor images" ($r.details.total_images -eq 2) "imgs=$($r.details.total_images)"

Write-Host "`n== merge / split =="
$r = RJ @('merge','--input',"$c1,$c1",'--output',(Join-Path $work "m.pdf"))
Check "merge" ($r.details.total_pages -eq 2) "pages=$($r.details.total_pages)"
$r = RJ @('split','--input',$c2,'--output-dir',(Join-Path $work "split"))
$n = @(Get-ChildItem (Join-Path $work "split\*.pdf") -ErrorAction SilentlyContinue).Count
Check "split" ($n -eq 2) "files=$n"

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
