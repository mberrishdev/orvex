# Fixture-only tests: Start-Process is mocked; no installer or network request runs.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$root = Join-Path ([IO.Path]::GetTempPath()) ('orvex-installer-tests-' + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $root
$script:mode = 'good'
$script:launched = $false
$script:urls = @()
$script:payload = [Text.Encoding]::UTF8.GetBytes('installer fixture; never executed')
$hash = [Security.Cryptography.SHA256]::Create()
$script:digest = ([BitConverter]::ToString($hash.ComputeHash($script:payload))).Replace('-', '').ToLowerInvariant()
$hash.Dispose()

function Invoke-WebRequest {
    param($Uri, $OutFile, [switch]$UseBasicParsing, $TimeoutSec)
    $script:urls += $Uri
    if ($Uri.EndsWith('latest.json')) {
        $url = if ($script:mode -eq 'url') { 'https://example.com/unsafe.exe' } else { 'https://github.com/mberrishdev/orvex/releases/download/v0.1.0-beta.3/Orvex_0.1.0-beta.3_x64-setup.exe' }
        @{version='0.1.0-beta.3'; platforms=@{'windows-x86_64'=@{url=$url}}} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $OutFile -Encoding ASCII
    } elseif ($Uri.EndsWith('SHA256SUMS.txt')) {
        $digest = if ($script:mode -eq 'checksum') { '0' * 64 } else { $script:digest }
        $line = "$digest  Orvex_0.1.0-beta.3_x64-setup.exe"
        if ($script:mode -eq 'duplicate') { $line += "`n$line" }
        $line | Set-Content -LiteralPath $OutFile -Encoding ASCII
    } else { [IO.File]::WriteAllBytes($OutFile, $script:payload) }
}
function Start-Process {
    param($FilePath, $ArgumentList, [switch]$Wait, [switch]$PassThru)
    $script:launched = $true
    if ($script:mode -eq 'exit') { return [pscustomobject]@{ExitCode=7} }
    if ($script:mode -eq 'silent' -and $ArgumentList[0] -ne '/S') { throw 'Silent flag was not forwarded' }
    return [pscustomobject]@{ExitCode=0}
}
function Assert-True($Value, $Message) { if (-not $Value) { throw $Message } }
$installer = Join-Path $PSScriptRoot '../install.ps1'
try {
    $output = Join-Path $root 'downloads'
    & $installer -DownloadOnly -OutputDirectory $output
    Assert-True (Test-Path (Join-Path $output 'Orvex_0.1.0-beta.3_x64-setup.exe')) 'Missing verified download'
    Assert-True (-not $script:launched) 'Download-only launched the installer'
    Assert-True ($script:urls[0].Contains('download/updater-beta/')) 'Wrong beta feed'
    $script:urls = @()
    & $installer -Version v0.1.0-beta.3 -Silent
    Assert-True ($script:urls[0].Contains('download/v0.1.0-beta.3/latest.json')) 'Wrong pinned release'
    Assert-True $script:launched 'Installer was not started'
    $script:mode = 'silent'
    & $installer -Channel stable -Silent
    Assert-True ($script:urls -contains 'https://github.com/mberrishdev/orvex/releases/latest/download/latest.json') 'Wrong stable feed'
    foreach ($case in @('checksum', 'duplicate', 'url')) {
        $script:mode = $case; $script:launched = $false; $failed = $false
        try { & $installer -Silent } catch { $failed = $true }
        Assert-True $failed "Unsafe fixture was accepted: $case"
        Assert-True (-not $script:launched) "Unsafe installer was launched: $case"
    }
    $script:mode = 'exit'; $failed = $false
    try { & $installer -Silent } catch { $failed = $_.Exception.Message.Contains('code 7') }
    Assert-True $failed 'Installer exit code was ignored'
    $script:mode = 'good'; $failed = $false
    try { & $installer -Version 9.9.9 -Silent } catch { $failed = $_.Exception.Message.Contains('does not match') }
    Assert-True $failed 'Mismatched release version was accepted'
    Write-Host 'PASS: Windows download-only, version selection, channels, silent install, checksum rejection, URL validation, and installer failure propagation.'
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force
}
