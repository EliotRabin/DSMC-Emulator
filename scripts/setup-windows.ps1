$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$destination = Join-Path $repoRoot 'native\win64'
New-Item -ItemType Directory -Path $destination -Force | Out-Null
$archive = Join-Path ([System.IO.Path]::GetTempPath()) ('dsmc-unicorn-' + [guid]::NewGuid().ToString('N') + '.zip')
try {
    $url = 'https://files.pythonhosted.org/packages/70/3d/de7be9bd1addabe6d8a1369381f8a080400c349850e978689c5e18287957/unicorn-2.1.4-cp37-abi3-win_amd64.whl'
    Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
    $expected = 'd7107500c64ce5c168fbff6bef9485b5db1350050036f4cea568650cf8bdbdf5'
    $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        throw 'Unicorn archive checksum mismatch'
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::OpenRead($archive)
    try {
        $entry = $zip.GetEntry('unicorn/lib/unicorn.dll')
        if ($null -eq $entry) {
            throw 'Unicorn DLL missing from archive'
        }
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $destination 'unicorn.dll'), $true)
    } finally {
        $zip.Dispose()
    }
    Write-Host "Unicorn 2.1.4 installed in $destination. Python is not required."
} finally {
    if (Test-Path -LiteralPath $archive) {
        Remove-Item -LiteralPath $archive
    }
}
