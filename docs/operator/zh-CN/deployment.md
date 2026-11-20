# 安装 Orch

[English](../deployment.md)

在一台 Linux Docker 主机上安装 Orch：从发布文件到运行中的系统和第一位管理员。完成后，你有一套可通过某个地址访问的服务、一位能登录的管理员，以及一份保存在主机之外的密钥副本。

各个组件是什么、如何配合，见[部署目录说明](../../../deploy/README.md)。

## 开始之前

你需要：

- **一台 Linux 主机**，x86_64 或 ARM64，装有 Docker Engine 及其 Compose 插件（2 版）、`bash`、GNU coreutils、`flock` 和 `curl`。`openssl` 用来生成数据库密码。`unzip` 和 `b3sum` 可有可无：没有它们时，更新程序在容器中解压服务器压缩包，由 `orchctl` 校验校验和。
- **有 root 权限的 Docker，且不做用户命名空间重映射。** 服务的数字用户 ID 必须与主机上挂载目录的所有者一致。无 root 的 Docker 需要另一套安排，不能使用随附的默认配置。
- **主机的资源。** 推荐的估算值，不是限制（这套服务没有设置任何限制）：自己或和一位同事试用，2 个 CPU 核心、4 GB 内存、30 GB 磁盘；约 20 人以内的团队，4 核、8 GB、100 GB SSD。你运行的每个环境都是其上的一个容器。默认环境镜像约 1.6 GiB，运行时其中的浏览器需要内存。这些估算依据的是这套服务的组成，而不是对繁忙系统的测量。
- **镜像仓库。** 首次启动会拉取三个标准镜像（`postgres:17-alpine`、`debian:bookworm-slim` 和 `nginx:1.30-alpine`），并用第一个构建数据库镜像（加入 pgvector 和 pg_bigm）。主机无法访问镜像仓库时，按[无外网环境](../offline.md)操作。
- **供网页应用使用的 HTTPS 地址**，仅在本机试用时可不要。Orch 自带的网页入口在主机上只提供明文 HTTP（`127.0.0.1:5180`），需要你在前面放一个对人提供 HTTPS 的前置代理：[HTTPS 前端](../edge.md)说明前置代理必须做什么。仅在本机试用时，使用 `http://127.0.0.1:5180`。
- **一个开放端口**，默认 9443，供机器和托管环境连接网关。

只运行一个 API 进程。终端、浏览器和显示的票据及其隧道属于签发它们的进程，因此第二个 API 进程需要自己的路由。

## 1. 获取发布文件

一个发布版本是一个目录，里面有若干文件和一个 `version.json`，后者列出每个文件的大小和 BLAKE3 校验和。选择一种方式。

**文件放在一个目录里。** 从[发布页面](https://github.com/mnihyc/Orchestra/releases/latest)把下列五个文件下载到同一个目录。`ARCH` 是 Docker 主机处理器的架构，`x86_64` 或 `aarch64`：

```sh
TAG=v0.1.3        # 要安装的版本，带 "v"
ARCH=$(uname -m)  # x86_64 或 aarch64
mkdir -p ~/orch-release && cd ~/orch-release
for f in version.json orch-deploy-$TAG.tar.gz orch-frontend-dist.tar.gz orch-server-linux-$ARCH.zip orchctl-linux-$ARCH-musl; do
  curl -fLO "https://github.com/mnihyc/Orchestra/releases/download/$TAG/$f"
done
```

无法访问 GitHub 的主机可以用磁盘接收这些文件。

**从网上获取。** 只下载 `orch-deploy-$TAG.tar.gz`。`update.sh` 按 `.env` 的设置，从本项目的 GitHub 发布页取其余文件。

## 2. 解压

把部署包解压到系统要放的位置，然后进入它的 `deploy` 目录：

```sh
mkdir -p ~/orch
tar -xzf orch-deploy-$TAG.tar.gz -C ~/orch     # 在部署包所在的目录运行
cd ~/orch/deploy
```

## 3. 配置

1. 生成 `.env` 并写入数据库密码：

   ```sh
   install -m 600 .env.example .env
   sed -i "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$(openssl rand -base64 24 | tr '+/' '-_')/" .env
   ```

   密码至少 24 个字符，只含字母、数字、`-` 和 `_`。`update.sh` 会拒绝过短的密码、含其他字符的密码（它会写进 URL）和占位符。让 `.env` 只有所有者可读。
2. 在 `config/orch.toml` 的 `[system]` 中，把 `public_url` 设为人们打开的 HTTPS 地址（仅试用时设为 `"http://127.0.0.1:5180"`）。
3. 在同一个文件中，把 `[[gateway.endpoints]]` 里的示例地址换成机器能在网关端口上访问、且在服务内部也能解析的地址。示例地址还在时，**添加机器**不会给出命令，环境也无法连接；随附的节点不受影响，可以上线。地址见 [HTTPS 前端](../edge.md)，`.env` 的其他设置见[配置](../configuration.md#set-the-stacks-env)。

## 4. 启动

```sh
./update.sh first-start --from ~/orch-release   # 文件放在一个目录里
./update.sh first-start $TAG                    # 从网上获取
```

`update.sh` 先用 `version.json` 校验每个文件，通过后才安装任何东西。随后它把这台主机的信息写入 `.env`（服务运行所用的用户和组、Docker 组、这个目录），把服务器、网页应用和 `orchctl` 解压到 `runtime/` 下，生成密钥，启动 PostgreSQL，做第一次备份，启动整套服务，并等待 `orchctl doctor` 通过。结束时它会给出要打开的地址：

```
ok    secrets          11 password and secret files are accessible by their owner alone
ok    config           read /etc/orch/orch.toml: Orch at https://orch.example.com
ok    database         connected to postgres:5432/orch (PostgreSQL 17); 1 migrations applied
ok    api              GET https://127.0.0.1:8443/health -> 200; /api/v1/meta -> release 0.1.3
ok    gateway          heartbeat 5 s ago, release 0.1.3: the backplane to the API is connected
ok    worker           heartbeat 5 s ago, release 0.1.3: alive
wait  node             no host yet, and no one has signed up: the bundled node enrolls itself as soon as the first administrator exists
all 7 checks passed
ok    edge-page        GET / -> 200 text/html, with the content policy and the four other security headers
ok    edge-api         GET /health -> 200; GET /api/v1/meta -> 200, first administrator still to make: yes
ok    edge-link        upgraded /link/v1/session: the gateway answers with the subprotocol orch-link.v1
all 3 checks passed

Orch v0.1.3 is running.
  Open https://orch.example.com and make the first administrator at /setup.
```

输出是英文，地址和数字因你的环境而异。某一步失败时，`update.sh` 会说明原因并保留它的日志。再运行同一条命令，它会从中断处继续。`docker compose logs <服务名>` 显示某个服务一侧的记录。

## 5. 确认可以使用

1. 打开这个地址。还没有任何人的系统会显示 `/setup`：在这里创建第一位系统管理员，完成后你已登录。随附的设置是凭邀请注册；其他设置在“管理”中填写。
2. 随附的节点要等上一步完成后才自行登记，最多需要一两分钟。检查：

   ```sh
   docker compose exec api /opt/orchctl/orchctl doctor
   ```

   `node` 这一行会从 `wait` 变为 `ok`。每一行的含义见[监控](../monitoring.md)。
3. 如果希望人们通过邮件收到登录验证码、地址确认和邀请，请设置邮件。打开“管理 → 电子邮件和渠道 → 新建渠道…”，选择“Email”，填写 Server 和 Sender（这些字段名在表单里以英文显示）。常用端口是 587（Security 选 STARTTLS）和 465（Security 选 TLS）。再填写发信账号的 User name 和 Password（密码入库时用主密钥加密保存），并在依赖它之前点“测试”发一封样信。要在命令行里做同样的事，把各字段写成一个 JSON 对象，放在 `runtime/data/mail.json`，api 容器里的路径是 `/var/lib/orch/mail.json`：

   ```json
   {"host": "smtp.example.org", "port": "587", "security": "starttls", "username": "orch", "password": "app-password", "from": "Orch <orch@example.org>"}
   ```

   每个值都是文本。只有 `host` 和 `from` 必填；其余的键是 `auth`、`reply_to`、`tls_ca` 和 `timeout_secs`。创建渠道、发一封样信，再删掉这个含密码的文件：

   ```sh
   docker compose exec api /opt/orchctl/orchctl channel put email --file /var/lib/orch/mail.json
   docker compose exec api /opt/orchctl/orchctl channel test email --to you@example.org
   rm runtime/data/mail.json
   ```

## 6. 保管密钥

**现在就把 `config/` 复制到主机之外。** 其中有全部密钥，丢失 `orch_master_key` 或 `orch_ca_key.pem` 无法挽回。见[备份与恢复](../backup-and-restore.md)。

## 安装留下了什么

这些都归运行 `first-start` 的用户所有，因此复制、移动或删除都不需要特权。

| 路径 | 内容 |
|---|---|
| `.env`、`config/orch.toml` | 你的设置。更新会保留你的值，并让 `.env` 保持 600 权限。 |
| `config/secrets/` | 密钥，文件权限 600，目录权限 700。 |
| `runtime/postgres/data` | 数据库。 |
| `runtime/data` | 数据块和上传的文件。 |
| `runtime/node` | 随附节点的身份和工具缓存，其中有它解压的智能体程序（约 1.1 GB）。 |
| `runtime/{server,frontend,cli}/` | 解压后的各版本，以及 `current` 和 `previous` 两个链接。 |
| `runtime/update-backups/` | 备份：每次更新和回滚之前一份，另有 `backup` 命令生成的。 |
| `runtime/update/` | 进行中的事务日志，以及已安装内容的记录。 |

## 下一步

- 为 Orch 配置 HTTPS 地址和网关地址：[HTTPS 前端](../edge.md)。
- 添加提供方并开始第一个会话：[开始使用](../../user/zh-CN/getting-started.md)。
- 连接一台机器或一台主机：[连接一台机器](../../user/zh-CN/connect-a-machine.md)。
- 规划备份：[备份与恢复](../backup-and-restore.md)。
- 以后更新：[更新与回滚](upgrades.md)。
- 出了问题：[故障诊断](../incidents.md)。

## 把数据迁移到另一块磁盘

`ORCH_DATA_DIR` 保存服务器文件，包括数据块、前端 TLS 文件和更新状态。`ORCH_POSTGRES_DATA_DIR` 保存 PostgreSQL 数据。新安装可在首次启动前在 `.env` 中分别设置。默认位置是 `./runtime/data` 和 `./runtime/postgres/data`。

迁移前先让新磁盘在开机时自动挂载，选取两个空目录。完成未结束的更新并做好[备份](../backup-and-restore.md)。在部署目录执行以下命令，停止写入并保留所有权、权限和链接。示例假设当前使用默认路径，新磁盘挂载在 `/mnt/orch`。

```sh
docker compose down
mkdir -p /mnt/orch/server /mnt/orch/postgres
cp -a ./runtime/data/. /mnt/orch/server/
cp -a ./runtime/postgres/data/. /mnt/orch/postgres/
```

两份复制都成功后，在 `.env` 中设置 `ORCH_DATA_DIR=/mnt/orch/server` 和 `ORCH_POSTGRES_DATA_DIR=/mnt/orch/postgres`。运行 `docker compose up -d`，再运行 `./update.sh doctor`。确认能打开已有会话及其中的文件后，再处理旧副本。在接受新写入之前，如需退回，可停止服务，恢复原路径，再启动。后续备份应从选定的服务器目录复制数据块。
