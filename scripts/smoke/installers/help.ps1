# Offline Windows help must not prompt for a token, fetch files or register a logon task.
# Run only by the integrator, on Windows PowerShell 5.1 or PowerShell 7.
param([string]$Installer = (Join-Path $PSScriptRoot '../../../deploy/install/install.ps1'))

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'

function Invoke-WebRequest { throw 'help attempted a download' }
function Invoke-RestMethod { throw 'help attempted a manifest lookup' }
function New-Item { throw 'help attempted to create a file or directory' }
function Register-ScheduledTask { throw 'help attempted to register a task' }
function Start-ScheduledTask { throw 'help attempted to start a task' }

$fullHelp = & $Installer -Help | Out-String
$shortHelp = & $Installer -h | Out-String
if ([string]::IsNullOrWhiteSpace($fullHelp) -or $fullHelp -ne $shortHelp) {
    throw 'the help aliases must return the same nonempty help without installation arguments'
}
foreach ($word in @('%LOCALAPPDATA%\Orch\orch-connector.exe', '%LOCALAPPDATA%\Orch\connector',
                    '-DataDir', '--data-dir', '-NoService', 'service stop', 'service uninstall', 'Ctrl+C')) {
    if (-not $fullHelp.Contains($word)) { throw "installer help is missing: $word" }
}
Write-Output 'PASS Windows installer help: both aliases, without credentials, downloads, files or tasks'
