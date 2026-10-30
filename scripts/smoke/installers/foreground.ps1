# The command copied from Add machine must not run an old connector after installation fails.
param([string]$Installer = (Join-Path $PSScriptRoot '../../../deploy/install/install.ps1'))

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$install = [scriptblock]::Create((Get-Content -LiteralPath $Installer -Raw))

$started = $false
$failed = $false
try {
    & $install -Url 'http://not-loopback.example' -Token 'fixture-not-a-real-token' -NoService
    if ($?) { $started = $true }
} catch { $failed = $true }
if (-not $failed -or $started) { throw 'a failed installation must not reach the foreground command' }

$continued = $false
& $install -Help | Out-Null
if ($?) { $continued = $true }
if (-not $continued) { throw 'a successful invocation must still continue' }
Write-Output 'PASS Windows installer: failure stops the foreground command and success continues'
