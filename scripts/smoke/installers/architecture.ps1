# Exercise Windows architecture selection without real downloads, file placement or scheduled tasks: the connector built
# for the processor of the machine is the one requested, whichever builds the system lists, and nothing else is.
param([string]$Installer = (Join-Path $PSScriptRoot '../../../deploy/install/install.ps1'))
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$global:orchInstallerArchitectureFixture = @{ Requests = @(); Builds = @('aarch64', 'x86_64'); Mode = 'stop'; Download = ''; Moves = 0 }
function New-Item { }
function Remove-Item { }
function Test-Path { return $false }
function Move-Item {
    $global:orchInstallerArchitectureFixture.Moves++
    throw 'FIXTURE_PLACEMENT_STOP: no file is installed by this fixture'
}
function Get-Item {
    param([string]$LiteralPath)
    if ($LiteralPath -ne $global:orchInstallerArchitectureFixture.Download) { throw 'FIXTURE_FORBIDDEN unexpected file' }
    return [pscustomobject]@{ Length = 1 }
}
function Get-FileHash {
    param([string]$Algorithm, [string]$LiteralPath)
    if ($Algorithm -ne 'SHA256' -or $LiteralPath -ne $global:orchInstallerArchitectureFixture.Download) {
        throw 'FIXTURE_FORBIDDEN unexpected hash'
    }
    $hash = if ($global:orchInstallerArchitectureFixture.Mode -eq 'bad-hash') { 'f' * 64 } else { '0' * 64 }
    return [pscustomobject]@{ Hash = $hash }
}
function Register-ScheduledTask { throw 'FIXTURE_FORBIDDEN task registration' }
function Start-ScheduledTask { throw 'FIXTURE_FORBIDDEN task start' }
function Invoke-RestMethod {
    param([string]$Uri, [int]$MaximumRedirection = -1, [switch]$UseBasicParsing)
    $global:orchInstallerArchitectureFixture.Requests += [pscustomobject]@{ Uri = $Uri; Redirects = $MaximumRedirection }
    $assets = @($global:orchInstallerArchitectureFixture.Builds | ForEach-Object {
        [pscustomobject]@{ download_url = "/api/v1/downloads/connector/windows/$_"; size = 1; sha256 = ('0' * 64) }
    })
    return [pscustomobject]@{ project = 'orch'; version = 'fixture'; assets = $assets }
}
function Invoke-WebRequest {
    param([string]$Uri, [string]$OutFile, [int]$MaximumRedirection = -1, [switch]$UseBasicParsing)
    $global:orchInstallerArchitectureFixture.Requests += [pscustomobject]@{ Uri = $Uri; Redirects = $MaximumRedirection }
    $global:orchInstallerArchitectureFixture.Download = $OutFile
    if ($global:orchInstallerArchitectureFixture.Mode -eq 'stop') {
        throw 'FIXTURE_TRANSFER_STOP: no file is downloaded by this fixture'
    }
}

$oldArch = $env:PROCESSOR_ARCHITECTURE
$oldNativeArch = $env:PROCESSOR_ARCHITEW6432
$oldLocalData = $env:LOCALAPPDATA
$oldCulture = [Threading.Thread]::CurrentThread.CurrentUICulture
try {
    $env:LOCALAPPDATA = Join-Path ([IO.Path]::GetTempPath()) 'fixture-no-directory-is-created'
    # Windows on ARM64 has a connector of its own. A 32-bit shell on ARM64 says so by the native architecture; an x64
    # shell on ARM64 (emulated) and native x64 both say AMD64 and get the x86-64 connector.
    $cases = @(
        @{ Process = 'ARM64'; Native = ''; Builds = @('aarch64', 'x86_64'); Mode = 'stop'; Arch = 'aarch64' },
        @{ Process = 'x86'; Native = 'ARM64'; Builds = @('aarch64', 'x86_64'); Mode = 'stop'; Arch = 'aarch64' },
        @{ Process = 'AMD64'; Native = ''; Builds = @('aarch64', 'x86_64'); Mode = 'stop'; Arch = 'x86_64' },
        @{ Process = 'x86'; Native = 'AMD64'; Builds = @('aarch64', 'x86_64'); Mode = 'stop'; Arch = 'x86_64' },
        @{ Process = 'ARM64'; Native = ''; Builds = @('aarch64', 'x86_64'); Mode = 'good-hash'; Arch = 'aarch64' },
        @{ Process = 'ARM64'; Native = ''; Builds = @('aarch64', 'x86_64'); Mode = 'bad-hash'; Arch = 'aarch64' },
        @{ Process = 'AMD64'; Native = ''; Builds = @('aarch64', 'x86_64'); Mode = 'bad-hash'; Arch = 'x86_64' },
        # A build the system does not list is refused; the other one is never taken in its place.
        @{ Process = 'ARM64'; Native = ''; Builds = @('x86_64'); Mode = 'stop'; Arch = 'aarch64'; Refused = $true },
        @{ Process = 'AMD64'; Native = ''; Builds = @('aarch64'); Mode = 'stop'; Arch = 'x86_64'; Refused = $true }
    )
    foreach ($culture in @('en-US', 'zh-CN')) {
        [Threading.Thread]::CurrentThread.CurrentUICulture = [Globalization.CultureInfo]::GetCultureInfo($culture)
        foreach ($case in $cases) {
            $env:PROCESSOR_ARCHITECTURE = $case.Process
            $env:PROCESSOR_ARCHITEW6432 = $case.Native
            $global:orchInstallerArchitectureFixture.Requests = @()
            $global:orchInstallerArchitectureFixture.Builds = $case.Builds
            $global:orchInstallerArchitectureFixture.Mode = $case.Mode
            $global:orchInstallerArchitectureFixture.Download = ''
            $global:orchInstallerArchitectureFixture.Moves = 0
            $previousError = [Console]::Error
            $captured = New-Object IO.StringWriter
            $failure = $null
            try {
                [Console]::SetError($captured)
                try { & $Installer -Url 'https://orch.example' -Token 'fixture-not-a-real-token' -NoService | Out-Null }
                catch { $failure = $_.Exception.Message }
            } finally { [Console]::SetError($previousError) }
            $said = $captured.ToString()
            $captured.Dispose()
            $requests = $global:orchInstallerArchitectureFixture.Requests
            $label = "$culture $($case.Process) $($case.Native) $($case.Mode)"
            if ($case.ContainsKey('Refused')) {
                if (-not $failure -or -not $failure.Contains("no connector for Windows on $($case.Arch)") -or $requests.Count -ne 1) {
                    throw "the missing $($case.Arch) build was not refused: $label; $said"
                }
            } else {
                $reason = switch ($case.Mode) {
                    'stop' { 'FIXTURE_TRANSFER_STOP' }
                    'good-hash' { 'FIXTURE_PLACEMENT_STOP' }
                    'bad-hash' { 'does not have the SHA-256' }
                }
                if (-not $failure -or -not $failure.Contains($reason) -or $requests.Count -ne 2) {
                    throw "the selected connector was not requested: $label; $said"
                }
                if (($case.Mode -eq 'good-hash') -ne ($global:orchInstallerArchitectureFixture.Moves -eq 1)) {
                    throw "file placement did not follow checksum verification: $label; $said"
                }
                $wanted = "https://orch.example/api/v1/downloads/connector/windows/$($case.Arch)"
                if ($requests[1].Uri -cne $wanted) {
                    throw "the requested connector was not $($case.Arch): $($requests[1].Uri)"
                }
                if (-not $said.Contains("for Windows on $($case.Arch)")) {
                    throw "the build that is downloaded was not named: $label; $said"
                }
            }
            # The installer says nothing of emulation: the build it names is the one it takes.
            if ($said -match 'emulat' -or $said.Contains('模拟')) { throw "the installer speaks of emulation: $label; $said" }
            foreach ($request in $requests) {
                if ($request.Redirects -ne 0 -or ($request.Uri -match '/windows/[a-z0-9_]+$' -and -not $request.Uri.EndsWith("/windows/$($case.Arch)"))) {
                    throw "the installer widened its download target: $($request.Uri)"
                }
            }
        }
    }
} finally {
    $env:PROCESSOR_ARCHITECTURE = $oldArch
    $env:PROCESSOR_ARCHITEW6432 = $oldNativeArch
    $env:LOCALAPPDATA = $oldLocalData
    [Threading.Thread]::CurrentThread.CurrentUICulture = $oldCulture
    Remove-Variable -Scope Global -Name orchInstallerArchitectureFixture
}
Write-Output 'PASS Windows installer: the connector built for the architecture of the machine, native on ARM64, in both languages'
