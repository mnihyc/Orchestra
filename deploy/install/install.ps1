# Installs the Orch connector for the person who runs it, on Windows.
#
#   & ([scriptblock]::Create((iwr 'https://orch.example.com/api/v1/downloads/install.ps1' -UseBasicParsing))) `
#     -Url 'https://orch.example.com' -Token 'oenr_….<pin>' [-Name NAME] [-DataDir DIR] `
#     [-Roles 'environment,agent'] [-Delegate 'environment,agent' | -DelegatePolicy] [-Dedicated] [-NoService]
#
# It fetches the connector for this processor from the Orch system named by -Url, checks its size
# and SHA-256 against the system's release manifest, places it in %LOCALAPPDATA%\Orch, enrolls this
# machine with the token, hands every limit over to Orch with -DelegatePolicy, and keeps the
# connector running as the Scheduled Task "Orch Connector" (at logon, started again when it fails).
# It never needs administrator rights. Run again, it changes only what is not right yet: the same
# connector is left in place, a bound Enroll again token replaces its retained identity, and the task is registered again and started. When it fails, it takes away what it placed
# itself. It never closes the window it runs in: a failure is said, and the script returns.
param(
    [string]$Url = '',
    [string]$Token = '',
    [string]$Name = '',
    [string]$Roles = '',
    [string]$Delegate = '',
    [string]$DataDir = '',
    [switch]$DelegatePolicy,
    [switch]$Dedicated,
    [switch]$NoService,
    [Alias('h')][switch]$Help
)

# Help returns before validation, downloads, files or task registration, including when pasted remotely.
if ($Help) {
    if ($PSUICulture -like 'zh*') {
        @'
为你的 Windows 账号安装 Orch 连接器并注册这台机器；无需管理员权限。

  .\install.ps1 -Url <系统地址> -Token <注册令牌>
                 [-Name <名称>] [-DataDir <绝对路径>] [-Roles environment,agent]
                 [-Delegate environment,agent | -DelegatePolicy] [-Dedicated] [-NoService]

程序：%LOCALAPPDATA%\Orch\orch-connector.exe；数据：%LOCALAPPDATA%\Orch\connector。
默认作为登录任务“Orch Connector”以你的账号运行；退出登录后停止。
停止（启动时改为 start）：& "$env:LOCALAPPDATA\Orch\orch-connector.exe" service stop
前台运行：安装时加 -NoService，然后运行
  & "$env:LOCALAPPDATA\Orch\orch-connector.exe" run
按 Ctrl+C 停止；已有登录任务需先停止。
同一台机器可运行多个连接器；另加 -DataDir <绝对路径>，各有独立的身份和登录任务。
命名实例的后续命令都需在子命令前使用相同的 --data-dir <绝对路径>。
卸载：先预览计划，再输入机器名称确认：
  & "$env:LOCALAPPDATA\Orch\orch-connector.exe" uninstall
加 --purge 同时移除数据和它创建的目录；已有工作目录不删除。
加 --plan 只预览；--yes 跳过输入名称。最后一个实例在命令退出后删除共享程序。

  -Url              系统地址：HTTPS；HTTP 仅限 127.0.0.0/8、[::1] 或 localhost
  -Token            系统显示的一次性注册令牌（oenr_…）
  -Name             机器在系统中的名称；省略时使用主机名
  -Roles            environment、agent、provider，使用逗号分隔
  -Delegate         将选中角色的设置交给 Orch；本机保留值仍然优先
  -DataDir          数据目录；指定路径的实例使用独立登录任务
  -DelegatePolicy   将所有设置交给 Orch；本机保留值仍然优先
  -Dedicated        声明这台机器专供 Orch 使用，作为隔离边界
  -NoService        仅安装和注册；不创建登录任务，随后自行前台运行
  -Help, -h         显示说明；不需要令牌，不下载或安装
'@
    } else {
        @'
Installs an Orch connector for your Windows account; no administrator rights are needed.

  .\install.ps1 -Url <system address> -Token <enrollment token>
                 [-Name <name>] [-DataDir <absolute path>] [-Roles environment,agent]
                 [-Delegate environment,agent | -DelegatePolicy] [-Dedicated] [-NoService]

Program: %LOCALAPPDATA%\Orch\orch-connector.exe; data: %LOCALAPPDATA%\Orch\connector.
Default: the logon task 'Orch Connector', running as your account until you log out.
Stop (or use start): & "$env:LOCALAPPDATA\Orch\orch-connector.exe" service stop
Foreground: install with -NoService, then run
  & "$env:LOCALAPPDATA\Orch\orch-connector.exe" run
Ctrl+C stops it; stop any existing logon task first.
Several connectors can run side by side: add -DataDir <absolute path> for another
identity and task; use --data-dir <same path> before subcommands on its later commands.
Uninstall: read its plan, then type the machine's name:
  & "$env:LOCALAPPDATA\Orch\orch-connector.exe" uninstall
Add --purge to remove data and folders it made. Existing work folders stay.
Use --plan to preview or --yes to skip typing. The last instance schedules program removal after exit.
Uninstall without --purge keeps data and identity; nested instances block their parent's purge.

  -Url              HTTPS system address; HTTP only on 127.0.0.0/8, [::1] or localhost
  -Token            the one-use enrollment token the system showed (oenr_…)
  -Name             the machine's name in Orch (the host name if left out)
  -Roles            environment,agent,provider, separated by commas
  -Delegate         hand the selected roles' settings to Orch; locally kept values still win
  -DataDir          another data directory, identity and task (platform default if left out)
  -DelegatePolicy   hand every setting to Orch; locally kept values still win
  -Dedicated        declare this machine dedicated to Orch as its isolation boundary
  -NoService        install and enroll without a task; start it yourself in the foreground
  -Help, -h         show this help without a token, download or installation
'@
    }
    return
}

Set-StrictMode -Version 2.0
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

function Say([string]$Text) { [Console]::Error.WriteLine($Text) }

function Test-SystemAddress([string]$Address) {
    if ($Address -match '[\s\\?#]') { return $false }
    $literal = [regex]::Match($Address, '\Ahttps?://([^/]+)(?:/.*)?\z', [Text.RegularExpressions.RegexOptions]::IgnoreCase)
    if (-not $literal.Success) { return $false }
    $authority = $literal.Groups[1].Value
    if ($authority -match '[@%]') { return $false }
    [Uri]$parsed = $null
    if (-not [Uri]::TryCreate($Address, [UriKind]::Absolute, [ref]$parsed)) { return $false }
    if ($parsed.Scheme -eq 'https') { return $true }
    # Check the literal host too: Uri normalizes shorthand and integer IPv4 addresses.
    $v6 = [regex]::Match($authority, '\A\[([0-9a-fA-F:]+)\](?::[0-9]+)?\z')
    [Net.IPAddress]$ip = $null
    if ($v6.Success) {
        return [Net.IPAddress]::TryParse($v6.Groups[1].Value, [ref]$ip) -and $ip.Equals([Net.IPAddress]::IPv6Loopback)
    }
    $v4 = [regex]::Match($authority, '\A([^:\[\]]+)(?::[0-9]+)?\z')
    if (-not $v4.Success) { return $false }
    $hostName = $v4.Groups[1].Value
    if ($hostName -ieq 'localhost') { return $true }
    if ($hostName -notmatch '\A127(?:\.(?:0|[1-9][0-9]{0,2})){3}\z') { return $false }
    return [Net.IPAddress]::TryParse($hostName, [ref]$ip) -and [Net.IPAddress]::IsLoopback($ip)
}

function Get-SystemAddressError {
    if ($PSUICulture -like 'zh*') {
        return '-Url 需要 HTTPS；HTTP 仅限回环主机 127.0.0.0/8、::1 或 localhost。其他主机上的 HTTP 会明文传输注册与下载，请改用 HTTPS；地址不能包含账号、查询参数或片段。'
    }
    return '-Url needs HTTPS; HTTP is allowed only on loopback hosts (127.0.0.0/8, ::1 or localhost). HTTP elsewhere exposes enrollment and downloads; use HTTPS. Omit userinfo, a query or a fragment from the base address.'
}

function Get-Sha256Hex([string]$Path) {
    (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

$Placed = @{ Binary = $false; Task = $null }
$Work = $null
try {
    if (-not $Url) { throw '-Url is needed: the address of the Orch system (see -Help)' }
    if (-not $Token) { throw '-Token is needed: the enrollment token the system showed (see -Help)' }
    if ($DelegatePolicy -and $Delegate) { throw 'use -Delegate or -DelegatePolicy, not both' }
    if ($Roles.Split(',') -contains 'node') { throw 'The node role is hosted by orch-node: add this computer as a host as well.' }
    $Url = $Url.TrimEnd('/')
    if (-not (Test-SystemAddress $Url)) { throw (Get-SystemAddressError) }

    $machine = $env:PROCESSOR_ARCHITEW6432
    if (-not $machine) { $machine = $env:PROCESSOR_ARCHITECTURE }
    switch ($machine) {
        'AMD64' { $Arch = 'x86_64' }
        'ARM64' { $Arch = 'aarch64' }
        default { throw "no connector is made for a $machine processor" }
    }
    if (-not $env:LOCALAPPDATA) { throw 'LOCALAPPDATA is not set: the connector is installed for a person, in their profile' }

    # Windows PowerShell 5.1 speaks TLS 1.2 only when asked to.
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $BinDir = Join-Path $env:LOCALAPPDATA 'Orch'
    $Bin = Join-Path $BinDir 'orch-connector.exe'
    $NamedDataDir = [bool]$DataDir
    if (-not $DataDir) { $DataDir = Join-Path $env:LOCALAPPDATA 'Orch\connector' }

    $Work = Join-Path ([IO.Path]::GetTempPath()) ('orch-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $Work | Out-Null

    Say "Asking $Url for its release"
    try {
        # A cleartext loopback origin must not redirect the request to cleartext elsewhere.
        $manifest = Invoke-RestMethod -Uri "$Url/api/v1/downloads/version.json" -MaximumRedirection 0 -UseBasicParsing
    } catch {
        throw "$Url/api/v1/downloads/version.json could not be fetched: is $Url the system's address, and was it started from a release? ($($_.Exception.Message))"
    }
    if ($manifest.project -ne 'orch') { throw "$Url did not answer with the release manifest of an Orch system" }
    $wanted = "/api/v1/downloads/connector/windows/$Arch"
    $asset = @($manifest.assets | Where-Object { $_.download_url -eq $wanted }) | Select-Object -First 1
    if (-not $asset) {
        throw "release $($manifest.version) of this system has no connector for Windows on $Arch; its operator can add one"
    }
    if (-not ($asset.PSObject.Properties.Name -contains 'sha256') -or -not $asset.sha256) {
        throw "release $($manifest.version) lists no SHA-256 for the connector, which this installer checks; it is older than this installer"
    }

    Say "Downloading the connector $($manifest.version) for Windows on $Arch"
    $download = Join-Path $Work 'orch-connector.exe'
    Invoke-WebRequest -Uri "$Url$wanted" -OutFile $download -MaximumRedirection 0 -UseBasicParsing
    $size = (Get-Item -LiteralPath $download).Length
    if ($size -ne [int64]$asset.size) {
        throw "the download is $size bytes and the manifest lists $($asset.size): it was cut or changed on the way"
    }
    if ((Get-Sha256Hex $download) -ne $asset.sha256) {
        throw 'the download does not have the SHA-256 the manifest lists: it was damaged or changed on the way'
    }
    Say "  checked: $size bytes, SHA-256 as listed"

    # A running program's file cannot be written over on Windows, but it can be renamed: the old
    # one steps aside and the new one takes its name.
    New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
    if ((Test-Path -LiteralPath $Bin) -and ((Get-Sha256Hex $Bin) -eq $asset.sha256)) {
        Say "The connector $($manifest.version) is in $Bin already"
    } else {
        if (Test-Path -LiteralPath $Bin) {
            Move-Item -LiteralPath $Bin -Destination "$Bin.old" -Force
        } else {
            $Placed.Binary = $true
        }
        Move-Item -LiteralPath $download -Destination $Bin -Force
        Remove-Item -LiteralPath "$Bin.old" -Force -ErrorAction SilentlyContinue
        Say "Placed the connector $($manifest.version) in $Bin"
    }

    $arguments = @('--data-dir', $DataDir, 'enroll', '--url', $Url, '--token', $Token)
    if ($Name) { $arguments += @('--name', $Name) }
    if ($Dedicated) { $arguments += '--dedicated' }
    if ($Roles) { $arguments += @('--roles', $Roles) }
    if ($Delegate) { $arguments += @('--delegate', $Delegate) }
    if ($DelegatePolicy) { $arguments += '--delegate-policy' }
    & $Bin @arguments
    if ($LASTEXITCODE -ne 0) {
        throw 'enrolling did not succeed: the token is used once and runs out; make another in Orch if it was used'
    }
    if ($Dedicated) { & $Bin --data-dir $DataDir dedicated true; if ($LASTEXITCODE -ne 0) { throw 'the dedicated-machine declaration was not saved' } }
    # An enrolled machine keeps its program, whatever happens next.
    $Placed.Binary = $false

    if ($DelegatePolicy) {
        & $Bin --data-dir $DataDir policy delegate all | Out-Null
        if ($LASTEXITCODE -ne 0) { throw 'the limits could not be handed over to Orch' }
        Say 'Every limit of this machine can now be changed from Orch'
    }
    if ($Delegate) {
        foreach ($section in $Delegate.Split(',')) {
            switch ($section.Trim()) {
                'all' { $key = '*' }
                '*' { $key = '*' }
                { $_ -in @('environment','agent','provider') } { $key = "$($_).*" }
                { $_ -in @('environment.*','agent.*','provider.*') } { $key = $_ }
                default { throw '-Delegate names environment, agent or provider, or all' }
            }
            & $Bin --data-dir $DataDir policy delegate $key | Out-Null
            if ($LASTEXITCODE -ne 0) { throw 'the selected role section could not be handed over' }
        }
    }

    $receiptArguments = @('--data-dir', $DataDir, '__record-installation')
    if ($NamedDataDir) { $receiptArguments += '--named' }
    & $Bin @receiptArguments
    if ($LASTEXITCODE -ne 0) { throw 'the installation ownership record could not be saved' }
    if ($NoService) {
        Say "No task was registered (-NoService). Start the connector with: & '$Bin' --data-dir '$DataDir' run"
    } else {
        # One task for the platform's own data directory, and one for each other directory.
        $TaskName = 'Orch Connector'
        if ($NamedDataDir) {
            $digest = [Security.Cryptography.SHA256]::Create().ComputeHash([Text.Encoding]::UTF8.GetBytes($DataDir))
            $TaskName = 'Orch Connector ' + ([BitConverter]::ToString($digest).Replace('-', '').Substring(0, 8).ToLowerInvariant())
        }
        $user = if ($env:USERDOMAIN) { "$env:USERDOMAIN\$env:USERNAME" } else { $env:USERNAME }
        $existing = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if ($existing) {
            Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        } else {
            $Placed.Task = $TaskName
        }
        $action = New-ScheduledTaskAction -Execute $Bin -Argument ('--data-dir "' + $DataDir + '" run') -WorkingDirectory $BinDir
        $trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
        $settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
            -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
            -StartWhenAvailable -MultipleInstances IgnoreNew
        $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
        Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings `
            -Principal $principal -Description "Orch connector ($DataDir)" -Force | Out-Null
        & $Bin @receiptArguments --service
        if ($LASTEXITCODE -ne 0) { throw 'the service ownership record could not be saved' }
        Start-ScheduledTask -TaskName $TaskName
        $Placed.Task = $null
        Say "The connector runs as the Scheduled Task '$TaskName', started at every logon"
    }
    Say "Done: this machine is connected to $Url. Its page in Orch shows it within a minute."
} catch {
    Say "error: $($_.Exception.Message)"
    if ($Placed.Task) { Unregister-ScheduledTask -TaskName $Placed.Task -Confirm:$false -ErrorAction SilentlyContinue }
    if ($Placed.Binary) { Remove-Item -LiteralPath $Bin -Force -ErrorAction SilentlyContinue }
    if ($Placed.Task -or $Placed.Binary) { Say 'what this run placed was taken away again' }
} finally {
    if ($null -ne $Work) { Remove-Item -LiteralPath $Work -Recurse -Force -ErrorAction SilentlyContinue }
}
