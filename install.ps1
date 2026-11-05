<# Install the released Windows desktop app. Requires Windows PowerShell 5.1 or PowerShell 7. #>
[CmdletBinding()]
param(
    [string]$Version,
    [ValidateSet('beta', 'stable')][string]$Channel = 'beta',
    [switch]$Silent,
    [switch]$DownloadOnly,
    [string]$OutputDirectory = (Join-Path (Get-Location) 'orvex-downloads')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw 'Use install.sh on macOS. This installer requires Windows x64.'
}
$architecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($architecture -ne 'AMD64') { throw 'There is no Windows ARM64 or 32-bit release yet. Windows x64 is required.' }
$versionPattern = '^\d+\.\d+\.\d+(-beta\.\d+)?$'
if ($Version) {
    $Version = $Version -replace '^v', ''
    if ($Version -notmatch $versionPattern) { throw 'Expected a version such as 0.1.0-beta.3.' }
}
# Windows PowerShell 5.1 may otherwise negotiate older TLS versions.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$base = 'https://github.com/mberrishdev/orvex/releases'
$feed = if ($Version) { "$base/download/v$Version/latest.json" } elseif ($Channel -eq 'beta') { "$base/download/updater-beta/latest.json" } else { "$base/latest/download/latest.json" }
$work = Join-Path ([IO.Path]::GetTempPath()) ('orvex-install-' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $work

function Get-OrvexDownload([string]$Url, [string]$Destination) {
    if (-not $Url.StartsWith('https://github.com/mberrishdev/orvex/releases/', [StringComparison]::Ordinal)) { throw 'Unexpected release URL.' }
    Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing -TimeoutSec 600
}

try {
    Write-Host 'Finding Orvex release...'
    $manifestPath = Join-Path $work 'latest.json'
    Get-OrvexDownload $feed $manifestPath
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $resolved = [string]$manifest.version
    if ($resolved -notmatch $versionPattern) { throw 'Release manifest contains an invalid version.' }
    if ($Version -and $Version -ne $resolved) { throw 'Release manifest does not match the requested version.' }
    $asset = "Orvex_${resolved}_x64-setup.exe"
    $url = "$base/download/v$resolved/$asset"
    if ([string]$manifest.platforms.'windows-x86_64'.url -cne $url) { throw 'Release manifest contains an unexpected Windows download URL.' }
    $installer = Join-Path $work $asset
    Write-Host "Downloading Orvex $resolved for Windows x64..."
    Get-OrvexDownload $url $installer
    $checksums = Join-Path $work 'SHA256SUMS.txt'
    Get-OrvexDownload "$base/download/v$resolved/SHA256SUMS.txt" $checksums
    $pattern = '^([a-fA-F0-9]{64})  ' + [Regex]::Escape($asset) + '$'
    $matching = @(Get-Content -LiteralPath $checksums | Where-Object { $_ -cmatch $pattern })
    if ($matching.Count -ne 1) { throw 'Missing or ambiguous SHA-256 checksum for the installer.' }
    $expected = $matching[0].Substring(0, 64)
    $actual = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
    if ($expected -ine $actual) { throw 'Checksum mismatch. Nothing was installed.' }
    Write-Host 'SHA-256 checksum verified.'
    if ($DownloadOnly) {
        $null = New-Item -ItemType Directory -Path $OutputDirectory -Force
        $destination = Join-Path $OutputDirectory $asset
        if (Test-Path -LiteralPath $destination) { throw "Download already exists: $destination" }
        Copy-Item -LiteralPath $installer -Destination $destination
        "$expected  $asset" | Set-Content -LiteralPath "$destination.sha256" -Encoding ASCII
        Write-Host "Downloaded $destination"
    } else {
        # Do not elevate here. NSIS applies the release's configured install mode.
        $arguments = @{ FilePath = $installer; Wait = $true; PassThru = $true }
        if ($Silent) { $arguments.ArgumentList = @('/S') }
        $process = Start-Process @arguments
        if ($process.ExitCode -ne 0) { throw "Orvex installer exited with code $($process.ExitCode)." }
        Write-Host "Installed Orvex $resolved."
    }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force
}
