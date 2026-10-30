---
title: 选择机器保存数据的位置
screens: /resources/templates, /resources/templates/$id, /resources/machines/$id, /resources/environments/$id, /resources/providers/$id
keywords: 存储, 磁盘, 机器, 主机, 模板, env.storage, runtime.storage, 工作区, 主目录, 状态
heading_id.配置命名存储: configure-named-storages
heading_id.替换主机模板路径: replace-host-template-paths
heading_id.设置共同默认值: set-common-defaults
---

# 选择机器保存数据的位置

机器报告命名存储的容量和空闲空间。先看其存储页，再在单元配置使用这些名称。
`env.storage` 选择新工作区、主目录和连接器状态数据的位置。独立状态存储用于驱动创建
的单元；目录连接器保留自己的私有数据目录，只声明工作区、主目录和挂载存储角色。
`runtime.storage` 在驱动声明支持时选择提供方主目录和状态位置。省略成员使用机器默认值，
未提供的名称会拒绝，不会悄悄改用别的磁盘。

例如 `{"workspace":"fast","home":"bulk","state":"fast"}` 为环境选择两个存储。
提供方没有工作区，可用 `{"state":"fast"}` 保存连接器私有状态；数据源声明主目录时
另行选择它的位置。

存储选择只影响新建的自有数据。已有目录保留身份、权限和位置。Orch 已创建的数据会在
重启和模板修改后保留原存储。修改设置不会搬移数据；已有环境要使用经过审查的移动或复制。

## 配置命名存储

节点在 `orch.toml` 的 `[[node.storage]]` 声明名称和路径；目录连接器在 `config.toml`
的 `[[storage]]` 使用相同的 `name`、`path` 和 `default`。路径必须已存在且位于本机允许的
`environment.roots` 中，机器先检查策略。声明的存储恰有一个默认项。

```toml
[[storage]]
name = "fast"
path = "/srv/work"
default = true
```

没有显式列表时，连接器按允许根目录生成 `default`、`root-2`、`root-3` 等名称，并在其中
创建自有目录。没有允许根就没有存储。根顺序可能改变时请显式命名。报告不暴露连接器
状态和凭据路径。

## 替换主机模板路径

主机进程模板包含实际存在的运行账号和命令，可以跨机器共享，不再含 `workspace_base`、
`home_base` 或 `mounts_base`。把旧目录声明为机器命名存储，在 `env.storage` 选择工作区
和主目录名，并在添加挂载卷位置时选存储，然后移除旧字段。已有卷保留精确路径；
应用配置不会搬移、接管或清除目录。

表单使用驱动自己的说明。示例包含账号和命令，不必知道某台主机的文件布局。

## 设置共同默认值

环境和提供方运行时共用限额、存储、放置条件、变量、秘密名称、更新及启动后命令的字段
结构。从系统经资源所属位置继承，最后使用资源本地值。运行时磁盘以 GB 设置，受机器
声明的配额能力约束。提供方运行时没有工作区、工作挂载、环境终端、浏览器或空闲警告。
