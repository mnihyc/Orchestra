---
title: 连接一台机器
screens: /resources/machines, /resources/machines/$id, /resources/hosts
keywords: 连接, 电脑, 安装, 连接器, 节点, 用户服务, 系统服务, 前台, 多实例, 数据目录, connect, connector, node
heading_id.1-选择这台机器做什么: 1-choose-what-runs-here
heading_id.2-选择账号和运行方式: 2-choose-its-account-and-lifetime
heading_id.通过代理连接: behind-a-proxy
heading_id.3-执行命令等待机器出现: 3-connect-and-wait-for-its-name
heading_id.修改驱动设置: change-driver-settings
heading_id.4-重新登记同一台机器: 4-enroll-the-same-machine-again
heading_id.5-停止或完整移除: 5-stop-or-remove-it
---

# 连接一台机器

## 1. 选择这台机器做什么

打开**资源 → 机器 → 添加**，选择角色。**环境**使用你注册的文件夹，**代理**运行代理，
**提供方**保管模型凭据；这三种角色可共用一个连接器。**节点**是另一个程序，在 Linux
主机上创建受管环境。选择你有权添加机器的位置。连接机器并不等于共享它的全部文件夹。

## 2. 选择账号和运行方式

连接器以**你的账号**运行，无需 sudo 或管理员权限。Linux/macOS 的程序位于
`~/.local/bin/orch-connector`，Windows 位于 `%LOCALAPPDATA%\Orch\orch-connector.exe`。
数据在 Linux 的 `~/.local/share/orch-connector`（遵循 `XDG_DATA_HOME`）、macOS 的
`~/Library/Application Support/Orch/connector` 或 Windows 的 `%LOCALAPPDATA%\Orch\connector`。

**用户服务**使用 Linux 的 systemd、macOS 的 launchd 或 Windows 的登录任务。
Linux 未启用 lingering 时会随退出登录停止；macOS 和 Windows 在登录期间运行。
对话框的**前台运行**命令不安装服务，安装后直接在当前终端启动程序，按 Ctrl+C 停止。
已有服务需先停止，不能让它和前台程序同时使用同一份身份。

同一台机器可以运行多个连接器。填写**另一个实例的名称**，为它分配独立的身份、数据和
服务；命令会带上 `--data-dir`，Windows 安装程序使用 `-DataDir`。该实例后续的命令
也要使用相同路径。多个实例共用程序文件。

节点以 **root** 运行，每台机器一个。程序为 `/usr/local/bin/orch-node`，配置为
`/etc/orch/orch.toml`，数据为 `/var/lib/orch-node`。**系统服务**使用 `orch-node.service`；
选择**前台运行**则会以 root 在当前终端启动。通过主机的**访问**页面分享给其他组织，
不要安装第二个节点。

### 通过代理连接

连接器和节点通过机器的代理连接系统以及它们转发到的模型服务：读取 `HTTPS_PROXY`、`HTTP_PROXY`、
`ALL_PROXY` 和 `NO_PROXY` 变量（大小写均可）；在 macOS 或 Windows 上若未设置这些变量，则使用系统的代理
设置。支持 `http://`、`socks5://` 和 `socks5h://` 代理（`socks5h` 由代理解析域名），代理需要认证时可在
地址中写 `用户名:密码@`。

服务启动时使用服务管理器的变量，而不是你的 shell 的变量：`orch-connector service install` 会把运行它的
shell 中的代理变量写进服务，连接器启动的程序（智能体程序、`git`）也会继承这些变量。要使用其他代理，
安装时加上 `--proxy socks5h://proxy.example:1080`（或 `--proxy none` 直接连接所有主机），并用 `--no-proxy`
指定直接连接的主机；重新安装即可修改。Windows 的登录任务使用你账号的变量，请用 `setx` 设置。
`orch-connector doctor` 会显示连接网关每个地址时经过的代理。

## 3. 执行命令，等待机器出现

只交出希望 Orch 管理的角色设置，本机保留的设置始终优先。当电脑专供 Orch 使用、可作为
隔离边界时，启用**完全控制**。选择**显示命令**，复制对应操作系统的完整命令，在那台电脑上执行。
一次性令牌有有效期，过期后重新生成命令。首次连接成功后，Orch 会提示已连接，关闭
对话框并自动打开机器页面。只完成登记、尚未在线时会继续等待。

若对话框提示 `public_url` 或 `[[gateway.endpoints]]`，请系统管理员替换该设置中的示例地址。
地址配置完成前不会显示命令。人和安装程序使用浏览器打开的地址；机器优先使用本机配置的地址列表，
否则使用每次连接时收到的系统列表。环境默认跟随节点的列表，节点配置也可为环境指定其他地址。

浏览器和安装程序需要设备信任服务端证书。机器还信任 Orch 自身证书中标明的 Orch 服务，
不受所用地址名称影响。使用公司证书颁发机构时，请管理员将其证书安装到设备的信任库，保留证书校验。

安装程序仅允许 `127.0.0.0/8`、`[::1]` 或 `localhost` 使用 HTTP 系统地址。
它们指向执行命令的那台电脑；其他电脑应使用系统的 HTTPS 地址。节点的例外只适用于
系统下载地址；节点的网关连接仍使用 TLS。

## 修改驱动设置

在节点的**策略**页，机器运维人员交出**驱动**设置后，可以打开并编辑其 JSON。对于已报告
配置架构的驱动，错误字段会显示在值旁；修正后再次保存即可。编辑器会保留草稿。如果驱动
尚未报告架构，Orch 无法在此检查其连接字段。连接修改会在节点重启后生效。

## 4. 重新登记同一台机器

管理员操作他人的机器前，需先确认**强制获取访问权限**。Orch 会将其姓名加入机器的**管理**权限行，
并在审计中记录此次强制授权。这需要浏览器会话开启**以管理员身份**；令牌或关闭该模式的会话不会获得
管理员权限。加入的权限会保留，直到从**访问**中移除。更改角色或设置时，机器在本地保留的策略仍然生效。

若证书已撤销，或机器离线期间证书过期且已超过续期宽限期，打开机器页面，选择**重新登记**。
仍在宽限期内的证书会在机器下次连接时自动续期。

在原机器上使用原数据目录执行对话框中的命令。命令就地替换身份，保留本机设置、策略、注册内容、
名称和页面。正在前台运行的实例会载入新身份；已停止的实例下次启动时使用它。
对话框会显示同一台机器已返回。新证书首次使用时，旧证书立即失效。

磁盘丢失时，在替代机器的空数据目录中执行同一命令，Orch 仍保留原机器页面。
**添加机器**或**添加主机**的命令不能替换已有身份；请在对应机器页面选择**重新登记**。

## 5. 停止或完整移除

机器的**安装方式**部分显示实际路径和服务命令。默认连接器使用
`orch-connector service stop`、`service start` 或 `service uninstall`；命名实例需在子命令前
加相同的 `--data-dir`。Windows 通过 `&` 调用其程序。节点使用
`systemctl stop orch-node.service` 或 `systemctl start orch-node.service`。

完整移除连接器时，先在 Orch 撤销机器，再卸载服务。确认后只删除该实例的数据，保留
工作文件夹及其他实例，包括子目录中的实例；最后一个实例移除后再删除共享程序。
节点需先在 Orch 清理环境和卷并移除主机，再禁用、移除服务，最后清理程序和数据。
其他 Orch 程序仍使用的配置应保留。仅移除服务会保留身份和数据。
自动更新及本地更新命令见[更新说明](../updates.md)。
