# Extended repair through the app's path on every model in a list, one at a time through
# capped.ps1 (below-normal priority, -Seconds per file, -MaxGB), then a summary table.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts/corpus/repair-corpus.ps1 -OutDir <dir> -Models <dir>
#
# -Models is the folder with the model files the list names (for the default list, Thingi10K's
# raw_meshes folder); without it, the TRICKY3D_CORPUS environment variable must name it.
#
# Per file it runs the `extended_repair_report` test of src-tauri/tests/wrap_overlap.rs and keeps
# its lines in <dir>/logs. <dir>/results.jsonl gets one line per file; files already there are
# skipped, so a stopped run continues where it was. <dir>/summary.md is rewritten at the end
# (-SummaryOnly rewrites just that). -Only takes a file with one model name per line, to rerun a
# subset (say, last round's failures, into a new -OutDir). The corpus is only read.
param(
    [Parameter(Mandatory)] [string] $OutDir,
    [string] $List = "",
    [string] $Models = $env:TRICKY3D_CORPUS,
    [string] $Only = "",
    [int] $Seconds = 90,
    [double] $MaxGB = 8,
    [switch] $SummaryOnly
)
$ErrorActionPreference = "Stop"
if (-not $SummaryOnly -and -not $Models) { throw "Name the model folder with -Models or TRICKY3D_CORPUS." }
if (-not $List) { $List = Join-Path $PSScriptRoot "thingi10k-broken.txt" }
$repo = Split-Path (Split-Path $PSScriptRoot)
$tauri = Join-Path $repo "src-tauri"
New-Item -ItemType Directory -Force -Path (Join-Path $OutDir "logs") | Out-Null
$results = Join-Path $OutDir "results.jsonl"

function Build-Test {
    # One shared target directory for every bench build; no LTO, so a rebuild takes a minute.
    $env:CARGO_TARGET_DIR = Join-Path $tauri "target/b"
    $env:CARGO_BUILD_JOBS = "8"
    $env:CARGO_PROFILE_RELEASE_LTO = "false"
    $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "16"
    Push-Location $tauri
    try {
        foreach ($try in 1..2) {
            # The first build after a change sometimes fails on two serde_json versions; the
            # second does not.
            $lines = & cmd.exe /c "cargo test --release --test wrap_overlap --no-run --message-format=json 2>nul"
            $exe = $lines | Where-Object { $_ -like '*"executable":"*' } | ForEach-Object {
                $_ | ConvertFrom-Json
            } | Where-Object { $_.target.name -eq "wrap_overlap" -and $_.executable } |
                ForEach-Object { $_.executable } | Select-Object -Last 1
            if ($LASTEXITCODE -eq 0 -and $exe) { return $exe }
        }
        throw "cargo test --no-run failed"
    } finally { Pop-Location }
}

function Read-Lines($path) {
    if (-not (Test-Path -LiteralPath $path)) { return @() }
    Get-Content -LiteralPath $path | Where-Object { $_.Trim() } | ForEach-Object {
        try { $_ | ConvertFrom-Json } catch { }
    }
}

# Why a file did not pass, in words that group well.
function Cause($row) {
    switch ($row.outcome) {
        "pass" { return "" }
        "timeout" { return "time limit in $($row.stage)" }
        "memory" { return "memory cap in $($row.stage)" }
        "crash" { return "crash in $($row.stage): $($row.panic)" }
        "error" { return "error: $($row.error)" }
    }
    $what = @()
    if (-not $row.closed) { $what += "not closed" }
    if (-not $row.oriented) { $what += "not oriented" }
    if ($row.crossings -gt 0) { $what += "crossings" }
    if ($row.oneTriangleShells -gt 0) { $what += "1-triangle shells" }
    $where = if ($row.failedStep) { "after $($row.failedStep)" } else { "after (unknown)" }
    $why = if ($row.wrapWhy) { " [$($row.wrapWhy)]" } else { "" }
    "$where`: $($what -join ', ')$why"
}

function Write-Summary {
    $rows = @(Read-Lines $results)
    $pass = @($rows | Where-Object { $_.outcome -eq "pass" }).Count
    $n = $rows.Count
    $rate = if ($n) { [math]::Round(100.0 * $pass / $n, 1) } else { 0 }
    $md = @("# Extended repair corpus run", "",
        "$pass of $n files pass ($rate %): closed, oriented, no crossing pairs, no 1-triangle shells.",
        "Run below-normal priority, $Seconds s and $MaxGB GB per file, one file at a time.", "",
        "## By outcome", "", "| Outcome | Files |", "|---|---:|")
    $rows | Group-Object outcome | Sort-Object Count -Descending | ForEach-Object {
        $md += "| $($_.Name) | $($_.Count) |"
    }
    $md += @("", "## Failures by cause", "", "| Cause | Files | Examples |", "|---|---:|---|")
    $rows | Where-Object { $_.outcome -ne "pass" } | Group-Object cause |
        Sort-Object Count -Descending | ForEach-Object {
            $ex = ($_.Group | Select-Object -First 5 | ForEach-Object { $_.file }) -join ", "
            $md += "| $($_.Name -replace '\|', '/') | $($_.Count) | $ex |"
        }
    $md += @("", "## Per file", "",
        "| File | Tris in | Outcome | Closed | Oriented | Crossings | 1-tri shells | Shells | Repair s | Wall s | Peak GB | Failed step / cause |",
        "|---|---:|---|---|---|---:|---:|---:|---:|---:|---:|---|")
    foreach ($r in $rows) {
        $md += "| $($r.file) | $($r.trianglesIn) | $($r.outcome) | $($r.closed) | $($r.oriented) | $($r.crossings) | $($r.oneTriangleShells) | $($r.shells) | $($r.seconds) | $($r.wall) | $($r.peakGB) | $($r.cause -replace '\|', '/') |"
    }
    Set-Content -LiteralPath (Join-Path $OutDir "summary.md") -Value $md -Encoding UTF8
    "$pass of $n pass ($rate %)"
}

if ($SummaryOnly) { Write-Summary; return }

$names = Get-Content -LiteralPath $List | Where-Object { $_ -and -not $_.StartsWith("#") } |
    ForEach-Object { ($_ -split " ")[0] }
if ($Only) {
    $keep = @(Get-Content -LiteralPath $Only | Where-Object { $_.Trim() })
    $names = $names | Where-Object { $keep -contains $_ }
}
$done = @{}
foreach ($r in (Read-Lines $results)) { $done[$r.file] = $true }
$exe = Build-Test
Write-Host "test binary: $exe"

$i = 0
foreach ($name in $names) {
    $i++
    if ($done[$name]) { continue }
    $model = Join-Path $Models $name
    $report = Join-Path $OutDir "logs/$name.jsonl"
    $out = Join-Path $OutDir "logs/$name.out"
    Remove-Item -LiteralPath $report, $out -ErrorAction SilentlyContinue
    $cap = & powershell -NoProfile -ExecutionPolicy Bypass -File "$PSScriptRoot/capped.ps1" `
        -Exe $exe -ArgLine "--ignored --exact extended_repair_report --test-threads=1 --nocapture" `
        -Env "TRICKY3D_MODEL=$model;TRICKY3D_REPORT=$report" -Seconds $Seconds -MaxGB $MaxGB -Out $out |
        ConvertFrom-Json
    $lines = @(Read-Lines $report)
    $stage = ($lines | Where-Object { $_.stage } | Select-Object -Last 1).stage
    $res = ($lines | Where-Object { $_.result } | Select-Object -Last 1).result
    $diag = $lines | Where-Object { $_.PSObject.Properties.Name -contains "failedStep" } | Select-Object -Last 1
    $row = [ordered]@{
        file = $name; outcome = ""; trianglesIn = $null; closed = $null; oriented = $null
        crossings = $null; oneTriangleShells = $null; shells = $null; trianglesOut = $null
        seconds = $null; wall = $cap.seconds; peakGB = $cap.peakGB; stage = $stage
        failedStep = $null; wrapWhy = $null; error = $null; panic = $null; warnings = @()
    }
    $imported = $lines | Where-Object { $_.stage -eq "imported" } | Select-Object -First 1
    if ($imported) { $row.trianglesIn = $imported.triangles }
    if ($res) {
        $row.closed = $res.closed; $row.oriented = $res.oriented; $row.crossings = $res.crossings
        $row.oneTriangleShells = $res.oneTriangleShells; $row.shells = $res.shells
        $row.trianglesOut = $res.triangles; $row.seconds = [math]::Round([double]$res.seconds, 2)
        $row.error = $res.error
        foreach ($rep in @($res.reports)) {
            foreach ($w in @($rep.warnings)) { if ($w) { $row.warnings += "$($rep.action): $w" } }
            if ($rep.action -eq "wrap_part_surface") {
                $m = [regex]::Match((@($rep.warnings) -join " "), "intersections \(([^)]*)\)")
                if ($m.Success) { $row.wrapWhy = $m.Groups[1].Value }
            }
        }
        if ($res.error) { $row.outcome = "error" }
        elseif ($res.ok) { $row.outcome = "pass" }
        else { $row.outcome = "fail" }
        if ($diag -and $diag.failedStep) { $row.failedStep = $diag.failedStep.step }
    }
    if ($cap.status -ne "exited" -and $row.outcome -ne "fail") {
        # Killed before the result (a failed result killed during its diagnosis stays "fail").
        if (-not $res) { $row.outcome = $cap.status }
    } elseif (-not $res) {
        $row.outcome = "crash"
        $text = if (Test-Path -LiteralPath $out) { Get-Content -LiteralPath $out -Raw } else { "" }
        $m = [regex]::Match($text, "panicked at [^\r\n]*[\r\n]+([^\r\n]*)")
        $row.panic = if ($m.Success) { $m.Groups[1].Value } else { "exit $($cap.exitCode)" }
    }
    $row.cause = Cause ([pscustomobject]$row)
    Add-Content -LiteralPath $results -Value ([pscustomobject]$row | ConvertTo-Json -Compress -Depth 5) -Encoding UTF8
    Write-Host ("[{0}/{1}] {2}: {3} {4}" -f $i, $names.Count, $name, $row.outcome, $row.cause)
}
Write-Summary
