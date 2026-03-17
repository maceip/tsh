# tsh installer — compatible with: irm 'https://raw.githubusercontent.com/maceip/tsh/main/install.ps1' | iex
# No #Requires directive (breaks iex piping)

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = "maceip/tsh"
$InstallDir = if ($env:TSH_INSTALL_DIR) { $env:TSH_INSTALL_DIR } else { "$env:LOCALAPPDATA\tsh" }
$BinDir = "$InstallDir\bin"
$BaseUrl = "https://github.com/$Repo/releases"

# --- Helpers ----------------------------------------------------------------

function Write-Info($msg) { Write-Host $msg -ForegroundColor Green }
function Write-Warn($msg) { Write-Host "warning: $msg" -ForegroundColor Yellow }

# --- Detect architecture ----------------------------------------------------

function Get-Arch {
    $arch = $env:PROCESSOR_ARCHITECTURE
    try { $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() } catch {}
    switch ($arch) {
        "X64"   { return "x86_64" }
        "AMD64" { return "x86_64" }
        "Arm64" { return "aarch64" }
        "ARM64" { return "aarch64" }
        default { throw "Unsupported architecture: $arch" }
    }
}

# --- Get latest version -----------------------------------------------------

function Get-LatestVersion {
    if ($env:TSH_VERSION) { return $env:TSH_VERSION }
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    return $release.tag_name
}

# --- Main -------------------------------------------------------------------

& {
    $Arch = Get-Arch
    Write-Info "Detected platform: windows-$Arch"

    $Version = Get-LatestVersion
    if (-not $Version) { throw "Could not determine latest version" }

    Write-Info "Installing tsh $Version"

    $ArchiveName = "tsh-${Version}-windows-${Arch}.zip"
    $DownloadUrl = "$BaseUrl/download/$Version/$ArchiveName"

    # Download
    $TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("tsh-install-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
    New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null
    $ZipPath = Join-Path $TmpDir $ArchiveName

    Write-Info "Downloading $DownloadUrl"
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $ZipPath -UseBasicParsing

    # Extract
    $ExtractDir = Join-Path $TmpDir "extracted"
    Expand-Archive -Path $ZipPath -DestinationPath $ExtractDir

    # Install
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    $StagingDir = Join-Path $ExtractDir "tsh-${Version}-windows-${Arch}"

    # Handle flat extraction (Compress-Archive may not preserve folder)
    if (-not (Test-Path $StagingDir)) {
        $StagingDir = $ExtractDir
    }

    Copy-Item (Join-Path $StagingDir "tsh.exe") -Destination (Join-Path $BinDir "tsh.exe") -Force
    Copy-Item (Join-Path $StagingDir "safety_filter.py") -Destination (Join-Path $BinDir "safety_filter.py") -Force

    # Store version
    Set-Content -Path (Join-Path $InstallDir ".version") -Value $Version

    # --- Generate tsh-update.ps1 --------------------------------------------
    $UpdateScript = @'
$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = "maceip/tsh"
$InstallDir = if ($env:TSH_INSTALL_DIR) { $env:TSH_INSTALL_DIR } else { "$env:LOCALAPPDATA\tsh" }
$BinDir = "$InstallDir\bin"
$BaseUrl = "https://github.com/$Repo/releases"

$Current = "unknown"
$VersionFile = Join-Path $InstallDir ".version"
if (Test-Path $VersionFile) { $Current = (Get-Content $VersionFile).Trim() }

Write-Host "Current version: $Current" -ForegroundColor Green
Write-Host "Checking for updates..." -ForegroundColor Green

$release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
$LatestTag = $release.tag_name

if (-not $LatestTag) { throw "Could not determine latest version" }

$CurrentVer = [version]($Current -replace '^v', '')
$LatestVer  = [version]($LatestTag -replace '^v', '')

if ($LatestVer -le $CurrentVer) {
    Write-Host "Already up to date ($Current)" -ForegroundColor Green
    exit 0
}

Write-Host "Updating from $Current to $LatestTag" -ForegroundColor Green

$arch = $env:PROCESSOR_ARCHITECTURE
try { $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() } catch {}
$arch = switch ($arch) { "X64" { "x86_64" } "AMD64" { "x86_64" } "Arm64" { "aarch64" } "ARM64" { "aarch64" } default { throw "Unsupported architecture" } }

$ArchiveName = "tsh-${LatestTag}-windows-${arch}.zip"
$DownloadUrl = "$BaseUrl/download/$LatestTag/$ArchiveName"

$TmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("tsh-update-" + [guid]::NewGuid().ToString("N").Substring(0, 8))
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null
$ZipPath = Join-Path $TmpDir $ArchiveName

Invoke-WebRequest -Uri $DownloadUrl -OutFile $ZipPath -UseBasicParsing
$ExtractDir = Join-Path $TmpDir "extracted"
Expand-Archive -Path $ZipPath -DestinationPath $ExtractDir

$StagingDir = Join-Path $ExtractDir "tsh-${LatestTag}-windows-${arch}"
if (-not (Test-Path $StagingDir)) { $StagingDir = $ExtractDir }

Copy-Item (Join-Path $StagingDir "tsh.exe") -Destination (Join-Path $BinDir "tsh.exe") -Force
Copy-Item (Join-Path $StagingDir "safety_filter.py") -Destination (Join-Path $BinDir "safety_filter.py") -Force
Set-Content -Path (Join-Path $InstallDir ".version") -Value $LatestTag

Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue
Write-Host "Updated to $LatestTag" -ForegroundColor Green
'@
    Set-Content -Path (Join-Path $BinDir "tsh-update.ps1") -Value $UpdateScript

    # --- Generate tsh-uninstall.ps1 -----------------------------------------
    $UninstallScript = @'
$ErrorActionPreference = "Stop"

$InstallDir = if ($env:TSH_INSTALL_DIR) { $env:TSH_INSTALL_DIR } else { "$env:LOCALAPPDATA\tsh" }

$confirm = Read-Host "This will remove tsh from $InstallDir and clean PATH. Continue? [y/N]"
if ($confirm -notmatch '^[yY]') {
    Write-Host "Aborted."
    exit 0
}

Remove-Item -Recurse -Force $InstallDir -ErrorAction SilentlyContinue

$UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
$CleanPath = ($UserPath -split ';' | Where-Object { $_ -notlike '*tsh*bin*' }) -join ';'
[Environment]::SetEnvironmentVariable("PATH", $CleanPath, "User")

Write-Host "tsh has been uninstalled." -ForegroundColor Green
Write-Host "Open a new terminal for PATH changes to take effect."
'@
    Set-Content -Path (Join-Path $BinDir "tsh-uninstall.ps1") -Value $UninstallScript

    # --- Configure PATH -----------------------------------------------------
    $UserPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    if ($UserPath -notlike "*tsh*bin*") {
        $NewPath = "$BinDir;$UserPath"
        [Environment]::SetEnvironmentVariable("PATH", $NewPath, "User")
        Write-Info "Added $BinDir to User PATH"
    }
    $env:PATH = "$BinDir;$env:PATH"

    # Cleanup
    Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue

    # Python check
    $python = Get-Command python3 -ErrorAction SilentlyContinue
    if (-not $python) {
        $python = Get-Command python -ErrorAction SilentlyContinue
    }
    if (-not $python) {
        Write-Warn "python3 not found - tsh requires Python 3 for the safety filter"
    }

    # Done
    Write-Host ""
    Write-Info "tsh $Version installed to $BinDir"
    Write-Host ""
    Write-Host "  To get started, open a new terminal and run:  tsh"
    Write-Host ""
    Write-Host "  Other commands:"
    Write-Host "    tsh-update.ps1      Update to the latest version"
    Write-Host "    tsh-uninstall.ps1   Remove tsh from your system"
    Write-Host ""
}
