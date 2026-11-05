param(
    [Parameter(Mandatory = $true)][string]$Firmware,
    [string]$Executable = (Join-Path $PSScriptRoot '..\dist\windows-x64\dsmc-emulator.exe'),
    [string]$Report = (Join-Path $PSScriptRoot '..\artifacts\startup-audit.json'),
    [ValidateRange(100000000, 100000000000)][long]$Steps = 700000000,
    [ValidateRange(1, 3600)][int]$Timeout = 180,
    [switch]$Hardware
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$firmwarePath = (Resolve-Path -LiteralPath $Firmware).ProviderPath
$executablePath = (Resolve-Path -LiteralPath $Executable).ProviderPath
if (-not [System.IO.Path]::IsPathRooted($Report)) {
    $Report = Join-Path (Get-Location).ProviderPath $Report
}
$reportPath = [System.IO.Path]::GetFullPath($Report)
New-Item -ItemType Directory -Path (Split-Path -Parent $reportPath) -Force | Out-Null

# UseShellExecute=false also handles an executable launched from a WSL UNC path.
# Drain both pipes asynchronously so a large JSON report cannot block the process.
$startInfo = New-Object System.Diagnostics.ProcessStartInfo
$startInfo.FileName = $executablePath
$startInfo.WorkingDirectory = $repoRoot
$startInfo.Arguments = '--headless --check-warnings --firmware "' + $firmwarePath +
    '" --report "' + $reportPath + '" --steps ' + $Steps + ' --timeout ' + $Timeout
if ($Hardware) { $startInfo.Arguments += ' --hardware' }
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$process = [System.Diagnostics.Process]::Start($startInfo)
try {
    $output = $process.StandardOutput.ReadToEndAsync()
    $errors = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $null = $output.Result
    Write-Host $errors.Result
    if ($process.ExitCode -ne 0) {
        throw "Startup audit failed (exit $($process.ExitCode)); report: $reportPath"
    }
    Write-Host "Startup audit passed; report: $reportPath"
} finally {
    $process.Dispose()
}
