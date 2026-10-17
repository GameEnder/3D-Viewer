# installer/build.ps1 - build the 3D Viewer MSI.
# Usage:  .\installer\build.ps1
# Output: installer\bin\3DViewer-<version>-x64.msi
#
# Requires WiX v3 (candle.exe/light.exe). Resolution order:
#   1. on PATH
#   2. chocolatey install ("C:\Program Files (x86)\WiX Toolset 3.14\bin")
#   3. portable WiX download (set $WixBin to its bin folder if needed)
param(
    # Portable WiX bin folder, e.g. "$env:TEMP\opencode\wix314\bin"
    [string]$WixBin = ""
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Split-Path -Parent $here

# --- locate candle/light ---------------------------------------------------
function Find-WixTool([string]$name) {
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $choco = Join-Path ${env:ProgramFiles(x86)} "WiX Toolset 3.14\bin\$name"
    if (Test-Path $choco) { return $choco }
    if ($WixBin -and (Test-Path (Join-Path $WixBin $name))) {
        return (Join-Path $WixBin $name)
    }
    throw "$name not found. Install WiX v3 (choco install wixtoolset) or pass -WixBin."
}
$candle = Find-WixTool 'candle.exe'
$light  = Find-WixTool 'light.exe'
Write-Output "WiX: $candle"

# --- version from crates/viewer-app/Cargo.toml ----------------------------
$cargoToml = Get-Content (Join-Path $root 'crates\viewer-app\Cargo.toml') -Raw
if ($cargoToml -notmatch '(?m)^version\s*=\s*"([^"]+)"') {
    throw 'cannot find version in crates/viewer-app/Cargo.toml'
}
$version = $Matches[1]
Write-Output "Version: $version"

# --- build the app ----------------------------------------------------------
Write-Output 'Building viewer.exe (release)...'
Push-Location $root
try {
    & cargo build --release -p viewer-app
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
} finally { Pop-Location }

# --- compile + link the MSI -------------------------------------------------
$buildDir = Join-Path $here 'build'
$outDir   = Join-Path $here 'bin'
New-Item -ItemType Directory -Path $buildDir -Force | Out-Null
New-Item -ItemType Directory -Path $outDir   -Force | Out-Null

$wixObj = Join-Path $buildDir 'viewer.wixobj'
$license = Join-Path $here 'license.rtf'
if (-not (Test-Path $license)) { throw "license.rtf not found: $license" }
& $candle -arch x64 `
    "-dProductVersion=$version" `
    "-dRepoRoot=$root" `
    "-dLicenseRtf=$license" `
    -out $wixObj `
    (Join-Path $here 'viewer.wxs')
if ($LASTEXITCODE -ne 0) { throw 'candle failed' }

$msi = Join-Path $outDir "3DViewer-$version-x64.msi"
# The WixUIExtension provides the wizard dialogs; the license path is
# absolute (baked in by candle), so no bind-path resolution is needed.
& $light -ext WixUIExtension `
    -cultures:en-us `
    -out $msi `
    $wixObj
if ($LASTEXITCODE -ne 0) { throw 'light failed' }

Write-Output "MSI: $msi"
Write-Output "Size: $([math]::Round((Get-Item $msi).Length / 1MB, 1)) MB"
