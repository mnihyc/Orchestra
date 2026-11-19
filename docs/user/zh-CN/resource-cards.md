---
title: 查看资源卡片和选项
screens: /resources, /resources/providers, /resources/providers/$id, /resources/agents, /resources/agents/$id, /resources/environments, /resources/environments/$id, /resources/hosts, /resources/hosts/$id, /resources/machines, /resources/machines/$id, /resources/templates, /resources/templates/$id, /resources/mounts, /resources/mounts/$id, /resources/mcp, /resources/mcp/$id, /resources/skills, /resources/skills/$id, /thread/$threadId, /topic/$topicId, /project/$projectId, /org/$slug, /admin/$section
keywords: 资源卡片, 检查, 选项, 提供方, 模型, 主机, 环境, 负载, 速度, 上下文, 限额, 未知
heading_id.检查选项: inspect-a-choice
heading_id.结合来源阅读信息: read-facts-with-their-source
heading_id.理解缺失信息和阻塞原因: understand-missing-or-blocked-facts
---

# 查看资源卡片和选项

## 检查选项

同一个资源名称在不同位置打开相同的卡片。悬停或聚焦可快速查看；点击或按 Enter
可固定卡片。Escape 关闭最上层卡片，标题可打开资源页面。

在选择器中，高亮选项会显示信息，不会选中它。按右箭头进入卡片，Escape 返回选项。
确定后再选择。查看卡片不会修改设置。

## 结合来源阅读信息

提供方可显示可用额度窗口、重置时间、活动轮次和适用限额。模型可显示上下文、支持的
推理强度、价格和实测速度。速度来自最近七天中符合条件的请求，不能保证下一轮也有
相同表现。比较前请阅读[测量的含义](speed-first-output-and-errors.md#what-the-figures-mean)。

主机显示容量、声明用量、可用存储以及是否接受新工作。环境显示主机、线程、终端，
以及报告的进程、CPU、内存和磁盘用量。声明容量和当前消耗是不同数据。判断数值
是否仍然适用前，先看观测时间。

挂载、MCP 服务器、模板、代理配置和技能卡片显示各自的配置和允许的用途。模板的
镜像引用和摘要说明来源。代理配置说明程序，不拥有提供方的登录凭据。

## 理解缺失信息和阻塞原因

短横线表示未知。加载中的卡片仍在读取；读取失败可以重试，不会把未知值写成零。
离线、已满和达到限额说明的是不同的启动障碍。

卡片只显示你有权读取的信息。看见名称不等于获得使用或管理权。不可用的选项保留
原因。卡片里的启动、停止或回收沿用资源页面的权限检查和确认流程。
