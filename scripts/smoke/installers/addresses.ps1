# Integrator-only URL matrix against the actual Windows installer; every installation/network operation is stubbed.
param([string]$Installer = (Join-Path $PSScriptRoot '../../../deploy/install/install.ps1'))
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$script:addressRequests = @()
$script:addressWrites = 0
$script:addressRemovals = 0
function New-Item { $script:addressWrites++ }
function Remove-Item { $script:addressRemovals++ }
function Move-Item { throw 'D441_FORBIDDEN file replacement' }
function Register-ScheduledTask { throw 'D441_FORBIDDEN task registration' }
function Start-ScheduledTask { throw 'D441_FORBIDDEN task start' }
function Invoke-RestMethod {
    param([string]$Uri, [int]$MaximumRedirection = -1, [switch]$UseBasicParsing)
    $script:addressRequests += [pscustomobject]@{ Uri = $Uri; Redirects = $MaximumRedirection }
    return [pscustomobject]@{ project = 'orch'; version = 'fixture'; assets = @(
        [pscustomobject]@{ download_url = '/api/v1/downloads/connector/windows/x86_64'; size = 1; sha256 = ('0' * 64) }
    ) }
}
function Invoke-WebRequest {
    param([string]$Uri, [string]$OutFile, [int]$MaximumRedirection = -1, [switch]$UseBasicParsing)
    $script:addressRequests += [pscustomobject]@{ Uri = $Uri; Redirects = $MaximumRedirection }
    throw 'D441_TRANSFER_STOP: no file is downloaded by this fixture'
}

# These names retain their real meaning in an isolated fixture process; restore them even on failure.
$oldArch = $env:PROCESSOR_ARCHITECTURE
$oldNativeArch = $env:PROCESSOR_ARCHITEW6432
$oldLocalData = $env:LOCALAPPDATA
try {
    $env:PROCESSOR_ARCHITECTURE = 'AMD64'
    $env:PROCESSOR_ARCHITEW6432 = 'AMD64'
    if (-not $env:LOCALAPPDATA) { $env:LOCALAPPDATA = Join-Path ([IO.Path]::GetTempPath()) 'D441-no-directory-is-created' }
    $cases = @(Get-Content -LiteralPath (Join-Path $PSScriptRoot 'addresses.tsv')) + @(
        "deny`thttp://localhost`nhttp://evil.example", "deny`thttp://localhost`rhttp://evil.example"
    )
    foreach ($line in $cases) {
        if (-not $line -or $line.StartsWith('#')) { continue }
        $fields = $line.Split([char]9)
        if ($fields.Count -ne 2) { throw 'the URL matrix needs expected status and one address per line' }
        $expected = $fields[0]
        $address = $fields[1]
        $script:addressRequests = @()
        $script:addressWrites = 0
        $script:addressRemovals = 0
        $previousError = [Console]::Error
        $captured = New-Object IO.StringWriter
        try {
            [Console]::SetError($captured)
            & $Installer -Url $address -Token 'fixture-not-a-real-token' -NoService | Out-Null
        } finally { [Console]::SetError($previousError) }
        $said = $captured.ToString()
        $captured.Dispose()
        if ($said.Contains('D441_FORBIDDEN')) { throw $said }
        if ($expected -eq 'allow') {
            if ($script:addressRequests.Count -ne 2 -or -not $said.Contains('D441_TRANSFER_STOP')) {
                throw "permitted address did not reach the manifest and asset download: $address; $said"
            }
            foreach ($request in $script:addressRequests) {
                if ($request.Redirects -ne 0 -or -not $request.Uri.StartsWith($address.TrimEnd('/') + '/api/v1/downloads/')) {
                    throw "download followed an unchecked redirect or changed origin: $address"
                }
            }
        } elseif ($expected -ne 'deny' -or $script:addressRequests.Count -ne 0 -or $script:addressWrites -ne 0 -or
                  $script:addressRemovals -ne 0 -or -not $said.Contains('127.0.0.0/8') -or -not $said.Contains('HTTPS')) {
            throw "address was not refused before downloads or files with a plain reason: $address; $said"
        }
    }
} finally {
    $env:PROCESSOR_ARCHITECTURE = $oldArch
    $env:PROCESSOR_ARCHITEW6432 = $oldNativeArch
    $env:LOCALAPPDATA = $oldLocalData
}
Write-Output 'PASS Windows installer: shared loopback address cases and redirect refusal; no real downloads, files or tasks'
