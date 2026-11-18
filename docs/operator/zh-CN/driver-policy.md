# 驱动策略与镜像构建

[English](../driver-policy.md)

节点在自己的数据目录中读取唯一的机器策略文件 `config.toml`。驱动权限、存储、钩子和端口策略都在这里，
部署配置 `config/orch.toml` 只保留 Docker 套接字、构建参数等连接和运行配置。

随部署提供的节点首次启动时，会把 [config/node.toml](../../../deploy/config/node.toml)
复制到 `runtime/node/config.toml`。重复首次启动和后续更新都会保留已经存在的机器策略。
初始策略允许 Docker 驱动、Dockerfile 构建和镜像归档加载，镜像、仓库、能力和运行时类别均不设白名单限制；
特权运行仍然关闭。

镜像构建由机器策略中的 `[drivers.images] builds` 控制。允许构建时，`images` 和 `registries`
还必须都设为 `["*"]`，因为 Dockerfile 可以获取其他基础镜像。把 `builds` 设为 `false` 即可禁止构建，
节点会按当前策略执行，不会从部署配置中重新导入权限。

连接参数保存在 `drivers.configurations`，按驱动的配置模式核验；机器文件将每个配置对象保存为 JSON 文本，
以保留空值和嵌套数组。修改连接参数后需要重启节点。已有任务的清理仍使用原先打开的连接。
在主机的“概览 → 详情 → 驱动 → 功能”中查看实际支持及不可用原因。支持某项操作并不代表策略已允许它。
