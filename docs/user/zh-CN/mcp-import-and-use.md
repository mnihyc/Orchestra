---
group: resources
screens: [/resources/machines/$id, /resources/mcp, /resources/mcp/$id, /resources/environments/$id, /thread/$threadId]
keywords: [MCP, 导入, Codex, Claude Code, 工具, 启用, 机器, 环境, 忘记, 策略]
heading_id.在哪里: where
heading_id.示例: example
heading_id.操作后会怎样: what-to-expect
summary: 将机器上的 MCP 定义导入为资源，并在需要的位置启用。
---

# 导入并启用 MCP 服务器

导入会将代理程序在机器上的服务器配置显示为 Orch 资源。含有密钥的定义仍留在原机器上。启用则通过现有 MCP 设置，将服务器提供给选定环境或位置。

## 在哪里

从[机器](/resources/machines)打开机器，再进入“工具”。“在代理自身配置中发现”显示上报的程序、范围、传输方式及导入状态。导入前检查名称和定义结构，初始勾选“在此启用”。原名称不符合格式时可使用别名。

[MCP 服务器](/resources/mcp) → 添加 MCP 服务器 → 从机器导入，也提供相同流程。选择你有权使用的环境。在线程的环境面板或环境工具中，“导入并在此启用”可逐个处理发现的配置来源。每个来源的导入和启用是一项操作，仍受机器策略及导入权限约束。

## 示例

Workstation 上报名为 docs 的 Codex 服务器。将它导入 Research 环境，保留“在此启用”。打开服务器查看“已在以下位置启用”，也可以在此选择其他具名位置。在环境工具中使用服务器旁的开关，无需输入标识符。

## 操作后会怎样

服务器显示启动中、运行中、已停止、失败或不可用，并附有原因。列表说明更新是立即、下一轮还是下次会话生效。从 Workstation 导入的副本不会借用另一台机器上同名配置的密钥；在那里不可用时，需要明确提供适用的服务器。

受限视图可能先显示导入成功，再显示可读取的资源链接。批量操作部分完成时会保留成功来源，只继续未完成的部分。策略关闭导致不显示发现结果，并不代表程序没有服务器。“忘记”移除导入的 Orch 资源，不会删除程序自己的配置。另见[资源分组](resource-groups.md)和[环境切换](switch-and-move-environments.md)。
