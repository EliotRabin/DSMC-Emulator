param(
    [switch]$Run
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    if (-not (Test-Path -LiteralPath 'native\win64\unicorn.dll')) {
        & "$PSScriptRoot\setup-windows.ps1"
    }
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) {
        throw 'Rust build failed'
    }
    $cargoTarget = if ($env:CARGO_TARGET_DIR) {
        $env:CARGO_TARGET_DIR
    } else {
        Join-Path $repoRoot 'target'
    }
    $releaseDir = Join-Path $cargoTarget 'release'
    Copy-Item -LiteralPath 'native\win64\unicorn.dll' -Destination (Join-Path $releaseDir 'unicorn.dll') -Force
    $portable = Join-Path $repoRoot 'dist\windows-x64'
    New-Item -ItemType Directory -Path $portable -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $releaseDir 'dsmc-emulator.exe') -Destination $portable -Force
    Copy-Item -LiteralPath 'native\win64\unicorn.dll' -Destination $portable -Force
    Copy-Item -LiteralPath 'native\COPYING','native\COPYING.LGPL2','native\README.md' -Destination $portable -Force
    $executable = Join-Path $portable 'dsmc-emulator.exe'
    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $executable
    $startInfo.Arguments = '--self-test'
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $check = [System.Diagnostics.Process]::Start($startInfo)
    try {
        if (-not $check.WaitForExit(15000)) {
            $check.Kill()
            throw 'Unicorn self-test timed out'
        }
        if ($check.ExitCode -ne 0) {
            throw 'Unicorn self-test failed'
        }
    } finally {
        $check.Dispose()
    }
    Write-Host "Ready: $executable"
    if ($Run) {
        $startInfo.Arguments = ''
        $startInfo.CreateNoWindow = $false
        $application = [System.Diagnostics.Process]::Start($startInfo)
        $application.Dispose()
    }
} finally {
    Pop-Location
}
