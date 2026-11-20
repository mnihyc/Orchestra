---
group: work
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [state, status, phase, reference, 状态, 阶段, 参考]
summary: 按页面查找状态文字，了解它的含义以及接下来可以做什么。
heading_id.会话与轮次: threads-and-turns
heading_id.侧栏里的会话: threaddisplaystate
heading_id.会话当前进度: threadstate
heading_id.轮次历史: turnstate
heading_id.停止轮次: stopstate
heading_id.等待源文件: blockedreason
heading_id.准备源文件: sourcestate
heading_id.话题看板: topicstatus
heading_id.计划步骤: planstepstatus
heading_id.从另一处继续: threadcontinuationstatus
heading_id.旁支会话: sidestate
heading_id.旁支会话的工作方式: sidereading
heading_id.会话文件: nativehistorystate
heading_id.读取早期历史: adopthistorystate
heading_id.会话为何重新打开: sessionreopenreason
heading_id.会话变更提示: markerkind
heading_id.警告与文件变更: markerharnesschanged
heading_id.个人提问: askstate
heading_id.摘要: summarystate
heading_id.自动归档与删除: noderetentionhold
heading_id.消息: messages
heading_id.发送消息: inputstate
heading_id.程序收到的输入: modelinputstate
heading_id.消息中的工作: itemstatus
heading_id.定时消息: inputschedulestate
heading_id.单次定时发送: inputschedulerunstate
heading_id.收件箱通知: noticedecisionstate
heading_id.为何没有发送通知: noticesuppressionreason
heading_id.外部渠道的投递情况: channelacceptancestate
heading_id.缺少的诊断详情: rawcaptureomissioncause
heading_id.机器: machines
heading_id.机器连接: machinestatus
heading_id.智能体程序连接: harnessstatus
heading_id.网关连接: tunnelgatewaystate
heading_id.环境: environments
heading_id.环境状态: environmentstatus
heading_id.移除与恢复: environmentremovalphase
heading_id.挂载存储: envmountstate
heading_id.文件快照: snapshotstate
heading_id.打开文件版本: fetchstatus
heading_id.等待文件: fetchunavailable
heading_id.已存上传: uploadinventorystate
heading_id.检查上传: uploadphase
heading_id.查找工作区文件: environmentindexreason
heading_id.提供方: providers
heading_id.提供方是否可用: providerstatus
heading_id.模型信息: memberstate
heading_id.用量限额: capstate
heading_id.智能体程序: agent-programs
heading_id.检查程序版本: harnessbuildstatus
heading_id.查询新版本: programchannelstate
heading_id.扩展: pluginstate
heading_id.机器上发现的-mcp-服务: mcpofferingstate
heading_id.mcp-服务状态: mcpstate
heading_id.子智能体: subagentstate
heading_id.审批与问题: approvals-and-questions
heading_id.答复请求: approvalstate
heading_id.请求列表: approvalliststate
heading_id.自动审批不可用的原因: approvalchoicereason
heading_id.检查自动审批: autoapprovalstate
heading_id.端口与视图: ports-and-views
heading_id.端口上的程序: porttargetstate
heading_id.共享端口: portopeningstate
heading_id.从外部打开视图: outsidewaystate
heading_id.终端会话: terminalstate
heading_id.浏览器会话: browserstate
heading_id.目标: goals
heading_id.目标进度: goalstatus
heading_id.智能体报告: goalreportstatus
heading_id.等待你决定的目标: goalreason
heading_id.核验结果: goaloutcome
heading_id.确认达成目标的人: goalcheckedby
heading_id.后台工作: background-work
heading_id.查看后台工作: backgroundstate
heading_id.后台任务: backgroundtaskstate
heading_id.结束命令: backgroundendstate
heading_id.作业: jobstate
heading_id.作业步骤: phasestate
heading_id.语义搜索: semanticstate
heading_id.重试搜索更新: semanticretrynext
heading_id.导出: exportstate
heading_id.导入: importstate
heading_id.人员与共享: people-and-sharing
heading_id.组织设置: firststepsstate
heading_id.页面访问: pagestate
heading_id.谁能使用共享资源: agentsharecurrentstate
heading_id.共享给会话的文件: agentsharesnapshotstate
heading_id.活动历史中的对象: auditobjectstate
heading_id.登录: sign-in
heading_id.账号访问: adminuserstatus
heading_id.登录验证: humancheckedgestate
heading_id.送达你的决定: delivering-your-decision
heading_id.自动标题和标签: automatic-titles-and-tags
heading_id.返回原来的模型: returning-to-the-original-model
heading_id.数据库请求超时: when-a-database-request-times-out
heading_id.文件传输: file-transfers
heading_id.重新试用提供方: trying-a-provider-again
---

# 状态参考

先找到显示该状态的页面。选择状态文字可链接到相应说明。下文中的姓名和日期是示例。另见[会话状态](thread-states.md)和[错误处理](error-reference.md)。

## 会话与轮次

### 侧栏里的会话

| 页面上的文字 | 含义 |
| --- | --- |
| [等待审批](#state:ThreadDisplayState.waiting_approval) | 请求或排队的消息需要有人决定。打开审批或会话来答复。 |
| [已阻塞](#state:ThreadDisplayState.blocked) | 工作因条件不足而暂停。打开会话查看原因。 |
| [失败](#state:ThreadDisplayState.failed) | 上一轮失败了。先查看错误，再决定是否发送新消息。 |
| [运行中](#state:ThreadDisplayState.running) | 智能体正在启动或执行一轮工作。打开会话查看进度。 |
| [等待开始](#state:ThreadDisplayState.queued) | 智能体空闲时会按顺序处理消息。打开队列可查看等待的消息。 |
| [队列已暂停](#state:ThreadDisplayState.held) | 消息保留在队列中。准备好后选择继续。 |
| [有新结果](#state:ThreadDisplayState.finished) | 上次阅读后有一轮已结束。打开会话查看回复。 |
| [空闲](#state:ThreadDisplayState.idle) | 智能体空闲，你已看完最新结果。发送消息即可开始工作。 |
| [已归档](#state:ThreadDisplayState.archived) | 此会话或其上级已归档。仍可打开阅读之前的工作。 |

### 会话当前进度

| 页面上的文字 | 含义 |
| --- | --- |
| [空闲](#state:ThreadState.idle) | 需要开始工作时可发送消息。 |
| [启动中](#state:ThreadState.starting) | 智能体正在做准备，会话会显示当前步骤。 |
| [运行中](#state:ThreadState.running) | 智能体正在工作，可在会话中查看回复。 |
| [等待审批](#state:ThreadState.waiting_approval) | 工作在等待审批。打开请求即可作出决定。 |
| [已阻塞](#state:ThreadState.blocked) | 会话会显示是什么阻止了下一步。解决所列问题后即可继续。 |

### 轮次历史

| 页面上的文字 | 含义 |
| --- | --- |
| [启动中](#state:TurnState.starting) | 程序正在准备这一轮。可在会话中查看进度。 |
| [运行中](#state:TurnState.running) | 在会话中查看接下来的回复或请求。 |
| [已完成](#state:TurnState.completed) | 这一轮已完成。可阅读回复并查看文件变更。 |
| [已停止](#state:TurnState.interrupted) | 这一轮提前停止。先查看已经完成的部分，再决定如何继续。 |
| [失败](#state:TurnState.failed) | 程序报告失败。打开这一轮可查看原因。 |

### 停止轮次

| 页面上的文字 | 含义 |
| --- | --- |
| [正在停止…](#state:StopState.stopping) | 停止请求已受理。请等待当前工作停止。 |
| [已停止](#state:StopState.stopped) | 这一轮已结束或准备工作已取消。可在会话中查看结果。 |

### 等待源文件

| 页面上的文字 | 含义 |
| --- | --- |
| [源准备失败](#state:BlockedReason.source_failed) | 源文件准备失败。打开源文件面板查看原因，解决后可重新准备。消息仍在队列中。 |

### 准备源文件

| 页面上的文字 | 含义 |
| --- | --- |
| [源代码等待准备](#state:SourceState.pending) | 工作正在等待源文件。打开源文件面板可查看下一步。 |
| [正在准备源代码](#state:SourceState.preparing) | 环境正在准备文件。可在源文件面板查看进度。 |
| [未能准备好源代码](#state:SourceState.failed) | 准备时出错。重试前先在源文件面板查看原因。 |

### 话题看板

| 页面上的文字 | 含义 |
| --- | --- |
| [进行中](#state:TopicStatus.open) | 此话题中的工作仍在进行。打开看板可查看各个会话。 |
| [已完成](#state:TopicStatus.done) | 此话题已标为完成。若要继续工作，可重新打开。 |

### 计划步骤

| 页面上的文字 | 含义 |
| --- | --- |
| [未开始](#state:PlanStepStatus.pending) | 智能体计划稍后执行此步骤。可先查看当前步骤。 |
| [进行中](#state:PlanStepStatus.in_progress) | 智能体正在执行此步骤。可在本轮看到进展。 |
| [已完成](#state:PlanStepStatus.completed) | 智能体已将此步骤标为完成。可在本轮查看结果。 |

### 从另一处继续

| 页面上的文字 | 含义 |
| --- | --- |
| [正在准备此消息版本…](#state:ThreadContinuationStatus.preparing) | 程序正在准备选中的会话位置。准备好后就可使用。 |
| [正在查看保存的消息版本。](#state:ThreadContinuationStatus.ready) | 现在可选择此处，并从这里继续。 |
| [无法准备此消息版本。](#state:ThreadContinuationStatus.failed) | 准备失败。仍可阅读原有历史，并从原来的位置继续。 |

### 旁支会话

| 页面上的文字 | 含义 |
| --- | --- |
| [已开的旁路会话](#state:SideState.open) | 打开旁支会话可查看工作进展或发送消息。 |
| [复核 · 已关闭](#state:SideState.closed) | 旁支会话已结束。仍可查看之前的对话。 |

### 旁支会话的工作方式

| 页面上的文字 | 含义 |
| --- | --- |
| [可执行操作](#state:SideReading.acting) | 旁支会话可使用主工作区允许的工具。在旁支会话中可查看其操作。 |

### 会话文件

| 页面上的文字 | 含义 |
| --- | --- |
| [会话文件已保留](#state:NativeHistoryState.retained) | 程序找到了继续此会话所需的文件。可在 Orch 中阅读对话。 |
| [会话文件已移除](#state:NativeHistoryState.removed) | 程序已确认会话文件不在了。仍可在 Orch 中阅读对话。 |
| [尚未报告](#state:NativeHistoryState.unreported) | 程序尚未确认会话文件是否存在。仍可阅读对话。 |

### 读取早期历史

| 页面上的文字 | 含义 |
| --- | --- |
| [来自 Orch 之前的对话](#state:AdoptHistoryState.read) | 已读取先前消息。如果原会话为空，就没有早期消息可显示。 |
| [未能读取之前的对话记录。](#state:AdoptHistoryState.unreadable) | 读取早期消息失败。选择重新读取即可重试。 |
| [此智能体程序无法读取该会话之前的记录。](#state:AdoptHistoryState.unsupported) | 此程序无法读取已有会话的早期消息。可使用当前可见的历史继续。 |
| [会话所在的机器已离线；恢复连接后可以读取其记录。](#state:AdoptHistoryState.offline) | 机器重新连接后即可读取。可在机器上线后回来查看。 |
| [会话所在的机器或提供方已不可用。](#state:AdoptHistoryState.unavailable) | 无法连接会话的机器、环境或提供方。重试前先检查它是否可用。 |

### 会话为何重新打开

| 页面上的文字 | 含义 |
| --- | --- |
| [会话已重新打开：服务器或程序重新启动过，所以不确定它是否仍处于打开状态。](#state:SessionReopenReason.unknown) | 服务器或智能体程序重启后重新打开了会话。可在原会话中继续。 |
| [会话已重新打开：指令有更改。](#state:SessionReopenReason.instructions) | 本轮使用更新后的指令。可在会话设置中查看。 |
| [会话已重新打开：其子代理的限制有更改。](#state:SessionReopenReason.limits) | 程序已重新打开会话，以应用新的子智能体限制。可在参数面板查看当前值。 |
| [会话已重新打开：有只在新会话中生效的设置发生了更改。](#state:SessionReopenReason.configuration) | 工作目录或其他启动设置变了。可在会话中查看当前工作区。 |
| [会话已重新打开：提供给它的 MCP 服务器有更改。](#state:SessionReopenReason.mcp) | 这一轮使用更新后的 MCP 服务。可在会话工具中查看可用服务。 |
| [会话已重新打开：提供给它的 Orch 工具有更改。](#state:SessionReopenReason.tools) | Orch 工具发生了变化。重新打开的会话将使用本轮可用的工具。 |
| [会话已重新打开：提供给它的技能有更改。](#state:SessionReopenReason.skills) | 本轮使用更新后的技能。可查看智能体当前可用的技能。 |
| [会话已重新打开：此程序在每次打开会话时重新读取目录中的文件，所以每一轮都会打开一个新会话。](#state:SessionReopenReason.program_reloads) | 程序会在打开会话时重新载入上下文。本轮会使用这些内容继续。 |
| [会话已重新打开：这个旁路会话的历史重新从其父会话读取。](#state:SessionReopenReason.side_history) | 旁支会话需要更新后的对话历史。可在该会话中查看下一轮。 |

### 会话变更提示

| 页面上的文字 | 含义 |
| --- | --- |
| [会话已重新打开：服务器或程序重新启动过，所以不确定它是否仍处于打开状态。](#state:MarkerKind.session_reopened) | 程序在本轮开始前重新打开了会话。可查看这条提示了解原因。 |

### 警告与文件变更

| 页面上的文字 | 含义 |
| --- | --- |
| [智能体：连接中断](#state:MarkerHarnessChanged.warning) | 程序在本轮给出了警告。可结合回复判断是否需要处理。 |
| [智能体 已更新更改的文件。](#state:MarkerHarnessChanged.turn_diff) | 程序更新了本轮文件变更清单。打开变更可查看最新结果。 |
| [Orch 无法读取 智能体 更新的差异，已保留此前的文件更改。](#state:MarkerHarnessChanged.turn_diff_unavailable) | 无法读取最新的文件变更报告。仍显示上次成功读取的变更。 |

### 个人提问

| 页面上的文字 | 含义 |
| --- | --- |
| [就绪](#state:AskState.ready) | 现在可以发送下一个问题。 |
| [正在回答](#state:AskState.running) | 正在准备答复。请等本轮结束再发送下一个问题。 |
| [访问权限已结束](#state:AskState.closed) | 你已无法访问此对话所属的组织。可向所有者询问访问权限。 |
| [已保留为会话](#state:AskState.kept) | 这段对话已保存为会话。打开该会话可从已有历史继续。 |

### 摘要

| 页面上的文字 | 含义 |
| --- | --- |
| [暂无摘要](#state:SummaryState.empty) | 暂时还没有摘要。你仍可打开原文。 |
| [最新](#state:SummaryState.fresh) | 摘要反映当前内容。详情可查看原文。 |
| [待更新](#state:SummaryState.stale) | 原文在上次生成摘要后有了变化。最新信息以原文为准。 |
| [已安排](#state:SummaryState.scheduled) | 更新正在等待下一次允许的启动时间，你可以先阅读原有摘要。 |
| [更新中](#state:SummaryState.updating) | 摘要更新已排队或正在运行。完成后会显示新文字。 |
| [更新失败](#state:SummaryState.failed) | 上次更新失败。可阅读先前的摘要并查看原因。 |
| [未设置摘要器](#state:SummaryState.no_summarizer) | 尚未选择合适的摘要程序，或程序尚不可用。可检查摘要设置。 |

### 自动标题和标签

| 页面上的文字 | 含义 |
| --- | --- |
| [下次更新](#state:AutomaticScheduleState.queued) | 更新已排队。显示的时间是它最早可以开始的时间。 |
| [等待重试](#state:AutomaticScheduleState.retrying) | 上次更新失败。可查看下次可以重试的时间。 |
| [已跳过 · 未选择摘要器](#state:AutomaticScheduleState.skipped) | 本次自动更新已跳过。文字旁边会说明原因。 |

### 自动归档与删除

| 页面上的文字 | 含义 |
| --- | --- |
| [工作正在进行](#state:NodeRetentionHold.active_work) | 仍有工作在进行。工作结束后才可自动归档。 |
| [已归档](#state:NodeRetentionHold.already_archived) | 此处已经归档。仍可打开阅读历史。 |
| [上级已归档](#state:NodeRetentionHold.ancestor_archived) | 上级位置已归档。更改此处前先查看上级状态。 |
| [尚未归档](#state:NodeRetentionHold.not_archived) | 自动删除在归档后才适用。此处目前仍开放。 |

## 消息

### 发送消息

| 页面上的文字 | 含义 |
| --- | --- |
| [等待开始](#state:InputState.pending) | 消息已进入队列。可在开始处理前查看。 |
| [正在发送…](#state:InputState.delivering) | 正在向智能体递交消息。请等待投递完成。 |
| [已发送](#state:InputState.delivered) | 智能体已收到消息。可在本轮查看回复。 |
| [已取消](#state:InputState.cancelled) | 消息在送达前已取消。如仍需处理，可以重新发送。 |
| [未发送](#state:InputState.failed) | 消息未能送达。重试前先在会话中查看原因。 |

### 程序收到的输入

| 页面上的文字 | 含义 |
| --- | --- |
| [已准备](#state:ModelInputState.prepared) | 输入已准备好，正在投递。请等待程序确认接收。 |
| [已发送](#state:ModelInputState.sent) | 程序已接收输入。可在会话中查看发送的内容。 |
| [发送失败](#state:ModelInputState.failed) | 程序拒绝了输入或投递失败。重试前先查看错误。 |
| [送达未确认](#state:ModelInputState.uncertain) | 程序尚未确认接收。重新发送前先查看会话中是否已有回复。 |

### 消息中的工作

| 页面上的文字 | 含义 |
| --- | --- |
| [进行中](#state:ItemStatus.in_progress) | 智能体仍在生成这部分回复。进展将显示在这里。 |
| [已完成](#state:ItemStatus.completed) | 本轮的这一部分已完成。打开可查看结果。 |
| [失败](#state:ItemStatus.failed) | 本轮的这一部分失败了。打开可查看错误。 |
| [已停止](#state:ItemStatus.cancelled) | 这项工作已停止。仍可查看已经显示的输出。 |
| [等待中](#state:ItemStatus.pending) | 有请求在等待。打开可答复或作出决定。 |
| [已批准](#state:ItemStatus.approved) | 请求已获准。可在本轮查看智能体的后续操作。 |
| [已答复](#state:ItemStatus.answered) | 有人已答复程序的问题。打开请求可查看答复。 |
| [已拒绝](#state:ItemStatus.denied) | 请求已被拒绝。打开可查看决定。 |
| [已过期](#state:ItemStatus.expired) | 决定期限已过。智能体需要重新提出请求才能再试。 |

### 定时消息

| 页面上的文字 | 含义 |
| --- | --- |
| [已启用](#state:InputScheduleState.active) | 系统会在所列时间发送消息。打开计划可查看下一次时间。 |
| [已暂停](#state:InputScheduleState.paused) | 后续发送已暂停。需要继续时可恢复计划。 |
| [已结束](#state:InputScheduleState.finished) | 这次一次性发送已结束。查看执行情况可了解是否已开始一轮工作。 |
| [已取消](#state:InputScheduleState.deleted) | 后续发送已删除。已进入队列的消息仍保留在会话中。 |

### 单次定时发送

| 页面上的文字 | 含义 |
| --- | --- |
| [已排队](#state:InputScheduleRunState.queued) | 定时消息已进入会话队列。满足条件后会开始处理。 |
| [暂缓执行](#state:InputScheduleRunState.blocked) | 定时消息已暂缓。打开会话可查看它在等待什么。 |
| [已开始](#state:InputScheduleRunState.started) | 这条消息已经启动了一轮工作。打开会话可查看进展。 |
| [未能开始](#state:InputScheduleRunState.failed) | 消息未能启动或已取消。可查看本次执行的详情。 |
| [已跳过](#state:InputScheduleRunState.skipped) | 本次未加入新消息。可查看当地时间是否被跳过，或上一次发送是否仍在等待。 |

### 收件箱通知

| 页面上的文字 | 含义 |
| --- | --- |
| [符合条件](#state:NoticeDecisionState.eligible) | 这条通知符合进入你收件箱的条件。外部投递情况可查看渠道状态。 |
| [未发送](#state:NoticeDecisionState.suppressed) | Orch 跳过了这条通知。打开详情可查看原因。 |
| [尚未判定](#state:NoticeDecisionState.not_evaluated) | Orch 没有判断是否应发送这条通知。仍可在会话中查看对应轮次。 |

### 为何没有发送通知

| 页面上的文字 | 含义 |
| --- | --- |
| [已中断](#state:NoticeSuppressionReason.interrupted) | 本轮被停止，因此跳过了常规结束通知。打开会话可查看结果。 |
| [尚未开始](#state:NoticeSuppressionReason.did_not_start) | 工作在本轮开始前失败了。打开会话可查看启动错误。 |
| [自动恢复中](#state:NoticeSuppressionReason.automatic_recovery) | 此故障正在自动恢复。可在会话中查看进展。 |
| [正在查看](#state:NoticeSuppressionReason.already_watching) | 本轮结束时你正在查看会话。结果已显示在会话中。 |
| [子智能体通知已关闭](#state:NoticeSuppressionReason.subagent_notices_off) | 当时已关闭子智能体通知。可调整通知偏好以用于之后的轮次。 |

### 外部渠道的投递情况

| 页面上的文字 | 含义 |
| --- | --- |
| [等待中](#state:ChannelAcceptanceState.pending) | 通知渠道尚未确认接收。稍后可再查看状态。 |
| [渠道已接收](#state:ChannelAcceptanceState.accepted) | 渠道已接收通知。设备上是否显示取决于该渠道。 |
| [尚未确认](#state:ChannelAcceptanceState.unknown) | 渠道可能已经接收通知。请求再次投递前可先到该渠道查看。 |

### 缺少的诊断详情

| 页面上的文字 | 含义 |
| --- | --- |
| [捕获已关闭](#state:RawCaptureOmissionCause.capture_off) | 当时未启用诊断采集。无法从本轮恢复这些详情。 |
| [脱敏依据不可用](#state:RawCaptureOmissionCause.masking_unavailable) | Orch 无法安全遮盖敏感值，因此省略了消息内容。其他可用详情仍可查看。 |
| [已达到捕获额度](#state:RawCaptureOmissionCause.capture_allowance) | 较新的诊断消息占用了可用空间。现在只能查看仍然保留的消息。 |
| [保留期限已过](#state:RawCaptureOmissionCause.retention_elapsed) | 这些详情已到保留期限并被清理。仍可阅读本轮对话。 |

## 机器

### 机器连接

| 页面上的文字 | 含义 |
| --- | --- |
| [在线](#state:MachineStatus.active) | 机器已连接。打开可查看它提供的环境。 |
| [离线](#state:MachineConnection.offline) | 机器已断开连接。可检查连接，也可等待它重新上线。 |
| [已丢失](#state:MachineConnection.lost) | 机器离线超过规定时间后，有人将它标为已丢失。仍可阅读会话，也可查看哪些环境可以恢复。 |
| [已吊销](#state:MachineStatus.revoked) | 此机器的注册已撤销。如需连接，须重新注册。 |

### 智能体程序连接

| 页面上的文字 | 含义 |
| --- | --- |
| [离线](#state:HarnessStatus.offline) | 无法连接运行程序的机器。请检查机器连接。 |
| [启动中](#state:HarnessStatus.starting) | 程序正在启动。请等待其上线。 |
| [在线](#state:HarnessStatus.online) | 程序已上线，可以接收工作。 |
| [出错](#state:HarnessStatus.error) | 程序报告错误。开始工作前可先打开详情。 |

### 网关连接

| 页面上的文字 | 含义 |
| --- | --- |
| [本地](#state:TunnelGatewayState.local) | 工具通过本地连接运行，无需网关即可使用。 |
| [连接中](#state:TunnelGatewayState.pending) | 正在建立网关连接。请等待连接完成。 |
| [工具连接](#state:TunnelGatewayState.attached) | 这里显示的网关正在转发此会话的工具请求。可在此查看正在使用哪个网关。 |
| [不可用](#state:TunnelGatewayState.unavailable) | 无法连接网关。重新连接前可检查网关状态。 |
| [尚未报告](#state:TunnelGatewayState.unknown) | 暂无网关状态。可查看连接详情。 |

## 环境

### 环境状态

| 页面上的文字 | 含义 |
| --- | --- |
| [未启动](#state:EnvironmentStatus.none) | 环境尚无运行中的工作区。开始工作前可查看设置。 |
| [正在创建](#state:EnvironmentStatus.provisioning) | 正在创建工作区。打开环境可查看准备进度。 |
| [运行中](#state:EnvironmentStatus.running) | 工作区正在运行。可在其中进行有权限的操作。 |
| [已停止](#state:EnvironmentStatus.stopped) | 工作区已停止。需要其中的程序或文件时可启动。 |
| [正在重建](#state:EnvironmentStatus.recreating) | 正在重新创建工作区。可在环境页面查看进度。 |
| [正在迁移](#state:EnvironmentStatus.moving) | 正在移动工作区。移动完成后再使用新位置。 |
| [失败](#state:EnvironmentStatus.failed) | 环境未能完成上次操作。可在环境页面查看错误。 |
| [离线](#state:EnvironmentStatus.offline) | 所在机器已断开。可检查连接并等待机器重新上线。 |
| [已丢失](#state:EnvironmentStatus.lost) | 机器已被标记为丢失。选择替代工作区前请查看恢复选项。 |
| [已停用](#state:EnvironmentStatus.disabled) | 此环境已停用。可向管理者询问能否启用。 |
| [已移除](#state:EnvironmentStatus.removed) | 环境已移除。可在移除详情中查看是否能恢复文件。 |

### 移除与恢复

| 页面上的文字 | 含义 |
| --- | --- |
| [已排队删除](#state:EnvironmentRemovalPhase.scheduled) | 移除工作已排队。可在移除详情中查看进展。 |
| [正在删除…](#state:EnvironmentRemovalPhase.removing) | 正在移除工作区。请等待结果，再决定是否恢复。 |
| [已删除；文件已保留](#state:EnvironmentRemovalPhase.kept) | 工作区已移除，存储仍然保留。可在保留期结束前查看恢复选项。 |
| [正在恢复…](#state:EnvironmentRemovalPhase.restoring) | 正在准备用于恢复的存储。可查看恢复进度。 |
| [正在删除保留文件…](#state:EnvironmentRemovalPhase.purging) | 正在删除保留的存储。请等待删除完成的确认。 |
| [已删除](#state:EnvironmentRemovalPhase.purged) | 存储已确认删除。无法再从此处恢复。 |
| [已丢失](#state:EnvironmentRemovalPhase.lost) | 机器被标记为丢失，因而无法确认是否已删除。可查看机器的恢复详情。 |

### 挂载存储

| 页面上的文字 | 含义 |
| --- | --- |
| [已挂载](#state:EnvMountState.attached) | 存储已挂载，可在工作区中通过所列路径访问。 |
| [不可用](#state:EnvMountState.unavailable) | 存储未能挂载。可查看挂载详情了解原因。 |
| [等待挂载](#state:EnvMountState.pending) | 存储尚未创建或挂载。设置完成后可再查看。 |

### 文件快照

| 页面上的文字 | 含义 |
| --- | --- |
| [正在创建](#state:SnapshotState.running) | 正在保存工作区文件。请等待快照完成。 |
| [完成](#state:SnapshotState.complete) | 此快照选中的文件均已保存。恢复前可查看文件。 |
| [部分完成](#state:SnapshotState.partial) | 仅保存了部分文件。恢复前可查看有哪些文件可用。 |
| [失败](#state:SnapshotState.failed) | 快照失败。尝试从中恢复前请查看错误。 |
| [不可用](#state:SnapshotState.unavailable) | 完整快照不可用。可打开详情查看原因及其他选择。 |
| [已过期](#state:SnapshotState.expired) | 保存的文件已过期。仍可在本轮查看为何需要其他来源。 |

### 文件传输

| 页面上的文字 | 含义 |
| --- | --- |
| [正在传输](#state:FileTransferState.transferring) | 机器已开始发送文件。可打开详情查看最近一部分的到达时间。 |
| [已中断](#state:FileTransferState.interrupted) | 传输中断，文件尚未收全。可在详情中查看到期时间。 |
| [已到期](#state:FileTransferState.expired) | 未完成的传输已过期，已收到的部分也已清理。如仍需要该文件，可重新发送。 |

### 打开文件版本

| 页面上的文字 | 含义 |
| --- | --- |
| [预览不可用](#state:FetchStatus.unavailable) | 无法获取此版本。可查看文件旁的原因。 |

### 等待文件

| 页面上的文字 | 含义 |
| --- | --- |
| [环境已离线；此文件直接从该环境读取。](#state:FetchUnavailable.offline) | 工作区重新连接后才能打开其中的文件。可检查环境连接。 |

### 已存上传

| 页面上的文字 | 含义 |
| --- | --- |
| [完成](#state:UploadInventoryState.complete) | 已校验完整上传。可在有权限的位置使用该文件。 |
| [部分完成](#state:UploadInventoryState.partial) | 只收到了部分文件。可查看上传情况，再决定是否继续。 |
| [已过期](#state:UploadInventoryState.expired) | 上传未在期限内完成。如仍需要文件，可重新上传。 |

### 检查上传

| 页面上的文字 | 含义 |
| --- | --- |
| [正在核验](#state:UploadPhase.verifying) | 文件各部分均已送达，接收方正在校验。请等待确认，无需重传。 |

### 查找工作区文件

| 页面上的文字 | 含义 |
| --- | --- |
| [无法读取规则](#state:EnvironmentIndexReason.unreadable) | 部分文件信息或排除规则无法读取。可直接打开文件查看当前内容。 |

## 提供方

### 提供方是否可用

| 页面上的文字 | 含义 |
| --- | --- |
| [需要设置](#state:ProviderStatus.needs_setup) | 设置尚未完成。打开提供方可查看缺少哪个步骤。 |
| [就绪](#state:ProviderStatus.ready) | 提供方可用。选择你有权使用的模型即可开始。 |
| [额度已用尽](#state:ProviderStatus.exhausted) | 提供方已用完额度。可在用量窗口查看下次重置时间。 |
| [出错](#state:ProviderStatus.error) | 提供方报告了问题。打开详情可查看原因和恢复选项。 |
| [离线](#state:ProviderStatus.offline) | 无法连接提供方。重试前先检查其机器或连接。 |
| [已停用](#state:ProviderStatus.disabled) | 提供方已停用。如需使用，可向管理者询问是否启用。 |

### 返回原来的模型

| 页面上的文字 | 含义 |
| --- | --- |
| [就绪后在后续轮次返回](#state:CallReturnState.when_ready) | 原来的模型可用后，后续轮次可能会使用它。可在模型详情中查看当前使用的模型。 |
| [继续使用故障切换模型](#state:CallReturnState.stay_on_failover) | 会话将继续使用替代模型。可打开模型选择器查看可选项。 |
| [正在返回](#state:CallReturnState.returning) | 正在切回原来的模型。可在会话中查看当前使用的模型。 |
| [尚未报告](#state:CallReturnState.unreported) | 会话中没有返回原模型的相关信息。可在继续前查看当前使用的模型。 |

### 重新试用提供方

| 页面上的文字 | 含义 |
| --- | --- |
| [已预留](#state:ProviderTrialState.reserved) | 已有一条消息被选中试用此提供方。可打开试用详情查看是哪条消息。 |
| [正在运行](#state:ProviderTrialState.running) | 选中的轮次正在试用此提供方。可打开会话查看进度。 |
| [已使用](#state:ProviderTrialState.consumed) | 这次试用机会已用完。可查看轮次结果，了解提供方是否应答。 |
| [未预留](#state:ProviderTrialState.not_reserved) | 这次检查没有预留消息进行试用。可在发送任务前查看提供方的当前状态。 |

### 模型信息

| 页面上的文字 | 含义 |
| --- | --- |
| [此来源不列出模型；请按名称添加。](#state:MemberState.unsupported) | 此来源不提供这些详情。可查看提供方允许自行填写哪些内容。 |
| [模型 · 尚未读取](#state:MemberState.not_read_yet) | 这些详情可读取，但尚未获取。若提供方页面有读取选项，可在那里操作。 |
| [此来源没有列出模型。](#state:MemberState.empty) | 提供方在这一项没有返回任何内容。若与预期不符，可检查账号或配置。 |
| [来源报告的模型](#state:MemberState.told) | 提供方已给出这些详情。展开对应部分即可查看。 |
| [未提供 · 需要管理密钥](#state:MemberState.unsupported_because) | 此项需要管理员密钥。可向提供方的管理者询问访问权限。 |

### 用量限额

| 页面上的文字 | 含义 |
| --- | --- |
| [可用](#state:CapState.ok) | 当前工作尚在额度内。可在用量页面查看剩余额度。 |
| [接近上限](#state:CapState.warning) | 用量已超过提醒值。开始更多工作前可查看剩余额度。 |
| [已达上限](#state:CapState.reached) | 额度已用完。打开用量页面可查看限制以及何时可能恢复。 |

## 智能体程序

### 检查程序版本

| 页面上的文字 | 含义 |
| --- | --- |
| [未检查](#state:HarnessBuildStatus.untested) | 此程序版本尚未检查。使用前可先运行检查。 |
| [检查中](#state:HarnessBuildStatus.checking) | 程序检查正在运行。结果会显示在这里。 |
| [已检查](#state:HarnessBuildStatus.checked) | 此版本已通过检查。可在支持的位置选择它。 |
| [未通过](#state:HarnessBuildStatus.failed) | 此版本检查失败。使用前请查看结果。 |

### 查询新版本

| 页面上的文字 | 含义 |
| --- | --- |
| [无发布渠道](#state:ProgramChannelState.none) | 此程序没有自动获取版本的来源。可直接查看已安装版本。 |
| [尚未读取](#state:ProgramChannelState.unread) | 已配置版本来源，但尚未成功读取。可检查其地址。 |
| [已读取](#state:ProgramChannelState.ready) | 版本信息已成功读取。可打开版本列表查看。 |
| [读取失败](#state:ProgramChannelState.failed) | 上次查询版本失败。重试前可查看原因。 |
| [已停用](#state:ProgramChannelState.disabled) | 自动版本检查已关闭。可向管理员询问检查间隔。 |

### 扩展

| 页面上的文字 | 含义 |
| --- | --- |
| [运行中](#state:PluginState.running) | 扩展正在运行。可在已配置的位置使用其功能。 |
| [重启中](#state:PluginState.restarting) | 扩展正在重新启动。请等待恢复运行。 |
| [已失败](#state:PluginState.failed) | 扩展运行失败。打开详情可查看需要处理的问题。 |

### 机器上发现的 MCP 服务

| 页面上的文字 | 含义 |
| --- | --- |
| [已发现](#state:McpOfferingState.found) | 机器上发现了服务定义。选择使用前可先审阅。 |
| [连接中…](#state:McpOfferingState.pending) | 机器正在保存选中的定义。请等待确认。 |
| [可用](#state:McpOfferingState.ready) | 现在可在此机器上使用已保存的定义。 |
| [导入失败](#state:McpOfferingState.failed) | 机器无法保存定义。可查看原因及本机设置。 |

### MCP 服务状态

| 页面上的文字 | 含义 |
| --- | --- |
| [启动中](#state:McpState.starting) | 服务正在启动。请等待工具可用。 |
| [运行中](#state:McpState.running) | 服务正在运行。可查看此会话可用的工具。 |
| [已停止](#state:McpState.stopped) | 服务已停止。需要使用其工具时可启动。 |
| [失败](#state:McpState.failed) | 服务启动或运行失败。重试前请查看错误。 |
| [不可用](#state:McpState.unavailable) | 无法在此处使用服务。打开详情可查看原因。 |

### 子智能体

| 页面上的文字 | 含义 |
| --- | --- |
| [运行中](#state:SubagentState.running) | 子智能体正在工作。可在主会话中查看进展。 |
| [已完成](#state:SubagentState.completed) | 子智能体已完成工作。可在主会话中阅读结果。 |
| [失败](#state:SubagentState.failed) | 子智能体失败了。打开详情可查看错误。 |
| [已停止](#state:SubagentState.interrupted) | 子智能体已停止。开始更多工作前可查看最后的输出。 |

## 审批与问题

### 答复请求

| 页面上的文字 | 含义 |
| --- | --- |
| [等待中](#state:ApprovalState.pending) | 需要有人作出决定。打开请求可查看智能体想做什么。 |
| [已批准](#state:ApprovalState.approved) | 请求已获准。可在会话中查看后续情况。 |
| [已答复](#state:ApprovalState.answered) | 已有人提供所需答复。打开请求可阅读答复。 |
| [已拒绝](#state:ApprovalState.denied) | 请求已被拒绝。打开可查看决定。 |
| [已取消](#state:ApprovalState.cancelled) | 请求者已撤回请求。无需再作决定。 |
| [已过期](#state:ApprovalState.expired) | 决定期限已过。如仍需执行，请重新提出请求。 |

### 送达你的决定

| 页面上的文字 | 含义 |
| --- | --- |
| [决定已接受 · 等待送达](#state:ApprovalDeliveryState.pending) | 你的决定已接受，正在等待送达智能体程序。可打开详情查看上次尝试。 |
| [已送达](#state:ApprovalDeliveryState.applied) | 智能体程序已确认收到你的决定。可在会话中查看后续进展。 |
| [决定已接受 · 送达情况未知](#state:ApprovalDeliveryState.unknown) | Orch 已接受你的决定，但尚未确认是否送达。可打开详情查看上次尝试及其结果。 |

### 请求列表

| 页面上的文字 | 含义 |
| --- | --- |
| [待我处理](#state:ApprovalListState.pending) | 这些请求仍在等待答复。选择一项可查看谁有权决定。 |
| [历史](#state:ApprovalListState.decided) | 这些请求已有决定。打开可查看结果。 |

### 自动审批不可用的原因

| 页面上的文字 | 含义 |
| --- | --- |
| [此机器上的自动审批检查尚未通过。](#state:ApprovalChoiceReason.auto_not_proven) | 此版本尚未在这台机器上通过自动审批检查。可选用其他可用模式，或请管理者检查。 |
| [此机器上的自动审批检查失败。](#state:ApprovalChoiceReason.auto_check_failed) | 此版本未通过自动审批检查。可选用其他模式，并查看检查结果。 |
| [此程序没有可用的自动审批检查。](#state:ApprovalChoiceReason.auto_check_unavailable) | 此程序没有可用的自动审批检查。请选择受支持的审批模式。 |

### 检查自动审批

| 页面上的文字 | 含义 |
| --- | --- |
| [自动：尚未在此机器上验证。](#state:AutoApprovalState.not_proven) | 所选版本尚未在这台机器上通过自动审批检查。管理者可运行检查。 |
| [自动：未通过此机器上的检查。](#state:AutoApprovalState.failed) | 此版本未通过自动审批检查。可查看结果或使用其他可用模式。 |
| [已检查](#state:AutoApprovalState.passed) | 此版本已通过自动审批检查。能否使用还取决于审核者权限和会话限制。 |
| [自动：此程序没有可用的自动审批检查。](#state:AutoApprovalState.unavailable) | 此程序无法在此处验证自动审批。请选择提供的其他模式。 |

## 端口与视图

### 端口上的程序

| 页面上的文字 | 含义 |
| --- | --- |
| [正在监听](#state:PortTargetState.observed) | 有程序正在监听此端口。有权限时可打开视图。 |
| [环境已停止](#state:PortTargetState.stopped) | 环境已停止。启动后端口才能恢复可用。 |
| [没有监听项](#state:PortTargetState.not_listening) | 没有程序监听此端口。请启动所需服务并检查端口号。 |
| [不可用](#state:PortTargetState.unavailable) | 目前无法检查端口。可查看环境及其连接情况。 |

### 共享端口

| 页面上的文字 | 含义 |
| --- | --- |
| [等待中](#state:PortOpeningState.waiting) | 管理者正在审阅请求。共享地址前先查看状态。 |
| [正在开放…](#state:PortOpeningState.opening) | 访问已获准，正在打开端口。请等待端口可用。 |
| [已开放](#state:PortOpeningState.open) | 获准的人员可通过所列地址访问端口。 |
| [环境已停止](#state:PortOpeningState.stopped) | 访问已获准，但环境已停止。请先启动环境。 |
| [没有监听项](#state:PortOpeningState.not_listening) | 访问已获准。请启动应在此端口监听的程序。 |
| [不可用](#state:PortOpeningState.unavailable) | 访问已获准，但无法连接端口。可检查环境连接。 |
| [已拒绝](#state:PortOpeningState.refused) | 共享端口的请求已被拒绝。重新申请前请查看决定。 |
| [已关闭](#state:PortOpeningState.closed) | 此次开放已结束。如需再次共享，请重新开放端口。 |

### 从外部打开视图

| 页面上的文字 | 含义 |
| --- | --- |
| [就绪](#state:OutsideWayState.ready) | 外部地址已就绪。可按所列权限访问。 |
| [需要关注](#state:OutsideWayState.needs_attention) | 使用此地址前需要处理问题。请查看连接详情。 |

### 终端会话

| 页面上的文字 | 含义 |
| --- | --- |
| [在提示符处](#state:TerminalState.open) | 终端已连接。提示符就绪后可输入命令。 |
| [已关闭](#state:TerminalState.closed) | 此终端已结束。如需继续输入命令，可启动新终端。 |

### 浏览器会话

| 页面上的文字 | 含义 |
| --- | --- |
| [正在启动](#state:BrowserState.opening) | 浏览器正在启动。请等待页面出现。 |
| [已打开](#state:BrowserState.open) | 浏览器已可用。打开视图即可操作页面。 |
| [已关闭](#state:BrowserState.closed) | 此浏览器会话已结束。如需继续，可开启新会话。 |

## 目标

### 目标进度

| 页面上的文字 | 含义 |
| --- | --- |
| [工作中](#state:GoalStatus.active) | 智能体正在朝目标工作。可在各轮对话中查看进展。 |
| [已暂停](#state:GoalStatus.paused) | 目标工作已暂停。恢复前可先查看原因。 |
| [已达成](#state:GoalStatus.met) | 目标已通过检查，或有人确认了证据。打开目标可查看结果。 |
| [已停止](#state:GoalStatus.stopped) | 达到目标前工作已停止。查看原因可判断能否继续。 |
| [小陈 清除了目标](#state:GoalStatus.cleared) | 有人清除了目标。如仍需继续工作，可设置新目标。 |

### 智能体报告

| 页面上的文字 | 含义 |
| --- | --- |
| [工作中](#state:GoalReportStatus.working) | 智能体报告工作仍在进行。可在会话中查看后续进展。 |
| [已达成](#state:GoalReportStatus.met) | 智能体报告已达到条件。确认完成前请查看证据和检查结果。 |
| [等你处理](#state:GoalReportStatus.blocked) | 智能体需要帮助才能继续。请阅读报告并处理所列障碍。 |

### 等待你决定的目标

| 页面上的文字 | 含义 |
| --- | --- |
| [完成汇报需要由人员作出决定](#state:GoalReason.unchecked) | 完成情况尚未得到独立核验。请查看证据，再选择标记达成或继续工作。 |

### 核验结果

| 页面上的文字 | 含义 |
| --- | --- |
| [尚未检查](#state:GoalOutcome.unchecked) | 可查看报告及其证据。你可以确认完成，也可以让智能体继续工作。 |

### 确认达成目标的人

| 页面上的文字 | 含义 |
| --- | --- |
| [由人员确认](#state:GoalCheckedBy.person) | 有人审阅证据后确认了完成。打开目标历史可查看是谁作出的决定。 |

## 后台工作

### 查看后台工作

| 页面上的文字 | 含义 |
| --- | --- |
| [后台工作](#state:BackgroundState.live) | 会话已连接。打开任务可查看当前工作。 |
| [智能体程序已离线，无法确认哪些命令仍在运行。](#state:BackgroundState.offline) | 会话或机器已断开。重新连接后可再次查看。 |
| [无法控制智能体程序的命令。](#state:BackgroundState.unavailable) | 程序目前无法报告后台工作。可在会话中查看详情。 |

### 后台任务

| 页面上的文字 | 含义 |
| --- | --- |
| [运行中](#state:BackgroundTaskState.running) | 任务正在运行。打开可查看输出。 |
| [已完成](#state:BackgroundTaskState.completed) | 任务已完成。可在会话中打开结果。 |
| [失败](#state:BackgroundTaskState.failed) | 任务失败。重新启动前请查看错误。 |
| [已结束](#state:BackgroundTaskState.stopped) | 任务已停止。可查看停止前已完成的工作。 |
| [已丢失](#state:BackgroundTaskState.lost) | 程序已失去运行该任务的会话。重复执行前请查看最后的输出。 |

### 结束命令

| 页面上的文字 | 含义 |
| --- | --- |
| [已结束](#state:BackgroundEndState.ended) | 程序已确认命令结束。如有需要，可查看最终输出。 |
| [此前已结束](#state:BackgroundEndState.already_ended) | 请求停止时命令已经结束。无需再次停止。 |
| [仍在运行](#state:BackgroundEndState.still_running) | 命令仍在运行。查看输出后可决定是否再次停止。 |
| [无法确认是否结束](#state:BackgroundEndState.unknown) | 程序无法确认命令已结束。再次执行前请检查当前状态。 |

### 作业

| 页面上的文字 | 含义 |
| --- | --- |
| [等待开始](#state:JobState.queued) | 作业正在排队。有可用的执行资源时便会启动。 |
| [进行中](#state:JobState.running) | 作业正在运行。打开可查看进度。 |
| [已完成](#state:JobState.succeeded) | 作业已成功完成。打开结果可查看变更。 |
| [失败](#state:JobState.failed) | 作业因错误停止。重试前可查看最后完成的步骤。 |
| [已取消](#state:JobState.cancelled) | 作业已取消。可查看取消前已完成的部分。 |
| [已丢失](#state:JobState.lost) | Orch 已失去与运行中作业的联系。重新启动前请检查实际变更。 |

### 数据库请求超时

| 页面上的文字 | 含义 |
| --- | --- |
| [等待数据库连接已超时](#state:DatabaseWaitPhase.acquire) | Orch 未能及时连上数据库。可稍后重试。 |
| [数据库语句执行超时](#state:DatabaseWaitPhase.statement) | 操作超时，数据库已将其停止。可打开详情查看时限。 |
| [数据库事务已超时](#state:DatabaseWaitPhase.idle_transaction) | 一项操作等待过久，数据库已将其关闭。可刷新页面查看当前结果。 |

### 作业步骤

| 页面上的文字 | 含义 |
| --- | --- |
| [等待中](#state:PhaseState.pending) | 此步骤尚未开始。可先查看正在执行的步骤。 |
| [进行中](#state:PhaseState.running) | 此步骤正在运行。完成后会在这里显示结果。 |
| [已完成](#state:PhaseState.done) | 此步骤已完成。后续步骤会显示整体结果。 |
| [已跳过](#state:PhaseState.skipped) | 此步骤已跳过。查看作业详情可了解实际执行了哪些步骤。 |

### 语义搜索

| 页面上的文字 | 含义 |
| --- | --- |
| [已关闭](#state:SemanticState.off) | 语义搜索已关闭。仍可按文字搜索。 |
| [未设置](#state:SemanticState.not_named) | 请先为语义搜索选择提供方和模型，之后才能建立索引。 |
| [不可用](#state:SemanticState.unavailable) | 数据库目前无法支持语义搜索。仍可按文字搜索。 |
| [就绪](#state:SemanticState.idle) | 所选文本均已编入索引。现在可使用语义搜索。 |
| [正在索引](#state:SemanticState.indexing) | Orch 正在准备语义搜索所需的文本。随着处理完成，会有更多结果可用。 |
| [已暂停](#state:SemanticState.paused) | 索引已暂停。可根据所列原因检查存储、提供方状态、权限或用量限额。 |
| [需要选择](#state:SemanticState.waiting) | 存储无法同时容纳新旧索引。需要管理员选择如何继续。 |

### 重试搜索更新

| 页面上的文字 | 含义 |
| --- | --- |
| [可重试索引时间](#state:SemanticRetryNext.immediate) | 容量允许时即可尝试较小的请求。可在此查看进展。 |
| [下一次索引检查](#state:SemanticRetryNext.indexer_pass) | 下一次索引检查时会再次考虑此请求。可让它继续排队。 |
| [已完成](#state:SemanticRetryNext.finished) | 重试已结束。可打开结果查看是否还需处理。 |

### 导出

| 页面上的文字 | 含义 |
| --- | --- |
| [正在开始](#state:ExportState.pending) | 正在准备导出。就绪后即可下载。 |
| [可下载](#state:ExportState.ready) | 导出已就绪。请在过期前下载。 |
| [失败](#state:ExportState.failed) | 导出在保存文件前失败或被取消。重试前可查看作业详情。 |
| [已过期](#state:ExportState.expired) | 导出文件已移除。如仍需副本，可重新导出。 |

### 导入

| 页面上的文字 | 含义 |
| --- | --- |
| [进行中](#state:ImportState.pending) | 正在准备或执行导入。打开作业可查看进度。 |
| [完成](#state:ImportState.done) | 导入已完成。查看报告可了解各个所选项目的结果。 |
| [失败](#state:ImportState.failed) | 导入遇错停止。请查看报告了解哪些项目已创建。 |
| [已取消](#state:ImportState.cancelled) | 导入已取消。报告会列出已经创建的项目。 |
| [正在撤销](#state:ImportState.undoing) | Orch 正在撤销此次导入。可查看作业直至完成。 |
| [已撤销](#state:ImportState.undone) | 此次导入创建的项目已撤销。详情可查看报告。 |

## 人员与共享

### 组织设置

| 页面上的文字 | 含义 |
| --- | --- |
| [已配置](#state:FirstStepsState.done) | Orch 已确认此设置步骤完成。可继续下一项未完成的步骤。 |
| [尚未完成](#state:FirstStepsState.open) | 此设置步骤尚未完成。打开可查看所需操作。 |
| [暂时未知](#state:FirstStepsState.unknown) | Orch 无法检查此步骤。请检查访问权限后重试。 |

### 页面访问

| 页面上的文字 | 含义 |
| --- | --- |
| [继承 所在位置 的权限](#state:PageState.follows) | 此页沿用上级共享设置。可在上级查看谁能使用。 |
| [自定义](#state:PageState.custom) | 此页使用单独的共享设置。打开本页的共享设置可查看。 |
| [私有](#state:PageState.restricted) | 此页限制访问。如需使用，可向所有者申请。 |

### 谁能使用共享资源

| 页面上的文字 | 含义 |
| --- | --- |
| [对此材料和接收会话有效](#state:AgentShareCurrentState.active) | 为此人申请的权限在此处全部生效。打开共享详情可查看。 |
| [当前限制降低了权限](#state:AgentShareCurrentState.reduced) | 所申请的权限仅部分生效。可查看此人在这里可使用哪些权限。 |
| [当前未生效](#state:AgentShareCurrentState.inactive) | 此项共享未在这里授予权限。若此人仍能访问，可检查其他共享来源。 |
| [按具体材料或接收会话评估](#state:AgentShareCurrentState.unassessed) | 需针对具体人员和资源检查权限。选择人员和资源可查看实际权限。 |

### 共享给会话的文件

| 页面上的文字 | 含义 |
| --- | --- |
| [等待送达](#state:AgentShareSnapshotState.pending) | 共享文件将在会话的下一轮送达。如仍需使用，可保留在队列中。 |
| [已送达](#state:AgentShareSnapshotState.delivered) | 共享文件已写入会话的工作目录。打开工作区即可查看。 |
| [已收回](#state:AgentShareSnapshotState.revoked) | 共享已撤回。再次共享前请检查当前权限。 |

### 活动历史中的对象

| 页面上的文字 | 含义 |
| --- | --- |
| [打开](#state:AuditObjectState.live) | 此对象仍存在。有权限时可从活动历史中打开。 |
| [已删除](#state:AuditObjectState.deleted) | 此对象已删除，目前仍保留以便恢复。可查看保留详情。 |
| [已于11 月 20 日彻底移除](#state:AuditObjectState.purged) | 已完成永久删除。仍可在活动历史中查看发生了什么。 |

## 登录

### 账号访问

| 页面上的文字 | 含义 |
| --- | --- |
| [正常](#state:AdminUserStatus.active) | 此人可以登录，并使用已获准的权限。 |
| [已停用](#state:AdminUserStatus.suspended) | 此人的登录和密钥已停用。暂停访问期间，其工作仍归本人所有。 |

### 登录验证

| 页面上的文字 | 含义 |
| --- | --- |
| [页面无法加载Turnstile：网站入口的策略缺少 challenges.cloudflare.com。见运维指南。](#state:HumanCheckEdgeState.blocks) | 网站策略阻止了部分登录验证。请联系管理员，允许提示中列出的主机。 |
| [无法从这里读取网站入口的策略。见运维指南。](#state:HumanCheckEdgeState.unknown) | Orch 无法读取网站策略。管理员可检查验证组件是否能加载。 |
