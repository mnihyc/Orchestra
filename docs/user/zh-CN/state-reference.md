---
group: work
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [state, status, phase, reference, 状态, 阶段, 参考]
heading_id.条目: entries
summary: 线程、提供方或机器旁显示的每种状态的含义。
---

# 状态参考

状态表示已报告的事实。离线可能恢复；丢失表示已明确作出永久不可达的决定。失败或不可用状态会保留原因，以及页面实际提供的处理方式。以下内部标识用于把详情对应到正确对象；名称相近的状态并不具有完全相同的含义。另见[线程状态](thread-states.md)和[错误处理](error-reference.md)。

## 条目

### ThreadDisplayState

| 参考标识 | 含义 |
| --- | --- |
| `state:ThreadDisplayState.waiting_approval` | 有请求或暂存输入等待人工处理。 |
| `state:ThreadDisplayState.blocked` | 线程无法继续。 |
| `state:ThreadDisplayState.failed` | 上个轮次失败，此后没有接受新的输入。 |
| `state:ThreadDisplayState.running` | 轮次正在启动或运行。 |
| `state:ThreadDisplayState.queued` | 消息排队等待开始。 |
| `state:ThreadDisplayState.held` | 有人暂停了队列，需选择继续。 |
| `state:ThreadDisplayState.finished` | 已完成的轮次尚未被当前读者查看。 |
| `state:ThreadDisplayState.idle` | 没有工作或未读结果需要状态图标。 |
| `state:ThreadDisplayState.archived` | 线程或其上级位置已归档。 |

### PageState

| 参考标识 | 含义 |
| --- | --- |
| `state:PageState.follows` | 所有权限行均沿用默认值。 |
| `state:PageState.custom` | 有人修改了权限行。 |
| `state:PageState.restricted` | 查看权限不再沿用默认值，形成受限共享。 |

### OutsideWayState

| 参考标识 | 含义 |
| --- | --- |
| `state:OutsideWayState.ready` | 已启用，当前未发现问题。 |
| `state:OutsideWayState.needs_attention` | 失败、目标或通道不可用，并显示修复方法。 |

### EnvironmentStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:EnvironmentStatus.none` | 仅有记录，首次使用时再创建。 |
| `state:EnvironmentStatus.provisioning` | 正在创建。 |
| `state:EnvironmentStatus.running` | 已启动。 |
| `state:EnvironmentStatus.stopped` | 已停止。 |
| `state:EnvironmentStatus.recreating` | 正在用新单元替换。 |
| `state:EnvironmentStatus.moving` | 正在迁移到另一台主机。 |
| `state:EnvironmentStatus.failed` | 运行失败。 |
| `state:EnvironmentStatus.offline` | 暂时无法连接其机器。 |
| `state:EnvironmentStatus.lost` | 已明确声明为永久无法连接。 |
| `state:EnvironmentStatus.disabled` | 已禁用。 |
| `state:EnvironmentStatus.removed` | 单元已移除，自有文件按记录的移除流程处理。 |

### TopicStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:TopicStatus.open` | 工作仍在继续。 |
| `state:TopicStatus.done` | 工作已完成。 |

### BackgroundTaskState

| 参考标识 | 含义 |
| --- | --- |
| `state:BackgroundTaskState.running` | 原生后台工作仍在运行。 |
| `state:BackgroundTaskState.completed` | 原生工作已成功完成。 |
| `state:BackgroundTaskState.failed` | 原生工作失败结束。 |
| `state:BackgroundTaskState.stopped` | 有人明确结束了工作。 |
| `state:BackgroundTaskState.lost` | 所属会话已消失。 |

### ThreadState

| 参考标识 | 含义 |
| --- | --- |
| `state:ThreadState.idle` | 等待新的消息。 |
| `state:ThreadState.starting` | 正在准备开始轮次。 |
| `state:ThreadState.running` | 轮次正在运行。 |
| `state:ThreadState.waiting_approval` | 轮次正在等待审批。 |
| `state:ThreadState.blocked` | 线程无法继续。 |

### HarnessBuildStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:HarnessBuildStatus.untested` | 尚未检查，作为未测试版本提供。 |
| `state:HarnessBuildStatus.checking` | 正在检查。 |
| `state:HarnessBuildStatus.checked` | 接口和原生用例已通过，或是本次发布采用的版本。 |
| `state:HarnessBuildStatus.failed` | 检查发现适配器需要的能力缺失，或未能执行；请查看检查详情。 |

### ProviderStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:ProviderStatus.needs_setup` | 需要完成设置。 |
| `state:ProviderStatus.ready` | 已就绪。 |
| `state:ProviderStatus.exhausted` | 提供商最近报告额度耗尽；这是观测结果，不是本地配额门禁。 |
| `state:ProviderStatus.error` | 提供方发生错误。 |
| `state:ProviderStatus.offline` | 暂时无法连接其机器。 |
| `state:ProviderStatus.disabled` | 已禁用。 |

### SourceState

| 参考标识 | 含义 |
| --- | --- |
| `state:SourceState.pending` | 等待准备源文件。 |
| `state:SourceState.preparing` | 正在准备。 |
| `state:SourceState.ready` | 目录已就绪。 |
| `state:SourceState.failed` | 准备失败，保留固定来源和等待中的输入。 |

### JobState

| 参考标识 | 含义 |
| --- | --- |
| `state:JobState.queued` | 等待轮到此任务。 |
| `state:JobState.running` | 正在执行。 |
| `state:JobState.succeeded` | 已成功完成。 |
| `state:JobState.failed` | 执行失败。 |
| `state:JobState.cancelled` | 有人取消了任务。 |
| `state:JobState.lost` | 此工作需要的机器已被明确声明丢失。 |

### GoalStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:GoalStatus.active` | Orch 继续让线程朝此目标工作。 |
| `state:GoalStatus.paused` | 有人暂停，或代理报告阻塞。 |
| `state:GoalStatus.met` | 目标条件已满足。 |
| `state:GoalStatus.stopped` | 未达成即停止：达到限制或无人可继续。 |
| `state:GoalStatus.cleared` | 有人清除了目标。 |

### GoalReportStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:GoalReportStatus.working` | 正在朝目标工作。 |
| `state:GoalReportStatus.met` | 目标条件已满足。 |
| `state:GoalReportStatus.blocked` | 需要人工处理才能继续。 |

### FirstStepsState

| 参考标识 | 含义 |
| --- | --- |
| `state:FirstStepsState.done` | 已观察到完成。 |
| `state:FirstStepsState.open` | 尚未完成。 |
| `state:FirstStepsState.unknown` | 无法观察此项是否完成。 |

### PortOpeningState

| 参考标识 | 含义 |
| --- | --- |
| `state:PortOpeningState.waiting` | 等待管理者决定。 |
| `state:PortOpeningState.opening` | 已接受开放，等待首次主机绑定。 |
| `state:PortOpeningState.open` | 已接受，当前正在提供连接。 |
| `state:PortOpeningState.stopped` | 已接受，但环境已停止。 |
| `state:PortOpeningState.not_listening` | 已接受，但没有程序监听目标。 |
| `state:PortOpeningState.unavailable` | 已接受，但当前不可用。 |
| `state:PortOpeningState.refused` | 审阅后的开放请求被拒绝。 |
| `state:PortOpeningState.closed` | 开放授权已结束，重启不会恢复。 |

### AuditObjectState

| 参考标识 | 含义 |
| --- | --- |
| `state:AuditObjectState.live` | 对象仍存在。 |
| `state:AuditObjectState.deleted` | 已删除，但仍保留记录。 |
| `state:AuditObjectState.purged` | 已确认物理清除。 |

### EnvMountState

| 参考标识 | 含义 |
| --- | --- |
| `state:EnvMountState.attached` | 已挂载。 |
| `state:EnvMountState.unavailable` | 无法挂载。 |
| `state:EnvMountState.pending` | 尚未创建或挂载。 |

### EnvironmentRemovalPhase

| 参考标识 | 含义 |
| --- | --- |
| `state:EnvironmentRemovalPhase.scheduled` | 等待移除单元。 |
| `state:EnvironmentRemovalPhase.removing` | 正在移除单元。 |
| `state:EnvironmentRemovalPhase.kept` | 保留自有卷。 |
| `state:EnvironmentRemovalPhase.restoring` | 正在为恢复保留卷。 |
| `state:EnvironmentRemovalPhase.purging` | 正在移除保留的卷。 |
| `state:EnvironmentRemovalPhase.purged` | 已确认物理移除。 |
| `state:EnvironmentRemovalPhase.lost` | 主机已声明丢失，尚未确认删除。 |

### CapState

| 参考标识 | 含义 |
| --- | --- |
| `state:CapState.ok` | 仍有剩余额度。 |
| `state:CapState.warning` | 已达到警告比例。 |
| `state:CapState.reached` | 已达到限额。 |

### MemberState

| 参考标识 | 含义 |
| --- | --- |
| `state:MemberState.unsupported` | 数据源未声明此事实，也从未报告过。 |
| `state:MemberState.not_read_yet` | 数据源声明支持，但尚未报告过。 |
| `state:MemberState.empty` | 报告为空值或空列表，表示没有提供。 |
| `state:MemberState.told` | 已报告具体值。 |
| `state:MemberState.unsupported_because` | 数据源需要管理员密钥才能提供此事实。这表示该事实暂不提供，并非模型调用失败。 |

### HarnessStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:HarnessStatus.offline` | 暂时无法连接其机器。 |
| `state:HarnessStatus.starting` | 正在启动。 |
| `state:HarnessStatus.online` | 已连接。 |
| `state:HarnessStatus.error` | 程序发生错误。 |

### MachineStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:MachineStatus.active` | 机器已注册。 |
| `state:MachineStatus.revoked` | 注册已撤回，会话已断开。 |

### McpOfferingState

| 参考标识 | 含义 |
| --- | --- |
| `state:McpOfferingState.found` | 发现了不同或尚未记录的定义，尚未选择。 |
| `state:McpOfferingState.pending` | 正按连接器本机策略复制发现的定义。 |
| `state:McpOfferingState.ready` | 受保护的定义已在该机器上可用。 |
| `state:McpOfferingState.failed` | 本机策略或原生来源拒绝复制。 |

### AgentShareCurrentState

| 参考标识 | 含义 |
| --- | --- |
| `state:AgentShareCurrentState.active` | 所请求的全部区域权限在此对象上对该接收者生效。 |
| `state:AgentShareCurrentState.reduced` | 所请求的部分区域权限在此对象上对该接收者生效。 |
| `state:AgentShareCurrentState.inactive` | 此贡献在当前评估上下文中不提供权限。 |
| `state:AgentShareCurrentState.unassessed` | 仍需逐对象或逐接收者评估，并不表示授权已失效。 |

### PortTargetState

| 参考标识 | 含义 |
| --- | --- |
| `state:PortTargetState.observed` | 已观察到可用且属于该环境的绑定。 |
| `state:PortTargetState.stopped` | 其环境已停止。 |
| `state:PortTargetState.not_listening` | 原生测量未发现可用绑定。 |
| `state:PortTargetState.unavailable` | 尚无法确定当前可用性。 |

### PlanStepStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:PlanStepStatus.pending` | 尚未开始。 |
| `state:PlanStepStatus.in_progress` | 正在进行。 |
| `state:PlanStepStatus.completed` | 已完成。 |

### ApprovalState

| 参考标识 | 含义 |
| --- | --- |
| `state:ApprovalState.pending` | 等待决定。 |
| `state:ApprovalState.approved` | 已批准。 |
| `state:ApprovalState.answered` | 有人已回答程序的问题或表单。 |
| `state:ApprovalState.denied` | 已拒绝。 |
| `state:ApprovalState.cancelled` | 申请人已撤回。 |
| `state:ApprovalState.expired` | 已过期。 |

### ThreadContinuationStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:ThreadContinuationStatus.preparing` | 正在准备确切的原生来源。 |
| `state:ThreadContinuationStatus.ready` | 已验证原生结果，可以选择此继续点。 |
| `state:ThreadContinuationStatus.failed` | 准备失败，原执行状态和历史保持完整。 |

### InputState

| 参考标识 | 含义 |
| --- | --- |
| `state:InputState.pending` | 排队等待发送。 |
| `state:InputState.delivering` | 正在交给代理。 |
| `state:InputState.delivered` | 已交给代理。 |
| `state:InputState.cancelled` | 交付前已取消。 |
| `state:InputState.failed` | 未能交付。 |

### AgentMessagePhase

| 参考标识 | 含义 |
| --- | --- |
| `state:AgentMessagePhase.commentary` | 工作过程中的说明。 |
| `state:AgentMessagePhase.final` | 最终回答。 |

### SubagentState

| 参考标识 | 含义 |
| --- | --- |
| `state:SubagentState.running` | 正在工作。 |
| `state:SubagentState.completed` | 已完成。 |
| `state:SubagentState.failed` | 执行失败。 |
| `state:SubagentState.interrupted` | 已中断。 |

### ItemStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:ItemStatus.in_progress` | 仍在产生内容。 |
| `state:ItemStatus.completed` | 已完成。 |
| `state:ItemStatus.failed` | 执行失败。 |
| `state:ItemStatus.cancelled` | 已取消。 |
| `state:ItemStatus.pending` | 审批等待决定。 |
| `state:ItemStatus.approved` | 审批已批准。 |
| `state:ItemStatus.answered` | 程序的问题或表单已得到人工回答。 |
| `state:ItemStatus.denied` | 审批已拒绝。 |
| `state:ItemStatus.expired` | 审批已过期。 |

### TurnState

| 参考标识 | 含义 |
| --- | --- |
| `state:TurnState.starting` | 正在准备。 |
| `state:TurnState.running` | 正在运行。 |
| `state:TurnState.completed` | 已完成。 |
| `state:TurnState.interrupted` | 已中断。 |
| `state:TurnState.failed` | 执行失败。 |

### TunnelGatewayState

| 参考标识 | 含义 |
| --- | --- |
| `state:TunnelGatewayState.local` | 仅本地连接。 |
| `state:TunnelGatewayState.pending` | 等待网关连接。 |
| `state:TunnelGatewayState.attached` | 已连接到网关。 |
| `state:TunnelGatewayState.unavailable` | 网关当前不可用。 |
| `state:TunnelGatewayState.unknown` | 尚无可用的网关状态报告。 |

### AdoptHistoryState

| 参考标识 | 含义 |
| --- | --- |
| `state:AdoptHistoryState.read` | 已读取早期历史，或历史为空。 |
| `state:AdoptHistoryState.unreadable` | 最近一次读取失败，可以重试。 |
| `state:AdoptHistoryState.unsupported` | 适配器不支持读取会话，重试不会有帮助。 |
| `state:AdoptHistoryState.offline` | 会话所属机器已离线。 |
| `state:AdoptHistoryState.unavailable` | 提供方、环境或机器已无法访问。 |

### SideState

| 参考标识 | 含义 |
| --- | --- |
| `state:SideState.open` | 旁支仍打开。 |
| `state:SideState.closed` | 旁支已关闭。 |

### BackgroundState

| 参考标识 | 含义 |
| --- | --- |
| `state:BackgroundState.live` | 现有原生会话的新鲜状态报告。 |
| `state:BackgroundState.offline` | 机器或会话连接已断开。 |
| `state:BackgroundState.unavailable` | 无法取得原生状态。 |

### PhaseState

| 参考标识 | 含义 |
| --- | --- |
| `state:PhaseState.pending` | 尚未开始。 |
| `state:PhaseState.running` | 正在进行。 |
| `state:PhaseState.done` | 已完成。 |
| `state:PhaseState.skipped` | 已跳过。 |

### NativeCatalogState

| 参考标识 | 含义 |
| --- | --- |
| `state:NativeCatalogState.ready` | 会话已提供实际生效的映射。 |
| `state:NativeCatalogState.not_ready` | 会话尚不能列出目录。 |
| `state:NativeCatalogState.unsupported` | 此适配器不能保证原生派发。 |
| `state:NativeCatalogState.failed` | 实际读取失败。 |

### SnapshotState

| 参考标识 | 含义 |
| --- | --- |
| `state:SnapshotState.running` | 正在记录。 |
| `state:SnapshotState.complete` | 全部内容已记录。 |
| `state:SnapshotState.partial` | 部分文件未能记录。 |
| `state:SnapshotState.failed` | 未产生可用结果。 |
| `state:SnapshotState.unavailable` | 未能提供完整状态，保留具体原因。 |
| `state:SnapshotState.expired` | 保留状态已过期，元数据仍解释轮次采用的替代方式。 |

### InputScheduleState

| 参考标识 | 含义 |
| --- | --- |
| `state:InputScheduleState.active` | 后续定时执行已启用。 |
| `state:InputScheduleState.paused` | 后续定时执行已暂停。 |
| `state:InputScheduleState.finished` | 一次性执行已经结束。 |
| `state:InputScheduleState.deleted` | 后续定时工作已删除，已排队输入不变。 |

### InputScheduleRunState

| 参考标识 | 含义 |
| --- | --- |
| `state:InputScheduleRunState.queued` | 普通输入已排队。 |
| `state:InputScheduleRunState.blocked` | 普通输入被暂存。 |
| `state:InputScheduleRunState.started` | 对应的普通轮次已开始。 |
| `state:InputScheduleRunState.failed` | 未能开始或已取消。 |
| `state:InputScheduleRunState.skipped` | 当地时间不存在，或前次执行仍待处理，因此跳过。 |

### FetchStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:FetchStatus.ready` | 确切版本已在文件存储中。 |
| `state:FetchStatus.fetching` | 已请求环境上传。 |
| `state:FetchStatus.unavailable` | 无法取得此内容。 |

### McpState

| 参考标识 | 含义 |
| --- | --- |
| `state:McpState.starting` | 正在启动进程与协议连接。 |
| `state:McpState.running` | 正在运行。 |
| `state:McpState.stopped` | 已停止。 |
| `state:McpState.failed` | 执行失败。 |
| `state:McpState.unavailable` | 在此处不可用。 |

### TerminalListState

| 参考标识 | 含义 |
| --- | --- |
| `state:TerminalListState.open` | 仍在运行的终端。 |
| `state:TerminalListState.closed` | 已结束的终端。 |

### TerminalState

| 参考标识 | 含义 |
| --- | --- |
| `state:TerminalState.opening` | 正在启动。 |
| `state:TerminalState.open` | 正在运行。 |
| `state:TerminalState.closed` | 已结束。 |

### BrowserState

| 参考标识 | 含义 |
| --- | --- |
| `state:BrowserState.opening` | 进程启动期间保留其位置。 |
| `state:BrowserState.open` | 浏览器控制地址已响应。 |
| `state:BrowserState.closed` | 浏览器已结束。 |

### ApprovalListState

| 参考标识 | 含义 |
| --- | --- |
| `state:ApprovalListState.pending` | 等待决定。 |
| `state:ApprovalListState.decided` | 已有决定。 |

### ModelInputState

| 参考标识 | 含义 |
| --- | --- |
| `state:ModelInputState.prepared` | 已记录，正在发送。 |
| `state:ModelInputState.sent` | 程序已接受。 |
| `state:ModelInputState.failed` | 程序拒绝或发送失败。 |
| `state:ModelInputState.uncertain` | 未收到答复，无法确认是否送达。 |

### AdminUserStatus

| 参考标识 | 含义 |
| --- | --- |
| `state:AdminUserStatus.active` | 账号处于可用状态。 |
| `state:AdminUserStatus.suspended` | 此人的所有会话和令牌立即停止接受；其拥有的内容仍归其所有。 |

### PluginState

| 参考标识 | 含义 |
| --- | --- |
| `state:PluginState.running` | 正在运行。 |
| `state:PluginState.restarting` | 正在重新启动。 |
| `state:PluginState.failed` | 执行失败。 |

### SemanticState

| 参考标识 | 含义 |
| --- | --- |
| `state:SemanticState.off` | 语义搜索已关闭。 |
| `state:SemanticState.not_named` | 已开启，但尚未指定向量提供方与模型。 |
| `state:SemanticState.unavailable` | 数据库无法保存向量。 |
| `state:SemanticState.idle` | 所有片段已有向量。 |
| `state:SemanticState.indexing` | 正在切分内容或生成向量。 |
| `state:SemanticState.paused` | 索引等待原因所指的磁盘空间、提供方、机器、限额或权限。 |
| `state:SemanticState.waiting` | 新模型等待管理员选择，因为磁盘配额无法同时容纳两代索引。 |

### ProgramChannelState

| 参考标识 | 含义 |
| --- | --- |
| `state:ProgramChannelState.none` | 未声明自动版本来源。 |
| `state:ProgramChannelState.unread` | 已有来源，但尚未成功读取。 |
| `state:ProgramChannelState.ready` | 已有成功读取的记录。 |
| `state:ProgramChannelState.failed` | 最近一次尝试失败。 |
| `state:ProgramChannelState.disabled` | 系统读取间隔已关闭定时读取。 |

### AgentShareSnapshotState

| 参考标识 | 含义 |
| --- | --- |
| `state:AgentShareSnapshotState.pending` | 等待线程下一个轮次。 |
| `state:AgentShareSnapshotState.delivered` | 已写入线程目录。 |
| `state:AgentShareSnapshotState.revoked` | 已撤回。 |

### ExportState

| 参考标识 | 含义 |
| --- | --- |
| `state:ExportState.pending` | 任务正在写入导出包。 |
| `state:ExportState.ready` | 可以下载。 |
| `state:ExportState.failed` | 任务失败或被取消，没有保留内容。 |
| `state:ExportState.expired` | 本人删除或保留期结束，记录仍保留一个月。 |

### ImportState

| 参考标识 | 含义 |
| --- | --- |
| `state:ImportState.pending` | 任务正在导入。 |
| `state:ImportState.done` | 请求的内容已导入，报告说明每个对象的结果。 |
| `state:ImportState.failed` | 任务失败，已创建内容保留，报告说明进度。 |
| `state:ImportState.cancelled` | 任务已取消，已创建内容保留，报告说明进度。 |
| `state:ImportState.undoing` | 任务正在撤销已创建内容。 |
| `state:ImportState.undone` | 已撤销所创建的内容。 |

### BackgroundEndState

| 参考标识 | 含义 |
| --- | --- |
| `state:BackgroundEndState.ended` | 原生控制已确认结束。 |
| `state:BackgroundEndState.already_ended` | 此确切命令之前已结束。 |
| `state:BackgroundEndState.still_running` | 已知命令仍在运行。 |
| `state:BackgroundEndState.unknown` | 无法证明已结束。 |

### SummaryState

| 参考标识 | 含义 |
| --- | --- |
| `state:SummaryState.empty` | 尚未生成摘要段落。 |
| `state:SummaryState.fresh` | 段落与已记录的当前来源一致。 |
| `state:SummaryState.stale` | 来源已变化，尚无排队的自动更新。 |
| `state:SummaryState.scheduled` | 更新正在等待最短间隔或恢复延迟结束，原有文字仍可阅读。 |
| `state:SummaryState.updating` | 确实存在排队或运行中的更新。 |
| `state:SummaryState.failed` | 最近一次尝试失败，仍可阅读原有文字。 |
| `state:SummaryState.no_summarizer` | 尚未选择摘要器，或所选摘要器不可用。 |

### AskState

| 参考标识 | 含义 |
| --- | --- |
| `state:AskState.ready` | 可以接收下一条消息。 |
| `state:AskState.running` | 已有一个轮次在运行；第二条输入不能同时启动另一轮。 |
| `state:AskState.closed` | 所有者已无法进入此会话的根组织。 |
| `state:AskState.kept` | 已从此会话的历史创建永久线程。 |

### UploadInventoryState

| 参考标识 | 含义 |
| --- | --- |
| `state:UploadInventoryState.complete` | 已验证的不可变文件内容已入库。 |
| `state:UploadInventoryState.partial` | 上传尚未完成；不能据此认定发送方此刻仍在活动。 |
| `state:UploadInventoryState.expired` | 上传会话的完成期限已过。 |

### 文件准备

| 参考标识 | 含义 |
| --- | --- |
| `state:EnvironmentIndexReason.unreadable` | 部分元数据或排除规则无法读取；安全的实时读取保留修正路径。 |
| `state:CommandKeyPhase.queued` | 已接受并等待执行位置；尚未占据执行通道。 |
| `state:CommandKeyPhase.running` | 命令当前正在使用执行通道。 |
| `state:CommandKeyPhase.released` | 捕获已释放执行通道；上传可以与后续启动同时继续。 |
