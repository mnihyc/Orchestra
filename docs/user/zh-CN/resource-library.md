---
title: 资源库
screens: /resources/providers, /resources/agents, /resources/skills, /resources/mcp, /resources/templates, /resources/environments, /resources/mounts, /resources/hosts, /resources/machines, /everything, /project/$projectId, /topic/$topicId, /documents/$nodeId, /inbox, /approvals, /search, /usage, /trace/$nodeId, /resources/providers/$id, /resources/agents/$id, /resources/skills/$id, /resources/mcp/$id, /resources/templates/$id, /resources/environments/$id, /resources/mounts/$id, /resources/hosts/$id, /thread/$threadId, /doc/$documentId
keywords: 资源库, 资源, 已提供给, 已使用, 热门, 我拥有的, 我管理的, 我能使用的, 我能查看的, 排序, 筛选, 上次状态, 设备
heading_id.选择视图: choose-a-view
heading_id.阅读提供与使用统计: read-given-and-used
heading_id.返回列表: return-to-a-list
---

# 资源库

## 选择视图

**显示**菜单位于搜索框前。**我拥有的**显示自己的资源；**我管理的**包含自己拥有的资源；
**我能使用的**包含前两类；**我能查看的**包含所有可见资源。
环境只有查看和管理两个级别，因此省略“我能使用的”。组织管理员还可在分隔线后选择组织中的全部资源。

视图只决定列出什么。实际使用仍检查当前权限、状态和限制；已停用的资源也可出现在管理列表中。
视图为空时，按钮会打开下一个范围更广的视图。

## 阅读提供与使用统计

**已提供给**表示当前关系。提供方、配置档和环境显示绑定的会话；技能显示明确配置的位置和实际交付的会话；
MCP 服务器显示明确配置的位置和环境；模板显示仍存在的环境或提供方运行时；挂载显示实际连接的环境；
宿主机显示其运行单元。打开数字可查看你有权读取的名称，其余显示为“另有 N 个”。继承可用性不会多算一个配置位置。

**已使用**只统计实际报告的工作：提供方、配置档和环境统计轮次；MCP 服务器统计工具调用；
技能统计报告的技能调用；环境模板和宿主机统计创建的环境；提供方模板则统计创建的提供方。
可用和已连接都不算使用。同一环境重新创建不会增加累计环境数；技能版本分别保留调用统计。

名称、人员及技能版本按页查看。“下一页”继续查看，“返回开头”回到第一页。

打开使用量可查看累计、最近 30 个 UTC 日期、不同使用人数及最近使用时间。提供方还显示不同会话数。
**开始记录于**表示更早的历史不完整；**未记录**或短横线表示程序未报告可信数字。
挂载不显示使用量。Codex 发现或交付技能并不能证明实际调用，Orch 不会搜索会话文字来猜测使用量。

**按使用量**对整个匹配列表排序后再分页，未记录的统计排在已知数字后。
每类资源的统计单位不同，适合在同类资源中比较。

## 返回列表

每个列表在本设备上保留自己的视图、排序和筛选；其他页面有各自的状态。
普通访问不会恢复搜索文字。链接或返回按钮携带的状态优先，并成为页面的上次状态。
已失去资格的选项会回到页面默认值。

这是页面的上次状态，不是设置，也没有继承默认值。
可在界面设置的设备数据管理中清除这些记录。
