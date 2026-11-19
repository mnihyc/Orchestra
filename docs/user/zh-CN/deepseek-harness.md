---
title: DeepSeek Harness
screens: /resources/agents, /resources/agents/$id, /thread/$threadId, /admin/$section
keywords: DeepSeek Harness, dsh, DeepSeek, OpenRouter, Agent Client Protocol, 隐私, 遥测, 流式, 插件, 程序, 密钥, 提供方
heading_id.它是什么: what-it-is
heading_id.模型和提供方: models-and-providers
heading_id.可用功能: what-works
heading_id.插件增加的功能: what-the-plugin-adds
heading_id.隐私默认设置: privacy-defaults
heading_id.限制: limits
---

# DeepSeek Harness

DeepSeek Harness（`dsh`）是 DeepSeek 的开源智能体程序。Orch 像运行 Codex 和 Claude Code 一样运行它：它在对话的环境中工作，
使用 Orch 的工具，并在对话的审批设置要求时，先询问再修改。

## 它是什么

Orch 为每个对话启动一个 `dsh`，并通过 Agent Client Protocol 与它通信。构建来自 PyPI，以 SHA-256 校验，在机器的工具缓存中只保存一份；
它不需要 Python 或 Node.js。像添加其他智能体一样添加它：打开**资源 → 智能体**，选择 **DeepSeek Harness**，再选一个允许它运行的环境。

## 模型和提供方

每个模型请求都经过 Orch 的代理，密钥由代理保管。给环境配置 DeepSeek、OpenRouter，或 OpenAI Chat、OpenAI Responses、Anthropic 端点之一的密钥；
对话会提供该提供方的模型。如果没有这些提供方的密钥，对话会说明原因和该怎么做：给环境配置一个该程序支持的提供方的密钥。

暂不提供 DeepSeek 账号登录。请使用密钥。

只提供 Responses 或 Anthropic 的提供方也可使用，无需 Chat Completions。同时提供两种 OpenAI 协议时，
dsh 优先使用 Chat Completions。如果程序拒绝会话，请检查其提供方和设置后重试；对话会保留程序报告的具体原因。

## 可用功能

- 完整的消息、推理、带输出的工具调用、计划列表，以及上下文已用多少。
- 审批：当命令要超出 dsh 自身的限制时，你会看到命令和原因，可以只允许一次或始终允许。
- 停止：正在运行的轮次以“已停止”结束；不停止的程序会被结束，对话会说明会话已丢失。
- 继续：对话会带着先前的内容重新打开程序，之后的第一轮会多花几秒。
- 程序在无人等待时结束（崩溃、被停止，或闲置后退出），对话会说明它是如何结束的，包括退出码或信号。你的下一条消息会让它带着先前的内容重新启动。
- Orch 的工具和环境自己的服务器，以地址和对话的令牌提供给它。
- 在模型支持时可发送图片，以及对话所选的模型和推理强度。

## 插件增加的功能

Orch 随附一个在 dsh 内部运行的插件，补充协议没有的部分。程序会说明你当前构建中哪些能用；**Harnesses** 页面列出已关闭的功能和原因。

- 模型写作时实时显示文字和推理。
- 向正在运行的轮次追加内容。
- 向你提问（`ask_user_question`）。
- 计划模式：在你点击继续之前不做任何修改；也可以用你自己的话选择继续规划。
- 在“询问”审批下，每次修改前先询问。
- 每次模型调用的令牌数。
- 压缩阶段。
- 来自对话技能的技能。
- 把对话历史带入 dsh。
- 当对话的环境与提供方分离时，命令和文件在对话的环境中运行。

目标使用 Orch 的目标控制和工具。dsh 自带的目标工具和自动继续功能已关闭，确保每一轮都经过对话的审批、限额和暂停控制。

没有插件，或某项功能已关闭时，其余功能照常工作：那部分是普通协议。

切换到 dsh 时，“历史”默认携带整个对话；选择某一轮时，只携带到该轮为止的对话。若导入失败，请改用“摘要”或“不携带历史”重试，或请运维人员检查程序的历史导入支持。

## 隐私默认设置

- 日志上传已关闭：dsh 不会把会话日志发送给它的厂商。
- 插件清单已关闭：不会发送插件列表。
- 遥测已关闭。
- 流量经过 Orch 的代理：离开环境的只有模型请求，其中标明客户端的请求头会被移除。带有日志或插件清单的请求会被整体拒绝。

## 限制

- 暂不提供 DeepSeek 账号登录。
- 它自身没有模式或命令列表；Orch 的计划模式和审批来自插件。
- 暂不提供在某一轮分叉。
- 对话的模型列表就是提供方的；程序没有的模型会在轮次开始前被拒绝。
- 构建解压后约 400 MB，每台机器每个版本只保存一份。
