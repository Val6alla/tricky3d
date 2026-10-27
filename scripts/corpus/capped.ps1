# Runs one program so the PC stays usable: below-normal priority, a time limit and a memory cap,
# the whole process tree killed when either is passed. Prints one JSON line:
# {"status":"exited"|"timeout"|"memory","exitCode":n,"seconds":s,"peakGB":g}.
#
#   capped.ps1 -Exe C:\abs\prog.exe -ArgLine "--flag x" -Env "K=V;K2=V2" -Seconds 90 -MaxGB 8 -Out log.txt
param(
    [Parameter(Mandatory)] [string] $Exe,
    [string] $ArgLine = "",
    [string] $Env = "",
    [int] $Seconds = 90,
    [double] $MaxGB = 8,
    [string] $Out = ""
)
$ErrorActionPreference = "Stop"

Add-Type -Namespace Capped -Name Native -MemberDefinition @"
[StructLayout(LayoutKind.Sequential)]
public struct PowerThrottling { public uint Version; public uint ControlMask; public uint StateMask; }
[DllImport("kernel32.dll", SetLastError = true)]
public static extern bool SetProcessInformation(System.IntPtr process, int infoClass, ref PowerThrottling info, uint size);
"@

$info = New-Object System.Diagnostics.ProcessStartInfo
$info.FileName = $Exe
$info.Arguments = $ArgLine
$info.UseShellExecute = $false
$info.CreateNoWindow = $true
if ($Out) {
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
}
foreach ($pair in ($Env -split ";" | Where-Object { $_ })) {
    $k, $v = $pair -split "=", 2
    $info.EnvironmentVariables[$k] = $v
}
$p = [System.Diagnostics.Process]::Start($info)
$p.PriorityClass = [System.Diagnostics.ProcessPriorityClass]::BelowNormal
# Windows would otherwise run a windowless process on efficiency cores at reduced speed
# (ProcessPowerThrottling = 4, execution speed bit 1, state 0 = throttling off).
$t = New-Object Capped.Native+PowerThrottling
$t.Version = 1; $t.ControlMask = 1; $t.StateMask = 0
[void][Capped.Native]::SetProcessInformation($p.Handle, 4, [ref]$t, 12)
if ($Out) {
    $stdout = $p.StandardOutput.ReadToEndAsync()
    $stderr = $p.StandardError.ReadToEndAsync()
}

$clock = [System.Diagnostics.Stopwatch]::StartNew()
$status = "exited"
$peak = 0.0
while (-not $p.WaitForExit(250)) {
    $p.Refresh()
    $gb = $p.PrivateMemorySize64 / 1GB
    $peak = [math]::Max($peak, [math]::Max($gb, $p.PeakWorkingSet64 / 1GB))
    if ($clock.Elapsed.TotalSeconds -gt $Seconds) { $status = "timeout" }
    elseif ($gb -gt $MaxGB) { $status = "memory" }
    if ($status -ne "exited") {
        & taskkill.exe /T /F /PID $p.Id 2>&1 | Out-Null
        [void]$p.WaitForExit(10000)
        break
    }
}
$elapsed = $clock.Elapsed.TotalSeconds
if ($Out) {
    [void]$p.WaitForExit()
    Set-Content -LiteralPath $Out -Value ($stdout.Result + $stderr.Result) -Encoding UTF8
}
$code = if ($p.HasExited) { $p.ExitCode } else { $null }
[pscustomobject]@{
    status   = $status
    exitCode = $code
    seconds  = [math]::Round($elapsed, 2)
    peakGB   = [math]::Round($peak, 2)
} | ConvertTo-Json -Compress
