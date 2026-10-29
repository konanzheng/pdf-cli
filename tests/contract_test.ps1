# pdf-cli contract test — verifies the machine-facing guarantees that every
# command must uphold, independent of what any single command does.
#
# The smoke test answers "does this command work?"; this file answers "can an
# agent rely on the output shape and exit codes?" — the two questions are
# different, and the second one is what makes the tool scriptable.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\contract_test.ps1
#   powershell -ExecutionPolicy Bypass -File tests\contract_test.ps1 -Exe path\to\pdf-cli.exe

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
        "F:\rust\build-target-all\release\pdf-cli.exe",
        "F:\rust\build-target-all\debug\pdf-cli.exe"
    )
    foreach ($c in $candidates) { if (Test-Path $c) { return (Resolve-Path $c).Path } }
    return $null
}

$exe = Find-Exe $Exe
if (-not $exe) {
    Write-Host "ERROR: pdf-cli.exe not found. Build it first (cargo build) or pass -Exe <path>." -ForegroundColor Red
    exit 2
}
Write-Host "Using binary: $exe"

# ---------- work dir ----------
$work = Join-Path $env:TEMP ("pdf-cli-contract-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Force -Path $work | Out-Null

# ---------- pdfium detection ----------
if (-not $PdfiumPath) {
    foreach ($c in @("$PSScriptRoot\..\testdata\pdfium\bin\pdfium.dll", "$PSScriptRoot\..\pdfium.dll")) {
        if (Test-Path $c) { $PdfiumPath = (Resolve-Path $c).Path; break }
    }
}
$havePdfium = [bool]($PdfiumPath -and (Test-Path $PdfiumPath))

# ---------- harness ----------
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

# Run a command, capturing stdout/stderr/exit code separately. The whole point
# of this file is to assert on those three channels independently.
function Run {
    param([string[]]$cmd, [string]$stdin = $null)
    $outFile = Join-Path $work "stdout.tmp"
    $errFile = Join-Path $work "stderr.tmp"
    if ($null -ne $stdin) {
        $stdin | & $exe @cmd 1>$outFile 2>$errFile
    } else {
        & $exe @cmd 1>$outFile 2>$errFile
    }
    $code = $LASTEXITCODE
    $stdout = if (Test-Path $outFile) { Get-Content $outFile -Raw -Encoding UTF8 } else { "" }
    $stderr = if (Test-Path $errFile) { Get-Content $errFile -Raw -Encoding UTF8 } else { "" }
    return [pscustomobject]@{ code = $code; stdout = $stdout; stderr = $stderr }
}

function AsJson($text) {
    if ([string]::IsNullOrWhiteSpace($text)) { return $null }
    try { return $text | ConvertFrom-Json } catch { return $null }
}

# ---------- fixtures ----------
$srcTxt = Join-Path $work "src.txt"
[System.IO.File]::WriteAllText($srcTxt, "Contract test fixture line one.`r`nSecond line.`r`n", (New-Object System.Text.UTF8Encoding($false)))
$pdf = Join-Path $work "one.pdf"
& $exe convert --input $srcTxt --output $pdf --quiet 2>$null | Out-Null
if (-not (Test-Path $pdf)) {
    # Surface the real reason instead of a bare "could not build a fixture".
    $why = & $exe convert --input $srcTxt --output $pdf 2>&1 | Out-String
    Write-Host "ERROR: could not build a fixture PDF. convert said:" -ForegroundColor Red
    Write-Host $why -ForegroundColor Red
    Write-Host "work dir: $work"
    exit 2
}

$multiTxt2 = Join-Path $work "src2.txt"
[System.IO.File]::WriteAllText($multiTxt2, "A different document entirely.`r`n", (New-Object System.Text.UTF8Encoding($false)))
$pdf2 = Join-Path $work "two.pdf"
& $exe convert --input $multiTxt2 --output $pdf2 --quiet 2>$null | Out-Null

Write-Host "`n== exit codes: 0 on success, 1 on error =="
$r = Run @('info','--input',$pdf,'--json')
Check "success exits 0" ($r.code -eq 0) "code=$($r.code)"
$r = Run @('info','--input',(Join-Path $work "nope.pdf"),'--json')
Check "failure exits 1" ($r.code -eq 1) "code=$($r.code)"
$r = Run @('info','--input',$pdf,'--bogus-flag','--json')
Check "bad flag exits non-zero" ($r.code -ne 0) "code=$($r.code)"

Write-Host "`n== --json envelope shape =="
$r = Run @('info','--input',$pdf,'--json')
$j = AsJson $r.stdout
Check "success envelope has success/command/output/details" `
    ($j -and $j.success -eq $true -and $j.command -eq 'info' -and $null -ne $j.output -and $null -ne $j.details) `
    "keys=$(if ($j) { ($j.PSObject.Properties.Name -join ',') } else { 'none' })"
Check "success envelope has no top-level error key" ($j -and -not ($j.PSObject.Properties.Name -contains 'error')) "ok"

$r = Run @('info','--input',(Join-Path $work "nope.pdf"),'--json')
$j = AsJson $r.stdout
Check "error envelope has error/message/code" `
    ($j -and $j.error -eq $true -and $j.code -and $j.message) `
    "code=$($j.code)"
Check "error envelope carries a hint field" ($j -and ($j.PSObject.Properties.Name -contains 'hint')) "present"
Check "error JSON goes to stdout, not stderr" ((-not [string]::IsNullOrWhiteSpace($r.stdout)) -and [string]::IsNullOrWhiteSpace($r.stderr)) "stdout=$(($r.stdout).Length) stderr=$(($r.stderr).Length)"

Write-Host "`n== error codes are stable identifiers =="
# Each case: args, expected code. Adding a command means adding it here.
$codeCases = @(
    @{ name = 'missing file';        args = @('info','--input',"$work\ghost.pdf");          code = 'FILE_NOT_FOUND' },
    @{ name = 'bad page range';      args = @('extract','--input',$pdf,'--pages','99');      code = 'INVALID_PAGE_RANGE' },
    @{ name = 'page zero';           args = @('extract','--input',$pdf,'--pages','0');       code = 'INVALID_PAGE_RANGE' },
    @{ name = 'inverted range';      args = @('extract','--input',$pdf,'--pages','3-1');     code = 'INVALID_PAGE_RANGE' },
    @{ name = 'non-numeric range';   args = @('extract','--input',$pdf,'--pages','a-b');     code = 'INVALID_PAGE_RANGE' }
)
foreach ($c in $codeCases) {
    $r = Run ($c.args + @('--json'))
    $j = AsJson $r.stdout
    Check "$($c.name) -> $($c.code)" ($j -and $j.code -eq $c.code) "code=$($j.code)"
}

Write-Host "`n== page range grammar =="
$r = Run @('extract','--input',$pdf,'--pages','1','--json')
Check "single page '1'" ($r.code -eq 0) "code=$($r.code)"
$r = Run @('extract','--input',$pdf,'--pages','1-1','--json')
Check "explicit range '1-1'" ($r.code -eq 0) "code=$($r.code)"
$r = Run @('extract','--input',$pdf,'--pages','1,1','--json')
Check "comma list '1,1'" ($r.code -eq 0) "code=$($r.code)"
$r = Run @('extract','--input',$pdf,'--pages','-1','--json')
Check "open start '-1' accepted as all pages" ($r.code -eq 0 -and (AsJson $r.stdout).details.page_count -eq 1) "code=$($r.code)"

Write-Host "`n== --output - writes to stdout =="
$r = Run @('extract','--input',$pdf,'--output','-')
Check "output - emits text on stdout" ($r.code -eq 0 -and -not [string]::IsNullOrWhiteSpace($r.stdout)) "len=$(($r.stdout).Length)"
# Streamed payload and the JSON envelope would land on the same stdout channel,
# so the combination must be refused rather than emit an unparseable stream.
$r = Run @('extract','--input',$pdf,'--output','-','--json')
$j = AsJson $r.stdout
Check "output - with --json is rejected, not interleaved" `
    ($r.code -eq 1 -and $j -and $j.code -eq 'OPERATION_FAILED') "code=$(if ($j) { $j.code } else { 'unparsable' })"

Write-Host "`n== --input - reads stdin =="
# Feed the PDF through the process's stdin with a real shell redirect: a
# PowerShell pipeline would decode the binary stream as text and corrupt it.
$stdoutFile = Join-Path $work "stdin_out.json"
$redirect = "`"$exe`" info --input - --json < `"$pdf`" > `"$stdoutFile`""
cmd /c $redirect 2>$null | Out-Null
$stdinText = if (Test-Path $stdoutFile) { Get-Content $stdoutFile -Raw -Encoding UTF8 } else { "" }
$sj = AsJson $stdinText
if ($sj -and $sj.success) {
    Check "stdin input works" ($sj.details.pages -eq 1) "pages=$($sj.details.pages)"
} else {
    Skip "stdin input" "could not deliver binary stdin"
}

Write-Host "`n== --dry-run writes nothing but reports fully =="
$dry = Join-Path $work "never.pdf"
$r = Run @('rotate','--input',$pdf,'--angle','90','--output',$dry,'--dry-run','--json')
$j = AsJson $r.stdout
Check "dry-run reports dry_run flag" ($j -and $j.details.dry_run -eq $true) "dry_run=$($j.details.dry_run)"
Check "dry-run creates no file" (-not (Test-Path $dry)) "exists=$(Test-Path $dry)"

Write-Host "`n== --quiet suppresses the human summary =="
$quietOut = Join-Path $work "quiet.pdf"
$r = Run @('rotate','--input',$pdf,'--angle','90','--output',$quietOut,'--quiet')
Check "quiet writes no stdout" ([string]::IsNullOrWhiteSpace($r.stdout)) "len=$(($r.stdout).Length)"
Check "quiet still performs the work" (Test-Path $quietOut) "exists=$(Test-Path $quietOut)"

Write-Host "`n== --json-lines emits one compact object per line =="
$r = Run @('info','--input',$pdf,'--json-lines')
$lines = @($r.stdout -split "`r?`n" | Where-Object { $_.Trim() -ne '' })
$oneLine = $lines.Count -eq 1
$parsedLine = if ($oneLine) { AsJson $lines[0] } else { $null }
Check "json-lines emits exactly one line per result" ($oneLine -and $parsedLine -and $parsedLine.success) "lines=$($lines.Count)"
Check "json-lines output is compact (no pretty-print newlines)" ($oneLine -and $lines[0].Trim() -match '^\{.*\}$') "ok"
$r = Run @('info','--input',(Join-Path $work "nope.pdf"),'--json-lines')
$ej = AsJson ($r.stdout -split "`r?`n" | Where-Object { $_.Trim() -ne '' } | Select-Object -First 1)
Check "json-lines errors are also single-line JSON" ($ej -and $ej.code -eq 'FILE_NOT_FOUND') "code=$(if ($ej) { $ej.code } else { 'none' })"

Write-Host "`n== global --password placement =="
$enc = Join-Path $work "enc.pdf"
& $exe encrypt --input $pdf --user-password pw --output $enc --quiet 2>$null | Out-Null
if (Test-Path $enc) {
    $r = Run @('--password','pw','info','--input',$enc,'--json')
    Check "global --password before subcommand" ($r.code -eq 0) "code=$($r.code)"
    $r = Run @('info','--input',$enc,'--password','pw','--json')
    Check "global --password after subcommand" ($r.code -eq 0) "code=$($r.code)"
    $r = Run @('info','--input',$enc,'--json')
    $j = AsJson $r.stdout
    Check "missing password -> PASSWORD_REQUIRED" ($j -and $j.code -eq 'PASSWORD_REQUIRED') "code=$($j.code)"
    $r = Run @('--password','wrong','info','--input',$enc,'--json')
    $j = AsJson $r.stdout
    Check "wrong password -> WRONG_PASSWORD" ($j -and $j.code -eq 'WRONG_PASSWORD') "code=$($j.code)"
} else {
    Skip "global --password" "could not build an encrypted fixture"
}

Write-Host "`n== input expansion: file, directory, glob =="
$r = Run @('info','--input',$pdf,'--json')
Check "plain file input" ($r.code -eq 0) "code=$($r.code)"

$inDir = Join-Path $work "many"
New-Item -ItemType Directory -Force -Path $inDir | Out-Null
Copy-Item $pdf (Join-Path $inDir "a.pdf")
Copy-Item $pdf2 (Join-Path $inDir "b.pdf")
$r = Run @('info','--input',$inDir,'--json')
$j = AsJson $r.stdout
Check "directory with several PDFs is a clear error, not 'access denied'" `
    ($j -and $j.code -eq 'OPERATION_FAILED' -and $j.message -match 'matched 2 PDF') "msg=$($j.message)"
$r = Run @('info','--input',"$inDir\a.pdf",'--json')
Check "explicit file inside a directory" ($r.code -eq 0) "code=$($r.code)"
$r = Run @('info',"--input",(Join-Path $inDir "*.pdf"),'--json')
$j = AsJson $r.stdout
Check "glob matching several -> same clear error" ($j -and $j.code -eq 'OPERATION_FAILED') "code=$($j.code)"
$r = Run @('info','--input',"$inDir\a*.pdf",'--json')
$j = AsJson $r.stdout
Check "glob matching exactly one resolves" ($r.code -eq 0) "code=$($r.code)"
Check "resolved path names the concrete file, not the pattern" `
    ($j -and $j.details.resolved -and $j.details.resolved -notmatch '\*') "resolved=$($j.details.resolved)"

# A batch-capable command consumes the same patterns instead of rejecting them.
$r = Run @('extract','--input',"$inDir\*.pdf",'--json')
$j = AsJson $r.stdout
Check "glob feeding a batch command processes every match" `
    ($r.code -eq 0 -and $j.details.document_count -eq 2) "documents=$($j.details.document_count)"

$emptyDir = Join-Path $work "empty"
New-Item -ItemType Directory -Force -Path $emptyDir | Out-Null
$r = Run @('info','--input',$emptyDir,'--json')
$j = AsJson $r.stdout
Check "directory with no PDFs -> actionable error" ($j -and $j.message -match 'No PDF files found') "msg=$($j.message)"

Write-Host "`n== --help / --version never crash =="
$r = Run @('--version')
Check "--version exits 0" ($r.code -eq 0 -and $r.stdout -match 'pdf-cli') "out=$($r.stdout.Trim())"
$r = Run @('--help')
Check "--help exits 0 and lists subcommands" ($r.code -eq 0 -and $r.stdout -match 'search') "ok"
$r = Run @('search','--help')
Check "subcommand --help exits 0" ($r.code -eq 0) "ok"

Write-Host "`n== every command in --help is dispatchable =="
$helpText = (Run @('--help')).stdout
$commands = @('convert','extract','merge','split','delete-pages','rotate','watermark-add',
             'watermark-remove','encrypt','decrypt','compress','info','doctor','web-to-pdf',
             'pdf-to-image','invoice','pagenum','meta','extract-images','search',
             'to-markdown','diff','form','crop','organize','redact','overlay')
$missing = @()
foreach ($c in $commands) { if ($helpText -notmatch [regex]::Escape($c)) { $missing += $c } }
Check "all $($commands.Count) commands appear in --help" ($missing.Count -eq 0) "missing=$(if ($missing) { $missing -join ',' } else { 'none' })"

# OCR was deliberately removed: it belongs to a separate OCR tool, not to pdf-cli.
$r = Run @('ocr')
Check "ocr is no longer a subcommand" ($r.code -eq 2 -and $r.stderr -match 'unrecognized subcommand') "code=$($r.code)"

Write-Host ""
$color = if ($script:fail -eq 0) { "Green" } else { "Red" }
Write-Host ("TOTAL: PASS=$($script:pass) FAIL=$($script:fail) SKIP=$($script:skip)") -ForegroundColor $color
Write-Host "work dir: $work"
if ($script:fail -eq 0) { exit 0 } else { exit 1 }
