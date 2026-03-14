# ============================================================================
# Token Shell (tsh) - PowerShell UTF-8 Profile Configuration
#
# This script configures the current user's PowerShell profile to force
# UTF-8 encoding for all external process pipes. This prevents the
# UTF-16LE encoding hazard when piping data into tsh.exe on Windows
# PowerShell 5.1.
#
# PowerShell 7 (Core) defaults to UTF-8 and does NOT require this script.
#
# Usage: paste into an active PowerShell window, or run:
#   powershell -ExecutionPolicy Bypass -File scripts\setup-powershell-utf8.ps1
# ============================================================================

# Determine the path to the current user's PowerShell profile
$ProfilePath = $PROFILE.CurrentUserCurrentHost

# Create the profile file and its parent directories if they do not exist
if (-not (Test-Path -Path $ProfilePath)) {
    New-Item -Path $ProfilePath -ItemType File -Force | Out-Null
    Write-Host "Created new PowerShell profile at: $ProfilePath"
}

# Check if the encoding overrides are already present
$ExistingContent = Get-Content -Path $ProfilePath -Raw -ErrorAction SilentlyContinue
if ($ExistingContent -and $ExistingContent.Contains("Token Shell Encoding Overrides")) {
    Write-Host "UTF-8 encoding overrides are already configured in your profile."
    Write-Host "Profile path: $ProfilePath"
    exit 0
}

# The required .NET encoding overrides
$EncodingConfig = @"

# ========================================================================
# Token Shell Encoding Overrides
# Forces PowerShell to use UTF-8 without BOM for external process pipes.
# Required for tsh.exe compatibility on Windows PowerShell 5.1.
# ========================================================================
`$OutputEncoding = [System.Text.Encoding]::UTF8
[System.Console]::InputEncoding = [System.Text.Encoding]::UTF8
[System.Console]::OutputEncoding = [System.Text.Encoding]::UTF8
"@

# Append the configuration to the profile
Add-Content -Path $ProfilePath -Value $EncodingConfig
Write-Host "UTF-8 encoding overrides appended to profile."

# Reload the profile into the current session
try {
    & $ProfilePath
    Write-Host "Profile reloaded. Ready for UTF-8 pipes."
} catch {
    Write-Host ""
    Write-Host "NOTE: If you see an execution policy error, run this as Administrator:"
    Write-Host "  Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser"
    Write-Host ""
    Write-Host "Then restart your PowerShell session."
}
