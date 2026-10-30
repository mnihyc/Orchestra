# Exercise Windows architecture selection without real downloads, file placement or scheduled tasks.
param([string]$Installer = (Join-Path $PSScriptRoot '../../../deploy/install/install.ps1'))
Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

$global:orchInstallerArchitectureFixture = @{ Requests = @(); HasX64 = $true; Mode = 'stop'; Download = ''; Moves = 0 }
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
    $assets = @([pscustomobject]@{ download_url = '/api/v1/downloads/connector/windows/aarch64'; size = 1; sha256 = ('0' * 64) })
    if ($global:orchInstallerArchitectureFixture.HasX64) {
        $assets += [pscustomobject]@{ download_url = '/api/v1/downloads/connector/windows/x86_64'; size = 1; sha256 = ('0' * 64) }
    }
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
    $cases = @(
        @{ Process = 'ARM64'; Native = ''; HasX64 = $true; Mode = 'stop' },
        @{ Process = 'x86'; Native = 'ARM64'; HasX64 = $true; Mode = 'stop' },
        # Native x64 and x64 PowerShell emulated on ARM may both say AMD64.
        @{ Process = 'AMD64'; Native = ''; HasX64 = $true; Mode = 'stop' },
        @{ Process = 'x86'; Native = 'AMD64'; HasX64 = $true; Mode = 'stop' },
        @{ Process = 'ARM64'; Native = ''; HasX64 = $false; Mode = 'stop' },
        @{ Process = 'ARM64'; Native = ''; HasX64 = $true; Mode = 'good-hash' },
        @{ Process = 'ARM64'; Native = ''; HasX64 = $true; Mode = 'bad-hash' }
    )
    foreach ($culture in @('en-US', 'zh-CN')) {
        [Threading.Thread]::CurrentThread.CurrentUICulture = [Globalization.CultureInfo]::GetCultureInfo($culture)
        foreach ($case in $cases) {
            $env:PROCESSOR_ARCHITECTURE = $case.Process
            $env:PROCESSOR_ARCHITEW6432 = $case.Native
            $global:orchInstallerArchitectureFixture.Requests = @()
            $global:orchInstallerArchitectureFixture.HasX64 = $case.HasX64
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
            if ($case.HasX64) {
                $reason = switch ($case.Mode) {
                    'stop' { 'FIXTURE_TRANSFER_STOP' }
                    'good-hash' { 'FIXTURE_PLACEMENT_STOP' }
                    'bad-hash' { 'does not have the SHA-256' }
                }
                if (-not $failure -or -not $failure.Contains($reason) -or $requests.Count -ne 2) {
                    throw "the selected connector was not requested: $culture $($case.Process) $($case.Native); $said"
                }
                if (($case.Mode -eq 'good-hash') -ne ($global:orchInstallerArchitectureFixture.Moves -eq 1)) {
                    throw "file placement did not follow checksum verification: $culture $($case.Mode); $said"
                }
                if ($requests[1].Uri -cne 'https://orch.example/api/v1/downloads/connector/windows/x86_64') {
                    throw "the requested connector was not x86-64: $($requests[1].Uri)"
                }
                $expected = if ($culture -eq 'zh-CN') {
                    '正在安装 x86-64 连接器；Windows 11 在 ARM64 上通过模拟运行它。'
                } else {
                    'Installing the x86-64 connector; Windows 11 on ARM64 runs it through emulation.'
                }
                $notice = @($said -split "`r?`n" | Where-Object { $_ -ceq $expected })
                if ($notice.Count -ne 1) { throw "the emulation notice must appear once in $culture; $said" }
            } else {
                if (-not $failure -or -not $failure.Contains('no connector for Windows on x86_64') -or $requests.Count -ne 1) {
                    throw "the missing x86-64 build was not refused: $culture; $said"
                }
            }
            foreach ($request in $requests) {
                if ($request.Redirects -ne 0 -or $request.Uri.Contains('/windows/aarch64')) {
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
Write-Output 'PASS Windows installer: x86-64 selection and localized emulation notice for ARM64 and x64 shells'
