---
title: 单元启动后运行命令
screens: /resources/environments, /resources/providers, /resources/templates
keywords: 启动后命令, after_up, env.after_up, runtime.after_up, runtime.storage, runtime.needs, env.variables, runtime.variables, argv, 超时, 只读根目录
heading_id.选择命令及其权限: choose-a-command-and-its-rights
heading_id.启动重试和失败: starts-retries-and-failure
heading_id.个人机器: a-persons-own-machine
---

# 单元启动后运行命令

受管环境和提供方运行时共用单元配置。启动后命令在连接器注册或附着之后、就绪之前，
于单元内部运行。主机运维钩子单独在主机执行，早于这些命令。

模板的命名命令先以只读显示，其下是解析后的 `env.after_up` 或 `runtime.after_up` 和来源。
先按名称顺序执行模板命令，再按名称执行设置命令。设置中的 null 项移除继承的设置命令，
不能移除模板命令。

## 选择命令及其权限

分别输入名称、程序、各个参数及期限。期限默认 300 秒，可选 1–3600 秒。
没有隐式 shell，参数中的 `$HOME`、`&&` 或管道都只是文字。需要 shell 语法时，
选择确实包含 shell 的可信镜像，并在 argv 显式指定它。命令使用单元账号和变量，
不使用主机运维账号。环境命令从工作区开始，提供方命令从主目录开始。

具备普通文件工具的环境可用可重复执行命令创建缓存目录：

```json
{"cache":{"command":["/usr/bin/mkdir","-p","/workspace/.orch/cache"],"timeout_secs":30}}
```

提供方上的命令靠近凭据并持有代理程序权限，仅所有者可在资源处修改。预置镜像为
只读根目录、非 root 用户且没有 shell 或包管理器。先选择合适的可信模板，设置不会
增加权限或安装缺失工具。

例如可信模板自带 `/usr/bin/install` 和静态 `/opt/tools/acme-cli`，可安装到可写主目录：

```json
{"install-cli":{"command":["/usr/bin/install","-D","-m","755","/opt/tools/acme-cli","/home/agent/bin/acme-cli"],"timeout_secs":30}}
```

实际用户和主目录必须匹配模板。位置管理者可设默认值，提供方自身命令仍仅所有者可改。
秘密通过普通秘密名称控件选择，不要写进明文参数或诊断输出。

## 启动、重试和失败

受管单元每次新启动、重启、重建、更新、移动和续期启动都会执行，命令必须可重复执行。
重放同一个启动作业返回已记录结果，不再执行。同一已运行单元续期不重新执行。

非零退出、启动失败或超时会令启动失败并停止单元，没有忽略失败开关。超时结束命令进程组。
作业或资源页显示失败名称、退出状态、已遮蔽的开头输出和设置链接。历史只保留名称、
结果和耗时，不保留输出或秘密。修复程序或镜像后，用一次新的审查运行再启动。

单元停止、因空闲停止或被回收时，单元内不运行任何命令；下次启动时这些命令会再次运行。
属于停止时的工作（例如删除路由）应由主机运维人员的 `pre_down` 或 `post_down` 节点钩子完成
（[节点钩子](../../operator/node-hooks.md)）。

模板以列表写同样的命令，每条都有名称：

```json
"after_up": [{"name": "10-cache", "command": ["/usr/bin/mkdir", "-p", "/workspace/.orch/cache"], "timeout_secs": 30}]
```

## 个人机器

个人机器没有受管单元，所以启动后命令及驱动存储、限额会说明不适用。环境变量在连接器
附着时生效；提供方代理程序变量属于代理配置。Orch 不在此人的主机执行这些命令。

受管提供方的 `runtime.storage` 只选择新状态卷的存储，已有卷保留记录位置。
`runtime.needs` 声明放置条件，不预留内存或磁盘。工作挂载、工作区存储和访问入口属于环境。
详见[资源](resources.md)及[提供方详情](provider-details.md)。
