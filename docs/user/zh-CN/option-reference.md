---
group: providers
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [选项, 字段, 搜索, 参考, options, fields]
heading_id.条目: entries
summary: 提供方接口接受的每个请求选项，以及它改变什么。
---

# 选项字段参考

只有接口或程序声明支持的选项才会提供，提供方限制和权限仍然适用。自定义结构中的字段沿用该结构的含义；本参考涵盖随产品提供的声明。设置中的字段见[设置参考](settings-reference.md)。作出改变前先审阅计划，选择运行方式前阅读[程序指南](register-your-program.md#choose-an-arrangement)。

## 条目

| 参考标识 | 含义 |
| --- | --- |
| `option:AdapterEvent.native_opening_id` | 输出进入队列前，从获准的 SessionSpec 取得产生报告的物理打开标识；空值仅用于未绑定的历史记录，不能控制活动会话。 |
| `option:ModelRequestReport.native_opening_id` | 实际观察到该请求的物理打开标识；送达和重放均保留此标识，实测用量仍属于原来的逻辑轮次。 |
| `option:Ev.native_opening_id` | 随持久会话事件保存的原始产生方；较早的报告保留其事实，但不能控制后继会话。 |
| `option:EvbEntry.native_opening_id` | 批次中的每条记录分别保留产生方；合批、合并增量及重启都不能用当前会话重新标记它。 |
| `option:SessionOpenMcp.server_id` | 会话中提供的每个 MCP 服务器都携带其注册 UUID 和当前名称；仅有名称会被拒绝。 |
| `option:Cmd.native_members` | 会话配置被接受时冻结的成员名称和 nonce。MCP 和模型变更在原生生效前只比较各自的成员；其他独立成员变更不会阻止它们。 |
| `option:Cmd.native_revision` | 与原始会话成员一同捕获的事实修订。重试会保留该值；送达时不会用较新的配置重标旧命令。 |
| `option:ToolSessionControl.refreshing` | 主连接正在同步当前作用范围事实。本地访问会等待；独立保留的远程传输继续使用各自中继的实时准入。 |
| `option:Frame.tool_session_update` | 已提交的会话和成员撤回只送达原来源机器的认证主连接；通知不会安装原生路由。 |
| `option:Frame.tool_session_control` | API 的当前控制状态暂停本地工具交换，同时保留原生会话。 |
| `option:ToolSessionUpdate.machine_id` | 该物理会话原来的来源机器；其他机器的主连接或仅隧道连接不能接收它的原生控制。 |
| `option:ToolSessionUpdate.thread_id` | 本次已提交物理作用范围变更所属的确切线程。 |
| `option:ToolSessionUpdate.segment_id` | 原来的逻辑分段；更新不会改选较新的分段。 |
| `option:ToolSessionUpdate.opening_id` | 访问发生变更的物理会话，即使其分段已移除也保留该标识。 |
| `option:ToolSessionUpdate.revision` | 单调递增的成员事实；旧通知不能恢复访问或结束后继会话。 |
| `option:ToolSessionUpdate.live` | false 撤回整个物理会话；true 仅保留 nonce 仍相同的现有成员。 |
| `option:ToolSessionUpdate.members` | 剩余成员的事实名称和 nonce；此列表不授予新路由或权限。 |
| `option:ToolSessionMember.member` | 物理会话内的 exec、llm 或具有稳定标识的 MCP 成员。 |
| `option:ToolSessionMember.nonce` | 未变更成员的身份；移除后再次添加会产生不同的 nonce。 |
| `option:ToolSessionControl.available` | 实时 API 当前能否验证此安装中的会话工具。 |
| `option:ToolSessionControl.admission_timeout_ms` | API 准入查询的正数等待时限，由 tunnels.control_timeout_ms 设置。 |
| `option:ToolSessionQuery.n` | 一次待答的准入查询；稍后返回的答案不能用于其他通道。 |
| `option:ToolSessionQuery.claims` | Open 或 Resume 所提交原始已验票据中的确切声明。 |
| `option:ToolSessionQuery.ticket` | API 会再次验签的原始票据，不进入诊断输出。 |
| `option:ToolSessionReply.n` | 此答案一次性完成的那次待答查询。 |
| `option:ToolSessionReply.allowed` | 当前原生物理会话及成员是否准许这次确切请求。 |
| `option:ToolSessionReply.revision` | 此次准入实际读取的已提交原生修订。 |
| `option:Cmd.native_target` | 原生操作获准时记录的原始物理目标。若当时没有会话，该操作之后也不会查找并影响后来的会话；此字段不授予任何权限。 |
| `option:NativeTarget.session` | 当前机器链路协议携带的会话目标快照；旧接收端不能忽略此字段后继续执行。 |
| `option:SessionTarget.thread_id` | 获准原生操作的确切线程，须与原命令正文匹配。 |
| `option:SessionTarget.segment_id` | 操作获准时记录的逻辑片段；当时没有片段则为空。 |
| `option:SessionTarget.opening_id` | 不可变的物理打开标识；空值表示目标当时已不存在，不代表可以关闭后来的会话。 |
| `option:SessionTarget.opening_epoch` | 同一线程各片段之间的获准打开顺序，不依赖时间戳或进程号。原生会话退役后，即使槽位为空或程序重启，也会保留此顺序。 |
| `option:ToolSessionScope.opening_epoch` | 签名绑定的获准打开顺序；临时后继片段被移除后，旧片段也不能重新取得访问权。 |
| `option:ToolSessionUpdate.opening_epoch` | 给逻辑片段之间的物理打开变化排序；延迟送达的旧更新不能关闭后继会话。 |
| `option:McpServerCreateBody.name` | 所添加服务器的名称；exec 与 orch 属于内置工具服务器，不能用作新增定义的名称。 |
| `option:EnvironmentMcpServerCreateBody.name` | 环境自有服务器的名称，须与内置的 exec、orch 服务区分。 |
| `option:SessionSpec.opening_id` | 传给适配器的获准物理打开命令标识，与逻辑设置和私有回环密钥分别保存。 |
| `option:SessionOpened.opening_id` | 产生此报告的物理打开标识；延迟送达的旧报告不能填充或重新打开后继会话。 |
| `option:SessionLost.expected` | 正常的空闲会话退役会安静地关闭旧物理打开状态，并保留对话。已有运行中的轮次仍会收到实际丢失及恢复结果。 |
| `option:SessionLost.opening_id` | 实际结束的物理打开标识，也会在连接器重启记录中保留；只有匹配的有效准入才会关闭并进入恢复。 |
| `option:TicketClaims.tool_session` | 会话工具票据绑定不可变的物理打开标识、片段、线程、成员随机标识及准入修订号；每次打开或恢复通道还须向 API 确认当前准入。 |
| `option:ToolSessionScope.opening_id` | 获准物理打开操作的命令标识；重新打开取得新标识，仅刷新传输则保留现有标识。 |
| `option:ToolSessionScope.member` | 原生 exec 成员（也包括内置执行器的 MCP 传输）、llm 成员，或 mcp 与服务器稳定 UUID 的组合；服务器名称不能代替其身份。 |
| `option:ToolSessionScope.nonce` | 成员未变时保留随机标识；撤回后重新加入须取得新标识，其他成员仍可使用。 |
| `option:ToolSessionScope.revision` | 会话及成员事实的持久顺序；较旧的答复不能恢复已关闭或撤回的通道。 |
| `option:ToolSessionScope.thread_id` | 该物理会话的确切线程，须与签名票据和当前片段同时匹配。 |
| `option:ToolSessionScope.segment_id` | 承载物理打开操作的现有逻辑片段；单凭片段标识不能区分重新打开。 |
| `option:CleanupPlan.effects` | 来自统一使用事实服务的当前影响：相关设置立即关闭此挂载，环境已有的挂载在下次启动时卸下。仅显示有权查看的名称，其余保留为数量。 |
| `option:CleanupStart.plan_id` | 此人实际查看过的当前计划。挂载位置被替换后，即使时间戳相同，旧计划也会失效。 |
| `option:CleanupStart.choices` | 已展示数据项的去向。删除挂载允许全部保留，也允许删除没有数据的挂载。 |
| `option:CleanupStart.confirmation_name` | 选中销毁由 Orch 管理的数据时须输入准确的挂载或计划名称；保留数据无需输入名称。 |
| `option:CleanupStart.notify` | 对整次操作明确选择是否通知，再按接收人的偏好筛选，每人最多一条分组通知。 |
| `option:CleanupItem.record.blocker` | 数据去向不可用的真实原因；not_permitted 表示缺少数据管理权限。仍可删除挂载定义并保留数据。 |
| `option:NotificationOperation.kind` | mount_delete 表示一次已确认的挂载删除，其统一清理任务结束后更新原有的分组通知。 |
| `option:ServerUpdateStart.release` | 系统管理员选择的当前已验证语义版本，不接受路径、命令、网址或参数列表。 |
| `option:ServerUpdateView.installed_version` | 当前回答读取请求的 API 版本，更新或回滚后也反映实际运行版本。 |
| `option:ServerUpdateView.offered_version` | 最近一次已验证的可用版本，尚无可用信息时为 null。 |
| `option:ServerUpdateView.update_available` | 可用版本是否新于当前回答的 API；旧检查缓存不会让已更新服务器仍显示落后。 |
| `option:ServerUpdateView.availability` | 管理页更新操作所需的随服务端安装节点、连接及本机 Docker 条件。 |
| `option:ServerUpdateView.command` | 原生节点路径不可用时仍可使用的同一个 update.sh 运维命令。 |
| `option:ServerUpdateView.job` | 最近一次持久化服务器更新作业，API 重启或页面重载后保留；再次点击加入正在进行的作业。 |
| `option:ServerUpdateView.observation` | 主机更新器记录的最新安全阶段及结果，不含原生日志或凭据。 |
| `option:ServerUpdateObservation.stage` | 发布更新器记录的阶段；updated 和 rolled_back 表示已经确认的结果。 |
| `option:ServerUpdateObservation.reason` | 固定且安全的失败原因，与阶段及未过滤的原生输出分开。 |
| `option:ServerUpdateObservation.from_version` | 现有更新日志记录的更新前版本，尚未记录时为 null。 |
| `option:ServerUpdateObservation.to_version` | 此作业获准的精确版本，不因重新连接或再次检查版本而改变。 |
| `option:NodeServerUpdate.timeout_secs` | 从 server.update_prepare_timeout_secs 捕获的正数原生准备时限，覆盖命令类型默认值；不会限制独立运行的发布事务总时长。 |
| `option:NodeServerUpdate.command_id` | 此作业已准备辅助进程对应的精确成功原生命令；缺少同一已存作业、计算机、版本及私有来源的观测会被忽略。 |
| `option:NodeServerUpdate.job_id` | 已认证随服务端安装节点上的同一持久更新作业，其他节点不能提供它的结果。 |
| `option:NodeServerUpdate.release` | 固定安装更新器使用的精确已验证版本；请求字段不接受本机路径或主机命令。 |
| `option:NodeServerUpdate.observation` | 随服务端安装节点仅通过持久节点事件流报告自身操作的安全更新记录。 |
| `option:MachineFacts.server_update` | 本机报告的固定更新能力，仅由具备本地 Docker 访问权的随服务端安装物理节点提供。 |
| `option:AuditAction.server.update_requested` | 记录请求服务器版本更新的人、持久作业及精确的随服务端安装主机和节点。 |
| `option:update.sh.ORCH_UPDATE_JOB` | 原生辅助进程附带持久作业 UUID，供记录阶段及恢复使用。普通 shell 更新省略它；它不含凭据，也不能选择主机命令。 |
| `error:server_update_rolled_back` | 所选版本未通过就绪检查，现有更新器已确认之前版本重新运行。 |
| `error:server_update_failed` | 现有更新器未能完成；保留的观测会说明是否需要执行原有恢复命令。 |
| `state:ServerUpdateAvailability.ready` | 已连接的随服务端安装节点提供固定本机更新器。 |
| `state:ServerUpdateAvailability.no_bundled_node` | 没有活动节点被标记为随服务端安装；可使用显示的 shell 命令。 |
| `state:ServerUpdateAvailability.ambiguous_bundled_nodes` | 有多个活动节点声称随服务端安装，因此不会猜测要更新哪个主机。 |
| `state:ServerUpdateAvailability.node_offline` | 随服务端安装节点须重新连接，才能接收更新作业。 |
| `state:ServerUpdateAvailability.docker_unavailable` | 随服务端安装节点缺少此安装的本地 Docker 更新路径；运维命令仍然可用。 |
| `state:ServerUpdateStage.queued` | 作业已获准，尚未开始获取发布。 |
| `state:ServerUpdateStage.fetching` | 现有更新器正在获取清单和文件。 |
| `state:ServerUpdateStage.verifying` | 获取的发布已进入原生验证步骤。 |
| `state:ServerUpdateStage.applying` | 正在准备已验证的程序版本目录。 |
| `state:ServerUpdateStage.stopping` | 应用服务正在停止，以执行更新事务。 |
| `state:ServerUpdateStage.dumping` | 现有更新器正在制作回滚数据库快照。 |
| `state:ServerUpdateStage.activating` | 现有更新器正在切换新程序版本目录。 |
| `state:ServerUpdateStage.starting` | 应用服务正在以所选版本重新启动。 |
| `state:ServerUpdateStage.checking` | 现有更新器正在探测服务及浏览器入口是否就绪。 |
| `state:ServerUpdateStage.rolling_back` | 现有更新器正在恢复之前的程序及数据库。 |
| `state:ServerUpdateStage.recovering` | 同一操作正在根据已有日志从中断中恢复。 |
| `state:ServerUpdateStage.updated` | 所选版本已运行并通过就绪检查。 |
| `state:ServerUpdateStage.rolled_back` | 尝试更新后，已确认之前版本重新运行。 |
| `state:ServerUpdateStage.failed` | 更新器未能确认新版本或已恢复版本；保留恢复路径。 |
| `state:ServerUpdateFailure.health_check_failed` | 所选版本未通过服务或浏览器入口就绪检查。 |
| `state:ServerUpdateFailure.update_failed` | 更新器在确认所选版本或回滚前结束。 |
| `state:ServerUpdateFailure.recovery_required` | 原有日志仍然存在，可使用同一个 update.sh recover 恢复路径。 |
| `option:PublicAddressUpdateBody.expected_revision` | 准备此草稿所依据的生效地址版本。若期间发生其他发布或本地重置，过期草稿会被拒绝，且不会返回其他系统投影。 |
| `option:PublicAddressUpdateBody.public_url` | 完整的候选主来源地址，须通过证书与安装验证后才会发布。 |
| `option:PublicAddressUpdateBody.alternate_urls` | 完整的候选其他来源地址集合，不可省略；空列表会与主地址一并原子清除其他地址。 |
| `option:PublicAddressSnapshot.using_example` | 生效主地址仍是随产品提供的示例地址，并非此安装实际使用的浏览器来源地址。 |
| `option:PublicAddressUpdated.projection` | 已准备地址发布的固有回执，不包含之前的文件初始值或其他系统配置。 |
| `option:PublicAddressUpdated.revision` | 此次发布提交的地址版本。 |
| `option:PublicAddressUpdated.public_url` | 由本次调用者草稿确定并获准的规范主来源地址。 |
| `option:PublicAddressUpdated.alternate_urls` | 获准的其他来源地址，与主地址一并规范化。 |
| `option:PublicAddressUpdated.using_example` | 获准的主地址是否仍为随产品提供的示例。 |
| `option:Meta.public_address_revision` | 与 public_url 及 alternate_urls 一并原子读取的已提交版本。版本变化会刷新读取结果，并显示一行浏览器提示，不会自动重载其他浏览器。 |
| `option:orchctl.public-address.reset` | 通过本地数据库恢复经校验的文件主地址及其他地址初始值，清除已存覆盖值并递增版本。恢复已获准的初始值或当前地址集合时不依赖 HTTP 主机名；全新文件来源地址须先完成相同的证书及来源地址准备，再提交。不会输出凭据。 |
| `option:orchctl.public-address.get` | 仅输出已提交的生效主地址，供本地脚本使用。系统存储尚未初始化时才使用文件初始值；之后的文件修改不会替换生效地址。 |
| `option:orchctl.doctor.only` | 可选择 public-address 查看已存主地址、其他地址、版本及初始值或覆盖值状态。未明确指定 --url 时，选定的浏览器检查使用已提交地址；默认本地检查会探测页面和 API。 |
| `option:headers.X-Orch-Frontend-Generation` | 随产品提供的前端进程已载入的证书版本。发布地址前会检查此标记、实际提供的 TLS 证书和安装证明。 |
| `option:PublicAddressProofQuery.challenge` | 单次浏览器来源地址检查使用的全新 UUID 挑战值，不是凭据。 |
| `option:PublicAddressProof.challenge` | 提供给此来源地址的同一挑战值；其他检查的证明无效。 |
| `option:PublicAddressProof.proof` | 使用此安装现有私有证书颁发机构生成的专用密钥证明；不会登录任何人，也不披露密钥或凭据。 |
| `option:PublicAddresses.public_url` | 主要 HTTP(S) 浏览器来源地址，与其他地址一并校验；新链接、命令和投递均读取系统存储中同一已提交版本。 |
| `option:PublicAddresses.alternate_urls` | 此安装明确声明的其他浏览器来源地址。发布会原子替换整个地址集合；文件修改不会覆盖已存储的初始值。 |
| `option:PublicAddressSnapshot.public_url` | 此版本正在生效的主要地址。已开始的登录、已签发的凭据和已导出的包保留获准时的地址。 |
| `option:PublicAddressSnapshot.alternate_urls` | 与主要地址一并提交的其他来源地址。 |
| `option:PublicAddressSnapshot.revision` | 发布已准备好的候选地址前核对的递增版本号；过期草稿不会改变生效地址。 |
| `option:PublicAddressSnapshot.from_file_seed` | 当前生效的是已存储的文件初始值。配置文件仅用于创建首个快照或明确执行的本地重置。 |
| `option:MachineGatewaySetBody.selection` | 选择自动或当前公布的网关，不重新注册或更换计算机身份。 |
| `option:MachineGatewaySetBody.gateway_id` | 仅在选择指定网关时必填；自动模式省略或清除此值。安装目录之外的网关会被拒绝。 |
| `option:MachineGatewaySetReceipt.projection` | 原生选择已接受的回执，不含计算机现有内容。 |
| `option:MachineGatewaySetReceipt.machine_id` | 请求指定的同一台已注册计算机。 |
| `option:MachineGatewaySetReceipt.selection` | 已接受的主网关选择；实际连接状态仍由网关状态提供。 |
| `option:MachineGatewaySetReceipt.gateway_id` | 所选已公布网关的标识，自动模式为 null。 |
| `option:MachineGatewaySetReceipt.changed` | 此次原生写入是否更改了保存的选择；重新连接前会持久记录回执。 |
| `option:MachineGatewaySet.choice` | 原生主网关选择，按计算机所属安装公布的网关验证。 |
| `option:MachineGatewaySetResult.choice` | 实际保存的原生选择。 |
| `option:MachineGatewaySetResult.changed` | 保存的配置是否发生变化，不表示新连接已经上线。 |
| `option:AuditAction.machine.gateway_changed` | 记录更改主网关的人、计算机、保存的选择和原生命令。 |
| `option:Host.bundled_with_server` | 仅由受保护的服务端引导令牌记录的不可变安装来源，不根据主机名或所选归属推断。 |
| `option:PagePreview.server_host_use` | 向随服务端安装的主机授予使用、管理或所有者权限前的当前提示和精确确认凭据；没有新增授权时省略。 |
| `option:ServerHostUseDisclosure.hosts` | 本次页面或默认权限变更会新增或更改授权的受保护服务端主机数量，不公开主机标识或名称。 |
| `option:ServerHostUseDisclosure.confirmation` | 通过 orch-server-host-use-confirmed 请求头确认此次权限变更及当前原生授权集合；变更意图改变时需重新确认，此凭据本身不授予访问权限。 |
| `option:ProviderPauseQuery.note` | 显示在受影响会话中的提供方停用简短备注。 |
| `option:ProviderPauseQuery.notify` | 停用提供方时，明确选择向每位受影响者发送一条合并通知；省略时不通知。对话框按“操作通知”设置初始选项，确认者可修改。 |
| `option:ResourceDeleteQuery.notify` | 删除提供方或 MCP 定义时，明确选择向每位受影响者发送一条合并通知；省略时不通知。提供方子任务进度更新同一条收件箱通知。MCP 通知记录定义移除，机器上的清理仍通过持久命令继续。 |
| `option:ResourceImpact.notification` | 提供方整体停用、提供方删除或 MCP 定义删除操作当前的通知对象和复选框初始值；其他影响类型和单个模型不提供此字段。 |
| `option:ResourceOperationNotification.audience` | 可显示的受影响人员，以及关联关系不可见的人员数量；不包含操作确认者，每人只计一次。 |
| `option:ResourceOperationNotification.default_on` | 根据此次影响继承的“操作通知”初始值；确认者可在执行前修改选项。 |
| `option:ask.models.*.model` | 已启用 API 模型的路由名称，与 provider_id 一起标识一个允许的问一问模型。子级列表可以缩小可选范围，并保留父级顺序。 |
| `option:ask.models.*.provider_id` | 此项允许的问一问模型所使用的 API 提供方标识。提问者和当前付费人仍须有权在对话所属组织中使用该提供方及模型。 |
| `option:AccessOptions.models` | 此使用权限涵盖的模型。 |
| `option:AccessOptions.services` | 此使用授权允许的已启用提供方服务，可按名称、系列或分组指定。省略时允许所有已启用服务；它与生成模型列表相互独立。 |
| `option:AccessOptions.efforts` | 此使用权限涵盖的思考强度。 |
| `option:AccessOptions.where` | 被拒绝的使用所针对的环境，或将要创建的环境所在的主机或机器；它说明提供方的条目没有覆盖到哪里。 |
| `option:BindingOptions.viewer_default` | 新建线程的当前查看者默认值及允许公开的来源；重新绑定现有线程时可缺省。 |
| `option:BindingOptions.environment_readers` | 选定已有环境对预期线程查看者的开放情况；新环境在创建事务中检查。 |
| `option:BindingOptions.defaults` | 设置决定的默认值。 |
| `option:BindingOptions.providers` | 每个提供方及其模型和代理。 |
| `option:BindingOptions.environments` | 当前读者可管理的已有环境，当前话题的环境优先。 |
| `option:BindingOptions.environment_default` | 未另行选择时，新线程如何选择环境。 |
| `option:BindingOptions.default_environment` | 最近一层设置指定的默认环境（无论设在哪一层），以及当前读者无法使用它时的原因；没有设置指定默认环境、或重新绑定现有线程时缺省。 |
| `option:BindingOptions.source` | 已安全解析的仓库设置及仓库地址的来源位置。 |
| `option:BindingOptions.access` | 在这里创建的线程最初采用的权限页。 |
| `option:BindingOptions.agent_access` | 此创建上下文可选的代理权限；重新绑定不会创建新贡献。 |
| `option:BindingOptions.new_environment` | 现在为此话题创建环境时，谁能查看、谁能管理。 |
| `option:BindingOptions.can_create` | 当前读者是否可以创建新环境。 |
| `option:BindingOptions.hosts` | 可创建环境的主机、容量、可用性及原因，并标出调度器选择；无法创建时为空。 |
| `option:BindingOptions.machines` | 当前读者自己的已连接机器，通过其目录环境运行线程。 |
| `option:ChangeOptions.active_turn` | 如何处理正在运行的轮次。 |
| `option:ChangeOptions.wait_minutes` | 等待运行轮次的分钟数；超时后停止，不作改变。 |
| `option:ChangeOptions.verify_timeout_secs` | 健康检查的超时时间，单位为秒。 |
| `option:ChangeOptions.keep_previous` | 保留旧侧内容，不安排移除。 |
| `option:ChangeOptions.stop_processes` | 从其他线程仍在使用的环境切换出去时，结束本线程在那里的进程。 |
| `option:AppliedChangeOptions.active_turn` | 如何处理正在运行的轮次。 |
| `option:AppliedChangeOptions.wait_minutes` | 等待运行轮次的分钟数；超时后停止，不作改变。 |
| `option:AppliedChangeOptions.verify_timeout_secs` | 健康检查的超时时间，单位为秒。 |
| `option:AppliedChangeOptions.keep_previous` | 保留旧侧内容，不安排移除。 |
| `option:AppliedChangeOptions.stop_processes` | 从其他线程仍在使用的环境切换出去时，结束本线程在那里的进程。 |
| `option:ImportOptions.parts` | 导入哪些类别；省略时为全部三类。 |
| `option:ImportOptions.versions` | 是否导入包中的较早文档版本，默认开启。 |
| `option:ImportOptions.comments` | 是否导入文档讨论，默认开启。 |
| `option:ImportOptions.access` | 仅在导入者的权限和目标位置的锁允许时恢复共享，默认关闭；需要权限管理权。 |
| `option:ImportOptions.authors` | 位置管理员可保留原作者，否则归于导入者。 |
| `option:ImportOptions.same_source` | 处理先前从同一来源导入的对象，默认替换。 |
| `option:ImportOptions.same_name` | 处理目标位置中同名的其他对象，默认保留两者。 |
| `option:ParameterOptions.from_model` | 参数选项使用模型报告的哪一个列表。 |
| `option:protocols.openai-responses.web_search.search_context_size` | 请求的搜索上下文量：低、中或高。 |
| `option:protocols.openai-responses.web_search.return_token_budget` | 声明的返回词元预算：默认或不限。 |
| `option:protocols.openai-responses.web_search.filters` | 此次搜索的域名筛选；允许列表与阻止列表不能同时提供。 |
| `option:protocols.openai-responses.web_search.filters.allowed_domains` | 仅搜索列出的域名。 |
| `option:protocols.openai-responses.web_search.filters.blocked_domains` | 不搜索列出的域名。 |
| `option:protocols.openai-responses.web_search.user_location` | 为此次搜索提供的大致位置；这是提供方选项，不会从读者信息推断。 |
| `option:protocols.openai-responses.web_search.user_location.type` | 大致位置的类型。 |
| `option:protocols.openai-responses.web_search.user_location.city` | 大致搜索位置中的城市。 |
| `option:protocols.openai-responses.web_search.user_location.region` | 大致搜索位置中的地区。 |
| `option:protocols.openai-responses.web_search.user_location.country` | 大致搜索位置中的国家代码。 |
| `option:protocols.openai-responses.web_search.user_location.timezone` | 大致搜索位置中的时区。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.model` | 托管工具使用的图像模型，与对话模型分开选择。应选择端点支持且符合选项限制的模型。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.action` | 选择 auto、generate 或 edit。auto 由工具判断生成还是编辑；编辑仍需要请求中提供的图像引用。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.background` | 选择自动、不透明或透明背景。所选图像模型和输出格式必须支持该背景。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.output_compression` | 提供方对 JPEG 或 WebP 输出采用的压缩设置，范围为 0 到 100，仅在支持时生效；不会改变请求的图像数量。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.output_format` | 请求的图像编码：PNG、JPEG 或 WebP。这不能证明返回字节的实际格式；结果保留实际报告的格式。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.partial_images` | 请求 0 到 3 张流式预览。预览不算另一张已完成图像；提供方可能将预览工作计入令牌用量。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.quality` | 请求的绘制质量。只能使用所选图像模型及其选项限制支持的级别。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.size` | 请求的尺寸或自动尺寸，必须在所选图像模型支持的范围内；不会覆盖提供方限制。 |
| `option:protocols.anthropic-messages.web_search.max_uses` | 设置后，限制本次请求中搜索工具的最大使用次数。 |
| `option:protocols.anthropic-messages.web_search.allowed_domains` | 仅搜索列出的域名。 |
| `option:protocols.anthropic-messages.web_search.blocked_domains` | 不搜索列出的域名。 |
| `option:protocols.anthropic-messages.web_search.user_location` | 为此次搜索提供的大致位置；这是提供方选项，不会从读者信息推断。 |
| `option:protocols.anthropic-messages.web_search.user_location.type` | 大致位置的类型。 |
| `option:protocols.anthropic-messages.web_search.user_location.city` | 大致搜索位置中的城市。 |
| `option:protocols.anthropic-messages.web_search.user_location.region` | 大致搜索位置中的地区。 |
| `option:protocols.anthropic-messages.web_search.user_location.country` | 大致搜索位置中的国家代码。 |
| `option:protocols.anthropic-messages.web_search.user_location.timezone` | 大致搜索位置中的时区。 |
| `option:SummaryTargetOptions.automatic` | 允许此目标自动刷新。默认关闭；手动刷新是独立操作。 |
| `option:SummaryTargetOptions.on` | 可以触发自动刷新的事件。省略时采用目标的常规事件；空列表关闭事件触发。 |
| `option:SummaryTargetOptions.min_interval_secs` | 自动刷新的最短间隔，单位为秒；零表示不额外限制间隔。 |
| `option:SummaryTargetOptions.context_tokens` | 正数的输入词元预算；省略时使用摘要器的默认值 32,000，并受模型可用窗口减去输出的约束。 |
| `option:SummaryTargetOptions.output_chars` | 输出字符预算，必须为正数；不会覆盖提供方限制或用量限额。 |
| `option:SummaryOptionsBody.options` | 此目标自己的选项；空值移除覆盖并恢复继承。 |
| `option:SummaryOptionsBody.expected_revision` | 调用者已读取的选项修订号；选项变化后需要重新审阅。 |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.max_uses` | 设置后，限制本次请求中读取网页工具的最大使用次数。 |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.allowed_domains` | 只读取列出域名的网页；不能与排除的域名同时设置。 |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.blocked_domains` | 从不读取列出域名的网页；不能与允许的域名同时设置。 |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.max_content_tokens` | 设置后，一次读取中提供方交给模型的网页内容最多多少个令牌。 |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.citations` | 读取的网页是否带上回答可以引用的出处。 |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.citations.enabled` | 开启或关闭读取网页的出处。 |
| `option:ModelGroupOptions.request_options` | 按接口声明请求选项的形式和约束；`native` 表示程序参数。端点、分组和供给的约束仍然适用。 |
| `option:providers.presets.*.credential_hints` | 公开的密钥格式提示，用来建议此预设及凭据用途；不会验证密钥或启用服务。保存后首次正常读取会确认提供方信息。 |
| `option:providers.presets.*.credential_hints.*.prefix` | 密钥格式的公开前缀，直接按原文填写。匹配只会建议选项，完整密钥在保存前仍留在表单中。 |
| `option:providers.presets.*.credential_hints.*.role` | 建议的用途：模型访问，或数据源的管理凭据。客户端不认识的用途需要由使用者明确选择受支持的用途。 |
| `option:providers.presets.*.credential_hints.*.field` | 可选的已声明秘密字段名，用来将建议的管理凭据填入所选信息数据源。该字段必须由数据源提供，并与模型访问凭据分开。 |
