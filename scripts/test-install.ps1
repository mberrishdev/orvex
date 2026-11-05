# Fixture-only tests: Start-Process is mocked; no installer or network request runs.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) ('orvex-installer-tests-' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $root
$global:OrvexInstallerFixture = @{ Mode = 'good'; Launched = $false; Urls = @(); Payload = $null; Digest = $null }
$global:OrvexInstallerFixture.Launched = $false
$global:OrvexInstallerFixture.Urls = @()
$global:OrvexInstallerFixture.Payload = [Text.Encoding]::UTF8.GetBytes('installer fixture; never executed')
$hash = [Security.Cryptography.SHA256]::Create()
$global:OrvexInstallerFixture.Digest = ([BitConverter]::ToString($hash.ComputeHash($global:OrvexInstallerFixture.Payload))).Replace('-', '').ToLowerInvariant()
$hash.Dispose()

function Invoke-WebRequest {
    param($Uri, $OutFile, [switch]$UseBasicParsing, $TimeoutSec)
    $global:OrvexInstallerFixture.Urls += $Uri
    if ($Uri.EndsWith('latest.json')) {
        $url = if ($global:OrvexInstallerFixture.Mode -eq 'url') { 'https://example.com/unsafe.exe' } else { 'https://github.com/mberrishdev/orvex/releases/download/v0.1.0-beta.3/Orvex_0.1.0-beta.3_x64-setup.exe' }
        @{version='0.1.0-beta.3'; platforms=@{'windows-x86_64'=@{url=$url}}} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $OutFile -Encoding ASCII
    } elseif ($Uri.EndsWith('SHA256SUMS.txt')) {
        $digest = if ($global:OrvexInstallerFixture.Mode -eq 'checksum') { '0' * 64 } else { $global:OrvexInstallerFixture.Digest }
        $line = "$digest  Orvex_0.1.0-beta.3_x64-setup.exe"
        if ($global:OrvexInstallerFixture.Mode -eq 'duplicate') { $line += "`n$line" }
        $line | Set-Content -LiteralPath $OutFile -Encoding ASCII
    } else { [IO.File]::WriteAllBytes($OutFile, $global:OrvexInstallerFixture.Payload) }
}
function Start-Process {
    param($FilePath, $ArgumentList, [switch]$Wait, [switch]$PassThru)
    $global:OrvexInstallerFixture.Launched = $true
    if ($global:OrvexInstallerFixture.Mode -eq 'exit') { return [pscustomobject]@{ExitCode=7} }
    if ($global:OrvexInstallerFixture.Mode -eq 'silent' -and $ArgumentList[0] -ne '/S') { throw 'Silent flag was not forwarded' }
    return [pscustomobject]@{ExitCode=0}
}
function Assert-True($Value, $Message) { if (-not $Value) { throw $Message } }
$installer = Join-Path $PSScriptRoot '../install.ps1'
try {
    $output = Join-Path $root 'downloads'
    & $installer -DownloadOnly -OutputDirectory $output
    Assert-True (Test-Path (Join-Path $output 'Orvex_0.1.0-beta.3_x64-setup.exe')) 'Missing verified download'
    Assert-True (-not $global:OrvexInstallerFixture.Launched) 'Download-only launched the installer'
    Assert-True ($global:OrvexInstallerFixture.Urls[0].Contains('download/updater-beta/')) 'Wrong beta feed'
    $global:OrvexInstallerFixture.Urls = @()
    & $installer -Version v0.1.0-beta.3 -Silent
    Assert-True ($global:OrvexInstallerFixture.Urls[0].Contains('download/v0.1.0-beta.3/latest.json')) 'Wrong pinned release'
    Assert-True $global:OrvexInstallerFixture.Launched 'Installer was not started'
    $global:OrvexInstallerFixture.Mode = 'silent'
    & $installer -Channel stable -Silent
    Assert-True ($global:OrvexInstallerFixture.Urls -contains 'https://github.com/mberrishdev/orvex/releases/latest/download/latest.json') 'Wrong stable feed'
    foreach ($case in @('checksum', 'duplicate', 'url')) {
        $global:OrvexInstallerFixture.Mode = $case; $global:OrvexInstallerFixture.Launched = $false; $failed = $false
        try { & $installer -Silent } catch { $failed = $true }
        Assert-True $failed "Unsafe fixture was accepted: $case"
        Assert-True (-not $global:OrvexInstallerFixture.Launched) "Unsafe installer was launched: $case"
    }
    $global:OrvexInstallerFixture.Mode = 'exit'; $failed = $false
    try { & $installer -Silent } catch { $failed = $_.Exception.Message.Contains('code 7') }
    Assert-True $failed 'Installer exit code was ignored'
    $global:OrvexInstallerFixture.Mode = 'good'; $failed = $false
    try { & $installer -Version 9.9.9 -Silent } catch { $failed = $_.Exception.Message.Contains('does not match') }
    Assert-True $failed 'Mismatched release version was accepted'
    Write-Host 'PASS: Windows download-only, version selection, channels, silent install, checksum rejection, URL validation, and installer failure propagation.'
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force
    Remove-Variable OrvexInstallerFixture -Scope Global
}
