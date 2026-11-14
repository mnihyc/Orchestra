---
group: settings
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [reference, settings, options, states, errors, 参考, 设置, 选项, 状态, 错误]
heading_id.条目: entries
summary: 每个设置可以在哪里设定，以及它改变什么。
---

# 设置参考

帮助 → 设置为每个内置设置提供完整条目，列出默认值、示例、适用范围；从控件打开时还显示当前来源。本索引列出所有设置及其已声明的结构化字段；`*` 代表由填写者选择的名称。继承方式见[设置与用量](settings-and-usage.md)。

个人只读提问的使用方法见 [Ask](ask.md)；报告的生命周期状态见[状态参考](state-reference.md)。

## 条目

| 参考标识 | 含义 |
| --- | --- |
| `setting:calls.rules` | **调用恢复** — 模型调用失败时按顺序重试、继续、故障转移或等待的规则。 |
| `setting:calls.repeat_limit` | **重复次数** — 重复时间窗口内同类故障允许自动恢复的次数；零表示不自动恢复。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:calls.repeat_window` | **重复时间窗口** — 用于停止反复恢复的滚动时间窗口。 |
| `setting:calls.notify_on_stop` | **停止时通知** — 重复保护停止恢复时向线程所有者发送一条通知。 |
| `setting:calls.return` | **故障转移后** — 下一轮时在原选择恢复可用后切回，或继续使用候选项。 |
| `setting:agent.model` | **模型** — 这些会话使用的模型，请填写提供方给出的模型 ID。 |
| `setting:agent.effort` | **思考程度** — 在模型支持时，选择它在一次运行中投入的思考程度。 |
| `setting:agent.params` | **智能体参数** — 按参数名称设置智能体适配器声明的参数值。 |
| `setting:agent.features` | **代理功能** — 按功能选择要启用的可选代理功能。 |
| `setting:agent.instructions` | **常驻指令** — 每次会话都会提供给代理的常驻指令。 |
| `setting:agent.memory` | **智能体笔记** — 代理是否在会话之间保留笔记：使用任何可用方式、仅由代理程序自行保存，或不保留。 |
| `setting:agents.harness_versions` | **代理程序版本** — 选择新建或更新单元所用的代理程序版本，沿项目继承，环境可自行覆盖。智能体配置的固定版本优先；修改此项不会替换正在运行的单元。 |
| `option:agents.harness_versions.*.use` | 程序版本的选用策略。 |
| `option:agents.harness_versions.*.version` | 选定的确切程序版本。 |
| `setting:agents.harness_channel_hours` | **发布检查间隔** — 有位置请求较新构建时读取发布渠道的频率；0 表示不定时读取。 |
| `setting:agents.harness_interface_timeout_secs` | **接口检查超时** — 构建输出其接口信息最多可用的时间。 |
| `setting:agents.harness_case_timeout_secs` | **兼容性用例超时** — 每项原生兼容性用例使用脚本模型运行时最多可用的时间。 |
| `setting:agents.harness_conformance_timeout_secs` | **兼容性检查超时** — 在独立单元中运行整套原生兼容性检查最多可用的时间。 |
| `setting:agents.harness_trust_mirror` | **信任镜像校验和** — 允许发布镜像自行提供程序校验和。这些版本会标为镜像校验和，而不是厂商校验和。 |
| `setting:agents.harness_mirror` | **发布镜像** — 在此服务器无法访问厂商时，提供厂商发布文件的服务器。 |
| `setting:agents.prompt_cache_ttl` | **提示缓存时长** — 为代理程序的主对话指定缓存时长。不设置时使用程序自身的默认值；不支持此控制的程序会忽略它。 |
| `setting:agents.web_action_wait_secs` | **网页操作等待时间** — 关闭尚未报告结束的原生 Codex 网页操作行，并说明原因。默认 120 秒，沿所在位置的设置继承，没有固定上限。轮次继续运行，之后真正到达的结束事件仍可更新同一行。会话重新打开时生效。 |
| `setting:agents.native_result_wait_ms` | **原生结果等待时间** — 为声明了保留证据读取等待的程序提供系统默认值 2000 毫秒。程序配置可在引擎的中断等待窗口内选择其他值。读取超时保留操作已报告的结果，未证实用量保持未知，其他会话继续运行。程序重新启动时生效。 |
| `setting:agents.native_result_buffer_bytes` | **原生结果缓冲区** — 会话读取可选证据时，按顺序保留后续原生输入的系统默认值，默认为 8 MiB。程序配置可选择其他大小，包括零。缓冲区满时，只将可选读取结束为未验证；所有输入继续处理。程序重新启动时生效。 |
| `setting:agents.subagents_at_once` | **同时运行的子智能体** — 代理程序可同时运行的子代理数量。此处及上级位置中的最小值生效；不支持的控件会说明无法适用的原因。 |
| `setting:agents.subagent_depth` | **子智能体层数** — 子代理可启动的层数；1 表示不能再嵌套。此处及上级位置中的最小值生效；不支持的控件会说明无法适用的原因。 |
| `setting:agents.skills` | **技能** — 按技能指定此处线程的智能体可使用哪些技能：关闭、最新版本或固定版本；下级位置可以更改或关闭继承的技能。 |
| `setting:agents.personal_skills` | **个人技能** — 线程的智能体是否也能使用发起该线程的人自己的技能。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:thread.cwd` | **起始目录** — 代理在此线程目录中启动的位置，填写相对路径。 |
| `setting:thread.side_threads` | **旁支线程** — 是否可以从此处的线程发起旁支线程。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:thread.side_max` | **同时打开的旁支线程** — 同一个线程最多可以同时打开多少个旁支线程。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:thread.side_idle_hours` | **旁支线程空闲时间** — 打开的旁支线程多少小时没有新一轮工作后关闭。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:thread.inputs_without_use` | **无使用权的写入者** — 可以在此写入、但不能使用该线程的模型或其环境的人发给智能体的消息如何处理。 下级只能选择更严格的选项。 严格程度依次为：`allowed` → `held` → `refused`，最后一项最严格。 |
| `setting:thread.switch_history` | **切换时的历史** — 切换或分叉时，除非计划另有选择，默认带上多少对话。 |
| `setting:thread.switch_carry_share` | **延续的上下文** — 切换时可以用延续的对话占用新模型上下文窗口的比例。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:thread.native_fork_check_bytes` | **原生分支检查大小** — 对话最多占多大空间时，才会拿程序自己保存的对话记录来核对某一轮次处的分支，从而由程序自己完成分支。更长的对话改为带入放得下的最近轮次。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:thread.read_turn_bytes` | **单轮读取大小** — Orch 把某一轮读入对话摘要或目标检查时，该轮消息完整写出后最多可占的大小；更大的一轮只标明其位置，不读取其中任何内容。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:thread.plan_first` | **先规划** — 会话的第一条消息是否默认打开输入框的计划开关。审批模式保持不变。 |
| `setting:thread.plan_answered_by` | **谁来回应计划** — 谁可以对智能体提出的计划选择继续执行或继续规划。 下级只能选择更严格的选项。 严格程度依次为：`writers` → `managers`，最后一项最严格。 |
| `setting:thread.silent_after_secs` | **无进展提示** — 运行中的智能体在没有工具运行时，多少秒没有任何新内容后，状态行才会说明已经多久没有新进展。 |
| `setting:thread.goal.max_turns` | **目标轮数** — 目标在此最多可用的轮数；0 表示不限。目标可设定更低的自身限额，不能提高上级位置的上限。下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:thread.goal.max_minutes` | **目标分钟数** — 目标在此最多可处于进行状态的分钟数；0 表示不限。每轮之间检查，不会打断正在进行的一轮。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:thread.goal.max_credits` | **目标额度** — 目标在此最多可用的额度，写作小数；0 表示不限。用量上限照常有效。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:approval.mode` | **审批方式** — 代理在执行操作前需要请求同意的条件。 |
| `setting:approval.allowed_modes` | **可选审批模式** — 此处允许选择哪些审批模式。 |
| `setting:sandbox.mode` | **工具权限** — 代理程序允许命令做什么：读取文件、修改工作区文件，或使用环境允许的全部功能。 |
| `setting:agent.messages` | **给智能体的说明** — 按消息类别替换 Orch 自己发送给智能体的说明文字。 |
| `setting:models.allowed` | **允许的模型** — 填写在此处允许使用的模型名称模式。 |
| `setting:services.allowed` | **允许的服务** — 指定此处允许使用的已启用供应商服务名称模式；设置列表的每一层范围都必须允许该服务。 |
| `setting:providers.allowed` | **允许的提供者** — 用提供者的 Orch 地址匹配规则限制选择；每个设置了列表的位置都必须允许此处工作所用的提供者。 |
| `setting:models.add_switches_on` | **添加时开启的内容** — 从提供方的目录添加模型时，模型一开始带有什么：它提供的全部内容（全部）、只有强度和上下文大小（基本），或只有它自己的窗口和强度（无）。其余内容之后可由管理者开启。 |
| `setting:collab.attribution` | **署名** — Orch 编写的输入、停止、拒绝和转发说明是否点名：自动在第二个人的话实际送达后署名、始终署名，或永不署名。永不署名时写作某人或审批人，仍保留来源、时间和备注；聊天和文档中的作者署名不变。 |
| `setting:collab.default_delivery` | **消息发送方式** — 线程忙碌时，消息排队、送入当前运行，或中断后优先发送。 |
| `setting:composer.paste_inline_context_share` | **消息中的粘贴文本** — 粘贴文本默认占用剩余上下文的比例。0 表示始终作为文件，100 表示始终放入消息。发送者可以逐次选择。 |
| `setting:env.thread_default` | **新线程** — 选择新线程加入继承得到的默认环境，还是创建自己的环境。 |
| `setting:env.default_id` | **默认环境** — 新线程共享环境且未另选时使用的环境。创建者必须能管理该环境。清除本级选择后继承上级。 |
| `setting:env.update` | **环境更新** — 环境落后于当前配置时，是等待有人更新，还是在每次启动时更新。 |
| `setting:env.template_id` | **环境模板** — 用于创建托管环境的模板。 |
| `setting:env.host_id` | **环境宿主机** — 用于创建托管环境的宿主机；未指定时选择可用空间最多的宿主机。 |
| `setting:env.runtime` | **运行时** — 选择主机提供的运行时。留空时使用模板的默认运行时。 |
| `setting:environments.runtimes` | **运行时规则** — 按名称允许、禁止或要求运行时。* 代表未单独指定的全部运行时。 |
| `setting:env.storage` | **环境存储** — 用于新建工作区、主目录和连接器状态数据的命名机器存储。已有数据保留记录的位置。 |
| `option:env.storage.workspace` | 工作文件的存储。 |
| `option:env.storage.home` | 代理主目录的存储。 |
| `option:env.storage.state` | 已记录文件状态的存储。 |
| `setting:env.data_sources` | **工作区与主目录** — 分别选择由 Orch 创建，或使用此机器 environment.roots 允许的已有目录。已有目录保留所有者和权限，不受卷容量限制，也不会在清除环境时删除。 |
| `option:env.data_sources.workspace` | 工作文件的存储。 |
| `option:env.data_sources.workspace.kind` | 声明的数据源或内容类型。 |
| `option:env.data_sources.workspace.path` | 此设置所属机器上的路径。 |
| `option:env.data_sources.home` | 代理主目录的存储。 |
| `option:env.data_sources.home.kind` | 声明的数据源或内容类型。 |
| `option:env.data_sources.home.path` | 此设置所属机器上的路径。 |
| `setting:env.after_up` | **启动后命令** — 每次托管环境启动后，在运行单元内执行的具名程序及参数。空值移除继承的同名设置；命令失败会阻止启动完成。 |
| `option:env.after_up.*.command` | 直接运行的程序及参数，不经过命令解释器。 |
| `option:env.after_up.*.timeout_secs` | 命令的超时时间，单位为秒。 |
| `setting:env.mounts` | **环境挂载** — 工作区旁的挂载：按挂载原设置、停用、只读，或只读并单独保留环境自己的修改。 |
| `setting:env.resources` | **环境上限** — 替代模板的运行单元资源上限，包括处理器、内存、进程和磁盘。 |
| `option:env.resources.cpu_millis` | CPU 配额，以千分之一个 CPU 为单位。 |
| `option:env.resources.memory_mb` | 内存配额，单位为 MiB。 |
| `option:env.resources.memory_swap_mb` | 内存与交换空间的合计配额，单位为 MiB。 |
| `option:env.resources.blkio_weight` | 块设备输入输出调度的相对权重。 |
| `option:env.resources.pids` | 进程数量上限。 |
| `option:env.resources.disk_gb` | 磁盘配额，单位为 GiB。 |
| `setting:env.needs` | **环境所需资源** — 环境声明至少需要的内存、处理器和磁盘，用于选择运行位置，不预留资源。 |
| `option:env.needs.memory_mb` | 内存配额，单位为 MiB。 |
| `option:env.needs.cpu_millis` | CPU 配额，以千分之一个 CPU 为单位。 |
| `option:env.needs.disk_gb` | 磁盘配额，单位为 GiB。 |
| `setting:env.idle_stop_minutes` | **空闲后停止** — 托管环境无活动多少分钟后停止；零表示永不自动停止。 |
| `setting:env.variables` | **环境变量** — 按名称提供给环境的变量。 |
| `setting:env.secret_names` | **环境机密名称** — 提供给环境的机密名称列表。 |
| `setting:network.allowed_hosts` | **允许访问的主机** — 受限运行单元可访问的其他主机的精确名称。各层列表取交集；空列表只允许 Orch 自身的传输。 |
| `setting:mcp.enabled` | **启用的 MCP 服务器** — 按服务器指定启用哪些 MCP 服务器；下级位置可以关闭继承的服务器。 |
| `setting:exec.version_overrides` | **任意代理版本** — 环境管理者允许与任意代理版本配合使用的机器程序路径和版本。路径或版本改变后需要重新批准。 |
| `option:exec.version_overrides.*.path` | 此设置所属机器上的路径。 |
| `option:exec.version_overrides.*.version` | 选定的确切程序版本。 |
| `option:exec.version_overrides.*.by` | 记录的操作者；在导航设置中则表示排序方式。 |
| `option:exec.version_overrides.*.at` | 记录此决定的时间。 |
| `setting:exec.offers` | **提供的执行方式** — 与提供方一起运行的代理如何在此环境中运行工具，以及尝试这些方式的顺序。 |
| `setting:exec.orch_tools` | **Orch 自身的工具** — 是否向在此环境工作的智能体提供 Orch 自身的工具。 |
| `setting:orch_tools.enabled` | **Orch 工具** — 允许提供给代理的 Orch 工具。 |
| `setting:orch_tools.owner_reads` | **个人读取工具** — 默认关闭。允许会话使用所有者当前的读取权限，共享后仍保持启用。受众提示条显示谁可查看或操作，并说明个人读取内容对他们可见。共享先前结果前仍需知情确认。 |
| `setting:orch_tools.calls_per_minute` | **每分钟工具调用** — 一个线程的智能体每分钟最多调用多少次 Orch 工具。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:orch_tools.search_query_chars` | **共享搜索查询预算** — 向本线程智能体公布并供共享搜索使用的查询字符预算。默认为 1000，可调大以支持更长的查询。 |
| `setting:orch_tools.search_results` | **共享搜索结果预算** — 向本线程智能体公布并供共享搜索使用的结果预算。默认为 200；达到字节或时间预算时仍提供续查游标。 |
| `setting:orch_tools.search_context` | **共享搜索上下文预算** — 向本线程智能体公布并供共享搜索摘要使用的前后行数预算。默认为 5，可调大以查看更广的上下文。 |
| `setting:threads.opening_items` | **线程首屏条目** — 首次打开线程时读取的时间线条目数，须为正数，默认 40。请求可明确指定其他初始窗口，较早内容仍可通过游标继续读取。 |
| `setting:threads.offline_deadline` | **离线回合期限** — 机器离线达到这个秒数后结束当前回合；不设置则一直等待。每次离线沿用它开始时的设置。 |
| `setting:threads.background_wake` | **后台唤醒** — 后台任务或子代理结束时唤醒代理。关闭后，Codex 的结果随下一次输入送达；Claude Code 不提供后台任务，命令在前台运行。变更在下次会话生效。 |
| `setting:threads.agent_access` | **智能体访问权限** — 创建会话时显示的初始范围及各区域权限。创建者确认自己可以授予的权限。仅适用于新会话，不改写已有授权。 |
| `option:threads.agent_access.where` | 代理权限贡献所适用的区域。 |
| `option:threads.agent_access.parts` | 选定区域中各部分的权限。 |
| `option:threads.agent_access.parts.chat` | 讨论区的权限。 |
| `option:threads.agent_access.parts.documents` | 文档权限。 |
| `option:threads.agent_access.parts.timeline` | 线程历史的权限。 |
| `option:threads.agent_access.parts.thread_fork` | 派生线程的权限。 |
| `option:threads.agent_access.parts.thread_archive` | 归档线程的权限。 |
| `setting:threads.let_viewers_in` | **允许会话查看者进入环境** — 新建会话时，是否默认允许其查看者使用环境。创建者确认该选择；更改此默认值本身不会授予任何权限。 |
| `setting:broadcast.chat_messages_per_hour` | **智能体每小时聊天消息** — 同一来源在滚动一小时内向所有聊天成功发送的消息数量；零表示禁止发送。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:sharing.snapshot_max_bytes` | **最大分享副本** — 单次请求可分享给智能体的最大副本大小，以字节计。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:export.max_bytes` | **最大导出文件** — 此处内容单次导出的最大文件大小，以字节计。超过上限时不开始导出，并说明实际大小。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:export.max_items` | **导出内容数量** — 此处内容的一次导出最多包含多少个对话、聊天和文档。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:export.keep_hours` | **导出保留时间** — 完成的导出保留多少小时供发起人下载，然后移除。 |
| `setting:import.max_bytes` | **导入大小限额** — 单次导入允许的展开后字节数。未设置时沿用此位置的导出大小限额。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:import.max_items` | **导入项目限额** — 单次导入允许的对象数量。未设置时沿用此位置的导出项目限额。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:workspace.ignore` | **忽略的路径** — 工作区索引不包含的路径模式。 |
| `setting:workspace.index` | **工作区索引** — 是否为整个工作区建立索引以便离线使用，还是只在环境在线时列出文件。 |
| `setting:source.repository_url` | **源代码仓库** — 仅在线程目录尚不存在时准备的仓库；已有目录保留自己的文件。 |
| `setting:source.branch` | **源代码分支** — 准备哪个分支；未指定时使用仓库公布的默认分支。 |
| `setting:source.secret_name` | **源代码读取机密** — 现有 HTTPS Git 读取机密的名称；其值仅密封发送给准备命令。 |
| `setting:workspace.record_days` | **工作轮次文件保留时间** — 在环境中保留完整轮次文件和恢复撤销文件的天数；零表示关闭记录。 |
| `setting:workspace.record_max_bytes` | **最大记录目录** — 允许进行完整状态记录的文件和链接总字节数；更大的目录使用逐文件撤销。 |
| `setting:workspace.record_wait_secs` | **等待轮次文件** — 轮次开始前等待记录其文件的秒数；超时仍未就绪时，轮次改用逐文件撤销开始，并在轮次上说明。 |
| `setting:broadcast.delivery` | **公告送达方式** — 公告如何送达正在运行的线程：中断、在当前轮次中引导，或等待下一轮。 |
| `setting:broadcast.fallback` | **无法引导时** — 公告原本需要引导当前轮次，但智能体不支持引导时采用的替代方式。 |
| `setting:broadcast.wake_idle` | **允许唤醒空闲线程** — 此处是否允许发送者唤醒空闲线程，出厂开启。每条公告或线程消息单独选择唤醒，省略时为 false。 |
| `setting:broadcast.agent_options` | **智能体可选的公告选项** — 智能体可为一条公告覆盖的投递方式，不含唤醒；每次发送单独选择唤醒。 下级可以移除允许的选项，不能加入上级排除的选项。 |
| `setting:terminal.enabled` | **终端** — 是否允许打开交互终端。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:terminal.record` | **保留终端最后输出** — 终端关闭时，是否在环境历史中保留最后的输出。 |
| `setting:terminal.detached_minutes` | **终端无人连接时间** — 终端在没有人连接后继续运行多少分钟，然后关闭。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:terminal.max_per_person` | **每人的终端数** — 每个人在一个环境中可同时打开的终端数量。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:terminal.max_per_environment` | **每个环境的终端数** — 一个环境内所有人合计最多可以同时打开多少个终端。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:browser.enabled` | **浏览器功能** — 此处环境是否显示浏览器、按请求启动浏览器，并向智能体提供浏览器工具。关闭后，智能体仍可能像运行其他程序一样运行浏览器。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:ports.enabled` | **端口访问** — 已有环境入口权限的人是否可通过 Orch 访问归属该环境的 TCP 和 UDP 端口。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:ports.max_open` | **扩大开放的端口数** — 有扩大访问范围或待审批访问范围的不同端口数量上限；零仍保留已有入口的自身访问。 |
| `setting:ports.link_max_secs` | **端口链接有效期** — 端口链接从创建起的最长有效时间，包括后来更改到期时间的情况。 |
| `setting:requests.expiry_hours` | **智能体请求等待期限** — 智能体发起的边界审批请求最长可以等待多久；请求的端口链接若更早到期，也会结束该请求。 |
| `setting:browser.preset` | **浏览器启动方式** — 没有另行选择时如何启动浏览器：无窗口无显示桌面，或在 Orch 为其打开的显示桌面中显示窗口。 |
| `setting:browser.args` | **浏览器启动参数** — 添加到浏览器启动命令的参数；不能在此设置 Orch 自行提供的参数、配置文件目录或控制地址。 |
| `setting:browser.viewport` | **浏览器窗口大小** — 新浏览器窗口的宽和高，从 320×240 到 3840×2160。 |
| `setting:browser.idle_minutes` | **浏览器空闲关闭时间** — Orch 启动的浏览器在没有人观看或操作、也没有智能体工具使用后，空闲多少分钟自动关闭；零表示永不自动关闭。 |
| `setting:browser.max_per_environment` | **每个环境的浏览器数** — 一个环境内最多同时运行多少个由 Orch 启动的浏览器。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:vnc.enabled` | **显示桌面功能** — 此处环境是否显示桌面、按请求打开桌面，并向智能体提供桌面工具。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:vnc.geometry` | **显示桌面大小** — Orch 打开的显示桌面的宽和高，从 320×240 到 3840×2160。 |
| `setting:vnc.max_per_environment` | **每个环境的显示桌面数** — 一个环境内最多同时运行多少个由 Orch 打开的显示桌面。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:env.idle_warning_minutes` | **空闲停止提醒** — 在环境因空闲停止前多少分钟发送提醒；零表示不提醒。 |
| `setting:env.removed_keep_days` | **移除后保留文件** — 移除运行单元后保留其自有文件的天数，受系统保留期限约束；0 表示可立即永久移除。 |
| `setting:env.unused_remove_days` | **未使用时移除环境** — 从模板创建的环境连续多少天没有线程使用后安排移除；零表示永不自动移除。 |
| `setting:env.lifetime` | **保留期限** — 长期保留、空闲超时后删除，或创建它的线程归档后也删除。只删除自有文件。 |
| `setting:env.temporary_idle_hours` | **临时环境空闲时限** — 临时环境连续空闲多少小时后删除环境及其自有文件。所有者会有一小时用于保留或导出。 |
| `setting:env.recycle_idle_minutes` | **回收的空闲时间** — 回收对话框默认采用的空闲时间，以分钟计；0 表示不设最低空闲时间。 |
| `setting:hosts.full_percent` | **宿主机满载阈值** — 运行内容声明所需内存达到宿主机内存的多少比例时，将宿主机视为已满。 |
| `setting:hosts.when_full` | **宿主机已满时** — 在已满宿主机上创建或迁入环境时，仅提醒，还是提前拒绝。 |
| `setting:runtime.idle_stop_minutes` | **运行时空闲停止** — 默认值 0 会让托管提供方的运行时持续运行。正数表示在没有会话或租约后，空闲这些分钟便停止。定时读取会保持其停止状态，直到轮次、登录、启动、刷新或凭据续期再次启动它。 |
| `setting:runtime.template_id` | **运行时模板** — 用于创建托管提供者运行时的提供者模板。 |
| `setting:runtime.host_id` | **运行时宿主机** — 用于创建托管提供者运行时的宿主机；未指定时选择可用空间最多的宿主机。 |
| `setting:runtime.resources` | **运行时上限** — 在模板基础上设置处理器、内存、进程和磁盘上限。驱动会报告能够强制执行的上限。 |
| `option:runtime.resources.cpu_millis` | CPU 配额，以千分之一个 CPU 为单位。 |
| `option:runtime.resources.memory_mb` | 内存配额，单位为 MiB。 |
| `option:runtime.resources.memory_swap_mb` | 内存与交换空间的合计配额，单位为 MiB。 |
| `option:runtime.resources.blkio_weight` | 块设备输入输出调度的相对权重。 |
| `option:runtime.resources.pids` | 进程数量上限。 |
| `option:runtime.resources.disk_gb` | 磁盘配额，单位为 GiB。 |
| `setting:runtime.storage` | **运行时存储** — 用于新建提供方主目录和连接器状态数据的命名机器存储。已有数据保留记录的位置。提供方运行时没有工作区。 |
| `option:runtime.storage.workspace` | 工作文件的存储。 |
| `option:runtime.storage.home` | 代理主目录的存储。 |
| `option:runtime.storage.state` | 已记录文件状态的存储。 |
| `setting:runtime.home` | **代理主目录** — 选择由 Orch 创建的主目录，或此机器允许使用的已有目录。未设置时保留配置的选择或代理程序的默认目录及其中已有的登录。已有目录保留所有者、权限和文件；删除运行实例也不会删除这些目录。 |
| `option:runtime.home.kind` | 声明的数据源或内容类型。 |
| `option:runtime.home.path` | 此设置所属机器上的路径。 |
| `setting:runtime.needs` | **运行时所需资源** — 托管提供者运行时声明需要的处理器、内存和磁盘，用于选择运行位置。 |
| `option:runtime.needs.memory_mb` | 内存配额，单位为 MiB。 |
| `option:runtime.needs.cpu_millis` | CPU 配额，以千分之一个 CPU 为单位。 |
| `option:runtime.needs.disk_gb` | 磁盘配额，单位为 GiB。 |
| `setting:runtime.after_up` | **运行时启动后命令** — 每次运行实例启动或连接后，在机器允许时执行的具名程序及参数。只有提供方的所有者可以在提供方自身修改这些命令。 |
| `option:runtime.after_up.*.command` | 直接运行的程序及参数，不经过命令解释器。 |
| `option:runtime.after_up.*.timeout_secs` | 命令的超时时间，单位为秒。 |
| `setting:runtime.variables` | **运行时变量** — 运行实例下次启动或连接时，这些变量覆盖模板及智能体配置中的同名变量。 |
| `setting:runtime.secret_names` | **运行时机密名称** — 覆盖智能体配置变量的提供方机密名称。值密封给运行实例所在机器，单独写入，永不返回。 |
| `setting:runtime.update` | **运行时更新** — 运行时落后于当前配置时，是等待有人更新，还是在每次启动时更新。 |
| `setting:runtime.renew_days` | **凭据续期** — 登录凭据剩余有效期少于这些天时续期；若到期日未知，则按距离上次续期的天数续期。零表示关闭自动续期。 |
| `setting:providers.facts_idle_minutes` | **空闲时读取事实的间隔** — 绑定线程近期没有工作时，按此间隔读取提供方事实。默认 60 分钟，至少 1 分钟。 |
| `setting:providers.facts_active_minutes` | **活跃时读取事实的间隔** — 绑定线程正在运行、近期工作过或等待已耗尽的套餐恢复时，按此间隔读取提供方事实。默认 5 分钟，至少 1 分钟。 |
| `setting:providers.facts_recent_minutes` | **提供方近期活动时长** — 绑定线程的轮次结束后，在这些分钟内继续按活跃间隔读取提供方事实。默认 15 分钟，至少 1 分钟。 |
| `setting:providers.facts_refused_minutes` | **凭据被拒时读取事实的间隔** — 凭据被拒时，按此间隔读取提供方事实，以发现外部修复。默认 5 分钟，至少 1 分钟。 |
| `setting:providers.presets` | **提供者预设** — 添加提供者时可按名称选择的现成 API 上游配置；下级位置可以添加、替换或隐藏预设。 |
| `option:providers.presets.*.note` | 与此规则一起记录的说明。 |
| `option:providers.presets.*.docs_url` | 数据源的文档地址。 |
| `option:providers.presets.*.upstreams` | 此供给使用的上游端点。 |
| `option:providers.presets.*.upstreams.*.protocol` | 此数据源声明的端点接口。 |
| `option:providers.presets.*.upstreams.*.base_url` | 端点的基础地址。 |
| `option:providers.presets.*.upstreams.*.options` | 按已声明端点结构传递的选项。 |
| `option:providers.presets.*.template` | 要使用的环境模板。 |
| `option:providers.presets.*.model` | 选定的模型。 |
| `option:providers.presets.*.dimensions` | 此索引的向量维数。 |
| `setting:providers.allow_http_upstreams` | **允许不加密的上游** — 是否允许不经过 TLS 访问 API 端点，例如私有网络中的模型服务器。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:providers.expiry_notice_days` | **凭据到期提醒** — 提供者所用令牌到期前多少天提醒其管理者；零表示仅在到期后提醒。 |
| `setting:providers.subscription_harnesses` | **允许的订阅类型** — 此处允许将哪些代理程序的订阅添加为提供方。 |
| `setting:providers.subscription_sharing` | **可分享的订阅类型** — 允许哪些代理程序的订阅供所有者以外的人使用。 |
| `setting:resources.created_by_members` | **成员可创建的资源** — 普通成员可在组织中为自己创建哪些类型的资源。 |
| `setting:skills.max_bytes` | **技能大小** — 一个技能版本的所有文件合计最多可占用的字节数。 |
| `setting:skills.max_files` | **技能文件数** — 一个技能版本最多可包含的文件数。 |
| `setting:notify.subagents` | **子智能体通知** — 线程的某个子智能体完成、失败或被停止时，是否像轮次结束时一样通知关注该线程的人。 |
| `setting:message.body_max_chars` | **消息长度** — 管理员消息可包含的字符数；系统或组织可修改默认值。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:notify.operations` | **操作通知** — 操作中的“通知”默认在阻断工作时勾选、始终勾选或始终不勾选。操作者确认选择；接收者仍按自己的通知选项接收。 |
| `setting:broadcast.rate_per_hour` | **每小时公告数** — 一个线程的智能体在滚动一小时内最多发送多少条公告；默认 12，零表示不允许发送。线程消息单独计数。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:broadcast.thread_messages_per_hour` | **每小时线程消息数** — 一个发送线程的智能体在滚动一小时内最多向单个线程发送多少条消息；默认 60，零表示不允许发送。系统、组织、项目或话题可替换继承值，但须遵守锁定。公告单独计数。 |
| `setting:capability.recreate_grace_secs` | **重建等待时间** — 重建在中断运行中的工作轮次前，允许它们完成的等待秒数；0 表示立即中断。 |
| `setting:access.request_expiry_days` | **访问请求有效期** — 无人决定的访问请求多少天后到期。 |
| `setting:access.max_pending_requests` | **待处理访问请求数** — 一个人最多可以有多少个等待决定的访问请求。 |
| `setting:access.quick_grant_durations` | **快捷授权时长** — 一键授予访问权限的可选时长：正整数天数后加 d，或用 always 表示永久。 下级可以移除允许的选项，不能加入上级排除的选项。 |
| `setting:nodes.new_color` | **新位置颜色** — 新位置采用所属类型的图标颜色，还是从十二种颜色中随机选择。 |
| `setting:tags.max_per_place` | **位置标签数** — 一个位置最多可以具有多少个标签；0 表示不允许标签。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:agent.default_provider_id` | **新线程的提供方** — 新线程首先使用的提供方，填写其标识。 |
| `setting:agent.default_agent_id` | **新线程的代理配置** — 新线程首先使用的代理配置，填写其标识。 |
| `setting:templates.restricted_drivers` | **受限模板驱动** — 哪些驱动的模板只能由系统管理员定义。 |
| `setting:switch.batch_parallel` | **批量并行数** — 一个批次中最多同时进行多少次切换、迁移或更新。 |
| `setting:switch.batch_threads` | **每批对话数** — 一批操作可规划的对话数；需要更大的切换批次时可调整此默认值。 |
| `setting:jobs.measurement_keepalive_secs` | **保持统计运行** — 关闭最后一个方案后，统计继续运行的秒数。 |
| `setting:jobs.measurement_valid_secs` | **复用统计结果** — 已完成的统计可供其他方案复用的秒数。 |
| `setting:jobs.measurement_timeout_secs` | **统计时限** — 机器完成一次统计可用的秒数。 |
| `setting:jobs.stall_secs` | **进度停滞提示** — 进度无变化达到此秒数时，显示正在等待什么。 |
| `setting:jobs.stop_wait_secs` | **停止等待时间** — 停止尚未获确认达到此秒数时，显示仍在停止的机器。 |
| `setting:jobs.io_timeout_secs` | **复制时限** — 机器完成一次文件操作可用的秒数。 |
| `setting:switch.active_turn` | **运行中的工作轮次** — 切换、迁移、重建或更新遇到运行中的轮次时，等待完成，还是中断。 |
| `setting:switch.wait_minutes` | **等待轮次完成** — 等待运行中的轮次结束多少分钟后放弃，并保持原状；0 表示不等待。 |
| `setting:switch.verify_timeout_secs` | **健康检查时限** — 新运行位置的检查最多可以进行多少秒，超过后回滚全部更改。 |
| `setting:switch.keep_previous` | **保留原运行位置** — 是否保留切换或迁移前的运行位置，而不是安排移除。 |
| `setting:binding.prefer` | **运行方式的顺序** — 优先按此顺序尝试代理运行方式，其他支持的方式随后尝试。 |
| `setting:binding.allowed` | **允许的运行方式** — 控制代理可在哪里运行，以及能否与环境共享登录信息。 |
| `setting:credential.refresh_margin_secs` | **共享凭据续期提前量** — 共享的凭据副本剩余有效期少于这些秒时续期。 |
| `setting:auto.title` | **自动标题** — 自动标题的来源：不生成、使用首条输入、使用代理提供的标题，或生成摘要。 |
| `setting:auto.tags` | **自动标签** — 摘要器是否为位置设置标签。 |
| `setting:summary.handoff_input_share` | **摘要输入比例** — 交接、分叉或目标核对发送给摘要模型的对话最多占其上下文窗口的比例：先发最新的轮次，更早的只发一行索引。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:auto.summarizer` | **摘要器** — 使用哪个摘要器及其配置。 |
| `option:auto.summarizer.kind` | 声明的数据源或内容类型。 |
| `setting:auto.triggers` | **自动更新的条件** — 何时重新生成自动标题、标签和摘要。 |
| `option:auto.triggers.on` | 触发此自动操作的事件。 |
| `option:auto.triggers.min_interval_secs` | 自动运行之间的最短间隔，单位为秒。 |
| `option:auto.triggers.interval_secs` | 定时间隔，单位为秒。 |
| `setting:capability.policy` | **能力策略** — 逐项指定能力请求是允许、询问还是拒绝，以及由谁决定。 |
| `option:capability.policy.*.decision` | 此能力是直接允许，还是需要审批。 |
| `option:capability.policy.*.approver_scope` | 由哪个区域提供审批人。 |
| `setting:sharing.allow_external` | **所有者可分享给外部人员** — 允许此处内容的所有者在权限页中添加组织树以外的人。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:sharing.allow_copy_out` | **复制到其他组织** — 是否允许把此处的对象复制或移动到其他组织。默认开启。下级可以关闭此权限，不能推翻上级的禁止；导出和在目标位置创建仍需各自的权限。 |
| `setting:sharing.external_max_days` | **外部访问有效期** — 点名外部人员的访问条目从写入或续期起最多可以有效多少天。 |
| `setting:sharing.allow_holders` | **权限持有人可分享** — 不拥有对象也不管理权限页的人，是否可在自己已有权限范围内分享：添加权限持有人、授予位置角色或分享给智能体。关闭时，仅所有者和权限页管理者可分享。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:sharing.on_link` | **发送时共享查看权限** — 此处发送链接时的默认查看权限共享选项。每个人也可以在个人设置中关闭。 |
| `setting:usage.rates` | **价格表** — 按模型 ID 匹配规则设置使用价格，以点数计。 |
| `option:usage.rates.*.web_search_per_call` | 每次成功网页搜索的点数。 |
| `option:usage.rates.*.input_per_mtok` | 每百万输入词元的点数。 |
| `option:usage.rates.*.cached_input_per_mtok` | 每百万缓存输入词元的点数。 |
| `option:usage.rates.*.output_per_mtok` | 每百万输出词元的点数。 |
| `option:usage.rates.*.cache_write_per_mtok` | 每百万缓存写入词元的点数。 |
| `option:usage.rates.*.cache_write_1h_per_mtok` | 每百万保留一小时的缓存写入词元的点数。这些词元属于缓存写入总量，此价格替代其普通写入价格；缺少报价时仍为未知。 |
| `option:usage.rates.*.cache_write_12h_per_mtok` | 每百万保留十二小时的缓存写入词元的点数。这些词元属于缓存写入总量中的独立部分，此价格替代其普通写入价格；缺少报价时仍为未知。 |
| `option:usage.rates.*.tiers` | 各声明档位的费率或限额。 |
| `option:usage.rates.*.capacity_tiers` | 按调用实际报告的容量档位设置十进制价格倍率，与速度独立。原生搜索和读取保留各自的费用。 |
| `option:usage.rates.*.above` | 超过指定上下文大小时适用的费率。 |
| `option:usage.rates.*.above.*.context_tokens` | 上下文大小的分界值，以词元为单位。 |
| `option:usage.rates.*.above.*.input_per_mtok` | 每百万输入词元的点数。 |
| `option:usage.rates.*.above.*.cached_input_per_mtok` | 每百万缓存输入词元的点数。 |
| `option:usage.rates.*.above.*.output_per_mtok` | 每百万输出词元的点数。 |
| `option:usage.rates.*.above.*.cache_write_per_mtok` | 每百万缓存写入词元的点数。 |
| `option:usage.rates.*.above.*.cache_write_1h_per_mtok` | 超过此上下文大小时，每百万一小时缓存写入词元的点数；缺少报价时仍为未知。 |
| `option:usage.rates.*.above.*.cache_write_12h_per_mtok` | 超过此上下文大小时，每百万十二小时缓存写入词元的点数；缺少此项则继承根报价，两处均缺少时价格仍为未知。 |
| `setting:usage.catalogue` | **使用费用估算** — 没有单独价格的模型使用量，是否根据 Orch 随附的价格目录估算。 |
| `setting:usage.catalogue_scale` | **价格目录换算比例** — 价格目录的一个单位相当于此位置多少点数。 |
| `setting:usage.catalogue_url` | **价格目录地址** — 系统可从中获取新版价格目录的 HTTPS 地址。 |
| `setting:usage.catalogue_refresh_days` | **价格目录刷新间隔** — 系统每隔多少天自动获取价格目录；零表示仅在管理员要求时获取。 |
| `setting:chat.enabled` | **聊天发言** — 是否允许人在此处节点的聊天中发言。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:chat.direct` | **私聊和群聊** — 根组织的成员是否可以相互发起私聊或群聊。 下级只能选择更严格的选项。 严格程度依次为：`members` → `off`，最后一项最严格。 |
| `setting:chat.group_max_members` | **群聊人数** — 一个群聊最多可以包含多少人。 |
| `setting:chat.group_mentions` | **角色和群组提及** — 谁可以点名当前读者中的角色或群组受众：聊天写入者，或群聊的当前管理者。 下级只能选择更严格的选项。 严格程度依次为：`writers` → `managers`，最后一项最严格。 |
| `setting:notifications.person_rate_per_hour` | **每小时可通知的人数** — 每人在一个根组织内、滚动一小时中可主动通知的人数；0 表示不限制。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:notifications.person_note_chars` | **人员通知备注长度** — 可选通知备注允许的字符数；0 表示不能添加备注。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:references.preview_bytes` | **文件预览大小** — 引用卡片读取文本摘录的字节数。 |
| `setting:references.preview_lines` | **文件预览上下文** — 引用行前后显示的行数；0 表示仅显示引用行。 |
| `setting:files.preview_source_bytes` | **预览原文件大小** — 生成静态文件预览时可解码的原文件大小上限。 |
| `setting:files.preview_pixels` | **预览图片像素** — 生成静态预览可解码的图片像素总数，在分配像素内存前检查。 |
| `setting:files.preview_edge` | **预览尺寸** — 静态缩略图最长边的像素数。 |
| `setting:files.preview_bytes` | **预览大小** — 单张静态缩略图或文本摘要的编码字节数上限。 |
| `setting:files.preview_cache_bytes` | **预览缓存** — 派生预览的缓存字节数，优先清除较早的预览；设为零则不缓存。 |
| `setting:files.viewer_bytes` | **媒体查看器文件大小** — 浏览器查看器可读取的图片、PDF、音频或视频原文件大小上限。 |
| `setting:files.native_image_bytes` | **原生图片输入大小** — 可直接发送给模型的图片大小上限；更大的图片通过路径交付，设为零则始终使用路径。 |
| `setting:docs.enabled` | **创建文档** — 是否允许在此处节点旁创建新文档。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:docs.templates` | **文档起始内容** — 在此处创建文档时可选择的、当前可读的普通文档。作为模板不授予来源访问权或创建权限。 |
| `setting:docs.max_bytes` | **文档大小** — 此处单个文档最多可包含的文本大小。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:docs.max_blocks` | **文档区块数** — 此处单个文档最多可以包含多少个区块。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:docs.versions_kept` | **文档版本数** — 此处单个文档最多保留多少个版本。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:docs.history_max_bytes` | **文档历史大小** — 此处单个文档保留的所有版本合计最多占用多少字节。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:docs.file_max_bytes` | **文档单个文件大小** — 此处文档内单个文件的最大字节数。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:docs.files_max_bytes` | **文档文件总大小** — 此处单个文档内所有文件合计的最大字节数。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:documents.links.shown` | **链接标签页** — 文档的“链接”部分向读者显示的标签页。文档所有者可以为单个文档关闭标签页；此处加锁则对整个位置生效。 |
| `setting:documents.links.page_size` | **链接每页行数** — “链接”列表在“显示更多”之前显示的行数。 |
| `setting:documents.links.occurrences` | **每个来源的提及数** — “被引用”中每个来源在“显示更多”之前列出的提及数。 |
| `setting:documents.links.line_chars` | **提及行长度** — “被引用”中每处提及周围显示的字符数。 |
| `setting:documents.links.mentions_counted` | **统计的提及数** — 单个来源中统计到多少处对该文档的提及，再显示为“以上”。 |
| `setting:documents.links.access_checks` | **链接权限检查数** — “被引用”每页检查阅读权限的来源数。被大量引用的文档会用游标继续。 |
| `setting:documents.links.graph_nodes` | **关系图节点数** — “链接”关系图最多绘制的节点数。 |
| `setting:documents.readme_blocks` | **README 预览** — 位置的 README 预览显示的顶层块数，同时保留其子块。遇到更早的分页符时结束预览；“打开文档”始终可查看完整页面。 |
| `setting:chat.attachment_max_bytes` | **聊天附件大小** — 单条聊天消息所附单个文件的最大字节数。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:search.text_chars` | **对话搜索文本** — 每条对话记录脱敏后索引的字符数。更改适用于新文本及重新构建的索引；不索引原始调用、输出、隐藏推理、子线程以及无法读取脱敏所需信息的文本。 |
| `setting:search.word_recent_candidates` | **近期文本样本** — 探查文本索引前抽样的近期记录数。 |
| `setting:search.word_candidates` | **文字候选数** — 每个索引字段在权限检查和排序前提取的候选数。 |
| `setting:search.word_probe_candidates` | **文字索引采样** — 转为按时间扫描前，采样的索引匹配数。 |
| `setting:search.word_tuning` | **文字检索** — 预计行数低于此值时，从可访问范围内读取候选；未单独设置的字段沿用文字候选数。每次请求默认读取一个窗口，显示更多会继续。 |
| `option:search.word_tuning.scope_candidates` | 在已授权搜索范围内检查的行数。 |
| `option:search.word_tuning.request_windows` | 搜索请求的时间窗口限额。 |
| `option:search.word_tuning.tiers` | 各声明档位的费率或限额。 |
| `option:search.word_tuning.tiers.title` | 标题文字或其排序权重。 |
| `option:search.word_tuning.tiers.description` | 描述文字或其排序权重。 |
| `option:search.word_tuning.tiers.heading` | 章节标题文字或其排序权重。 |
| `option:search.word_tuning.tiers.body` | 正文文字或其排序权重。 |
| `setting:search.page_size` | **每页条数** — 点击显示更多前展示的结果数。 |
| `setting:search.reorder_window_secs` | **排序窗口** — 新结果可以调整首屏顺序的持续时间。 |
| `setting:search.meaning_pause_ms` | **输入停顿** — 输入停顿多久后请求嵌入，单位为毫秒。 |
| `setting:search.cursor_entries` | **保留的搜索** — 为每位读者保留的搜索续读位置。 |
| `setting:search.cursor_ttl_secs` | **搜索有效期** — 搜索可以继续加载结果的有效时间。 |
| `setting:search.vector_cache_entries` | **查询缓存大小** — 每个组织缓存的查询向量上限；零表示不缓存。 |
| `setting:search.vector_cache_secs` | **查询向量缓存** — 同一组织和模型重复使用查询向量的时间。 |
| `setting:search.semantic.enabled` | **语义搜索** — 使用下方的嵌入模型，同时查找含义相近的结果。 下级可以关闭此权限，不能推翻上级的禁止。 |
| `setting:related.weights` | **相关内容权重** — 用零到一的小数设置语义、位置和引用的权重。零表示不使用该信号。 |
| `option:related.weights.meaning` | 语义相似度的权重。 |
| `option:related.weights.place` | 所属位置接近程度的权重。 |
| `option:related.weights.references` | 显式引用的权重。 |
| `setting:related.place_scores` | **相关位置分值** — 同一文件夹、话题、项目或组织的分值，范围为零到一。 |
| `option:related.place_scores.folder` | 同一文档目录的评分。 |
| `option:related.place_scores.topic` | 同一话题的评分。 |
| `option:related.place_scores.project` | 同一项目的评分。 |
| `option:related.place_scores.org` | 同一组织的评分。 |
| `setting:related.count` | **相关内容条数** — 相关内容显示的条数。零表示关闭。 |
| `setting:related.candidates` | **相关候选数** — 精确检查阅读权限前，每个信号及引用邻域收集的候选数。 |
| `setting:search.semantic.embeddings` | **嵌入模型** — 用于计算嵌入向量的提供方和模型；用量计入指定者名下。 |
| `option:search.semantic.embeddings.provider_id` | 选定的提供方标识。 |
| `option:search.semantic.embeddings.model_name` | 选定模型的路由名称。 |
| `option:search.semantic.embeddings.dimensions` | 此索引的向量维数。 |
| `option:search.semantic.embeddings.query_prefix` | 生成搜索向量时放在查询前的文字。 |
| `option:search.semantic.embeddings.document_prefix` | 生成文档向量时放在文档前的文字。 |
| `setting:search.semantic.max_disk_share` | **磁盘限制** — 索引大小占所索引内容大小的上限；达到上限时暂停索引。留空：不限。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:search.semantic.tuning` | **索引与排序** — 文本如何分块、生成嵌入向量及排序。 |
| `option:search.semantic.tuning.chunk_tokens` | 每个索引文本块的目标词元数。 |
| `option:search.semantic.tuning.batch_tokens` | 一次索引批次的词元上限。 |
| `option:search.semantic.tuning.candidates` | 参与排序的候选匹配数。 |
| `option:search.semantic.tuning.oversample` | 额外考虑的向量候选数量。 |
| `option:search.semantic.tuning.fusion_k` | 排名融合的权重常量。 |
| `option:search.semantic.tuning.same_model_cosine_milli` | 同模型相似度阈值，以千分之一为单位。 |
| `option:search.semantic.tuning.interval_secs` | 定时间隔，单位为秒。 |
| `option:search.semantic.tuning.exact_filter_candidates` | 精确搜索考虑的筛选后候选数。 |
| `option:search.semantic.tuning.source_batch` | 每个索引批次读取的源记录数。 |
| `option:search.semantic.tuning.max_scan_tuples` | 扫描最多访问的索引元组数。 |
| `option:search.semantic.tuning.scan_mem_multiplier` | 向量扫描的内存倍数。 |
| `option:search.semantic.tuning.ef_search` | 图搜索的广度。 |
| `option:search.semantic.tuning.iterative_scan` | 向量扫描如何继续查找更多候选。 |
| `option:search.semantic.tuning.query_wait_secs` | 查询等待相应索引工作的时间，单位为秒。 |
| `setting:retention.archive_idle_days` | **归档空闲话题** — 话题无活动多少天后自动归档；零表示永不自动归档。 |
| `setting:retention.delete_archived_days` | **删除已归档节点** — 节点归档多少天后删除；零表示永不自动删除。 |
| `setting:retention.agent_history_idle_days` | **代理历史保留天数** — 线程多少天没有运行轮次后移除代理程序自己的历史；0 表示不自动移除。 |
| `setting:retention.inbox_days` | **收件箱保留时长** — 通知在个人收件箱中保留多少天后删除，无论是否已读。 |
| `setting:redaction.mask_secrets` | **遮盖机密** — 是否在保存的文字中遮盖曾提供给线程的机密值。 下级可以要求此保护，不能关闭上级要求的保护。 |
| `setting:redaction.patterns` | **额外遮盖规则** — 需要在保存文字中遮盖的其他匹配规则，每个规则具有名称。 |
| `option:redaction.patterns.*.name` | 声明项目的可读名称。 |
| `option:redaction.patterns.*.regex` | 此规则使用的正则表达式。 |
| `setting:diagnostics.retention_days` | **诊断保留时间** — 诊断信息保留多少天。 |
| `setting:orgs.personal_enabled` | **创建个人组织** — 为新账号创建个人组织。关闭此选项会保留已有组织；用户仍可加入团队和接收共享文档。 |
| `setting:orgs.personal_closed` | **个人组织不接纳成员** — 是否将个人组织创建为仅容纳其本人的组织。 |
| `setting:orgs.personal_flat` | **个人组织不嵌套组织** — 是否将个人组织创建为不包含子组织的组织。 |
| `setting:orgs.created_by` | **谁可以创建组织** — 谁可以创建团队组织：所有已登录用户、仅系统管理员，或获系统管理员邀请的用户（每次邀请仅使用一次）。个人组织的创建由独立策略控制。 |
| `setting:navigation.order` | **树的顺序** — 对遵循组织自身顺序的人，组织的树按此显示：按某人的安排、最新在前、最近更改在前，或按名称。 |
| `option:navigation.order.by` | 记录的操作者；在导航设置中则表示排序方式。 |
| `option:navigation.order.custom` | 为各父级明确指定的导航顺序。 |
| `setting:registration.mode` | **注册方式** — 允许谁注册：无人、持邀请的人，或能访问此系统的任何人。 |
| `setting:org.terms` | **加入者须接受的条款** — 加入此组织或接受其位置、聊天的访客邀请前需接受的 Markdown 条款。先显示根组织的条款，再显示各级下属组织的条款。空文本不添加条款；修改后不会要求现有成员再次接受。 |
| `setting:invitations.forms` | **邀请方式** — 此组织树内可创建的邀请形式。 下级可以移除允许的选项，不能加入上级排除的选项。 |
| `setting:invitations.default_hours` | **邀请有效时间** — 邀请人未另作选择时，邀请默认有效多少小时。 |
| `setting:invitations.max_hours` | **邀请的最长有效时间** — 邀请最多可以有效多少小时。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:invitations.max_uses` | **共享链接的使用次数** — 一个共享邀请链接最多可以邀请多少人。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:mail.shared` | **共享邮件服务器** — 没有独立邮件服务器的根组织是否通过系统邮件服务器发送邮件。 |
| `setting:api.idempotency_window` | **创建重试保留时间** — 创建请求的键保留已接受对象标识的小时数；0 表示不保留。可按需要延长，24 小时是默认值。 |
| `setting:release.check_days` | **检查新版发布** — 对管理员配置的发布来源进行只读检查的间隔天数；零表示关闭。检查不发送安装统计，也不安装任何内容。 |
| `setting:cleanup.release_keep_count` | **保留未使用的发布** — 每次更新后，各载荷保留的未使用发布数量；0 表示全部保留。当前和上一个发布始终保留。 |
| `setting:cleanup.unused_keep_days` | **清理未使用的缓存** — 没有使用者的镜像和工具版本保留多少天；0 表示关闭自动清理。数据遵循各自的保留期限。 |
| `setting:backup.warn_days` | **数据库备份提醒** — 最近一次成功的仅数据库备份超过这些天时提醒；失败的备份尝试不会更新成功时间。 |
| `setting:registration.email_verification` | **验证邮箱** — 用户是否必须验证邮箱后才能登录。 |
| `setting:auth.local_login` | **密码登录** — 系统管理员以外的人是否可通过密码或邮件验证码登录。 |
| `setting:auth.email_code` | **邮件验证码** — 发送到已验证邮箱的验证码可用于哪些登录方式。 下级可以移除允许的选项，不能加入上级排除的选项。 |
| `setting:auth.second_step` | **第二重验证** — 此组织中的人是否必须设置并提供第二重身份证明才能工作。 下级只能选择更严格的选项。 严格程度依次为：`optional` → `required`，最后一项最严格。 |
| `setting:tokens.max_days` | **令牌最长有效期** — 此处接受的 API 令牌从创建到到期的最长总有效时间。 |
| `setting:system.banner` | **系统横幅** — 向所有人显示的消息，包括其级别和结束时间。 |
| `option:system.banner.text` | 此规则携带的文字。 |
| `option:system.banner.level` | 请求的权限级别。 |
| `option:system.banner.until` | 此规则停止适用的时间。 |
| `setting:outbound.allowed_private_hosts` | **可访问的私网主机** — 组织或个人的通知渠道或目标可以访问哪些私有地址上的主机。 |
| `setting:storage.max_blob_bytes` | **系统单个文件上限** — Orch 保存的单个文件的最大字节数。 |
| `setting:storage.env_cache_quota_bytes` | **环境文件缓存** — 每个环境最多保留多少字节的文件内容缓存。 下级只能降低此上限，不能取消上级设定的限制。 |
| `setting:agents.page_fetch` | **网页抓取** — 未设置时保留程序默认行为。关闭不提供此能力；原生使用已声明的网页读取工具；服务使用明确选择的抓取后端。它与网页搜索分别设置。 |
| `setting:agents.image_generation` | **图像生成** — 未设置时遵循程序默认值。“原生”使用受支持的原生图像工具，“关闭”禁用它，“服务”使用明确选择的后端并关闭原生生成。默认值不是锁定，也不会自动选择付费服务。 |
| `setting:gateways.tunnel_strategy` | **隧道网关** — 新连接按双方有效往返时延的总和选择网关，也可固定使用执行端网关。已有隧道沿用建立时的方案。 |
| `setting:threads.agent_schedules` | **智能体定时任务** — 允许智能体为本线程安排可见的后续输入。默认关闭；每次运行仍须通过当前权限和提供方额度检查。 |
| `setting:agents.image_generation_backend` | **图像生成后端** — 选择确切的图像路由和已启用的提供方项目。此选择沿作用域继承到线程，并遵守上级锁定。未设置时不选择付费服务；运行时仍检查提供方的当前使用权限和限额。 |
| `option:agents.image_generation_backend.capabilities` | 按能力名称保存的明确后端选择。值为 null 时仅清除该能力的服务后端；缺少条目时只使用保留的默认值。能力的运行模式仍单独选择。 |
| `option:agents.image_generation_backend.capabilities.*.route` | 此能力声明的路由，包括程序提供的委派会话。路由必须支持当前能力；填写另一能力的路由不能启用它。 |
| `option:agents.image_generation_backend.capabilities.*.offering` | 此条目选择的确切提供方及已启用模型或服务。每个能力保留自己的选择，并在使用时检查当前权限。 |
| `option:agents.image_generation_backend.capabilities.*.offering.provider` | 为此能力选择的提供方账户，可以与对话提供方不同；普通使用权限、共享策略和允许列表仍然适用。 |
| `option:agents.image_generation_backend.capabilities.*.offering.offering` | 所选模型或服务在提供方中的名称。委派时，它指定助手使用的模型；不会自动选择登录或模型。 |
| `option:agents.image_generation_backend.capabilities.*.config` | 只填写该路由文档说明的附加引用，否则保持为空。此值不存放凭据，也不能授予权限或增加使用额度。 |
| `option:agents.image_generation_backend.default` | 未明确覆盖的能力使用的保留后端值。修改一个能力时，其他能力仍使用此值。未设置默认值不会选择付费服务。 |
| `option:agents.image_generation_backend.route` | 已声明的图像路由，例如使用图像接口的 image_generation.service-api、使用托管模型工具的 image_generation.service-model，或使用符合条件的原生程序助手的 image_generation.delegation。不可用的路由保持不可用，不会自动改用另一提供方。 |
| `option:agents.image_generation_backend.offering` | 此路由使用的确切提供方及模型或服务。仅当路由不需要项目时才省略；选择项目本身不会授予访问权限。 |
| `option:agents.image_generation_backend.offering.provider` | 本次图像调用所属的提供方账户，可与对话提供方不同。驱动线程的人必须具有其当前使用权限，所选项目还必须符合适用的允许列表。 |
| `option:agents.image_generation_backend.offering.offering` | 路由要求的已启用图像服务、托管工具模型或原生助手模型在提供方中的名称。请选择确切项目，不使用账户默认项目或按价格推断的后备项目。 |
| `option:agents.image_generation_backend.config` | 所选路由理解的附加引用。除非路由说明需要，否则保持为空；不要在此填写提供方凭据。此对象不能授予使用权限、绕过限额或替代当前来源授权。 |
| `setting:agents.web_search_backend` | **网页搜索后端** — 选择已启用的提供方及原生搜索模型，或该提供方的搜索和抓取服务。未设置时不会自动启用付费服务。 |
| `option:agents.web_search_backend.route` | 确切的搜索或页面读取路由，由已启用模型、提供方服务或受支持的委派会话执行；不会推断另一提供方。 |
| `option:agents.web_search_backend.offering` | 此路由使用的提供方及已启用模型或服务。搜索和页面读取可以通过各自的能力条目选择不同项目。 |
| `option:agents.web_search_backend.offering.provider` | 提供本次搜索或页面读取操作的账户；运行时会检查其当前使用权限、共享策略和限额。 |
| `option:agents.web_search_backend.offering.offering` | 该提供方中确切的模型或服务名称。路由必须支持请求的能力，不会推断默认项目或付费后备项目。 |
| `option:agents.web_search_backend.config` | 仅填写路由文档说明的附加引用。无需引用时保持为空；不要放入提供方凭据，也不能用它绕过限额。 |
| `option:agents.web_search_backend.capabilities` | 按能力保存的独立后端选择，例如 web_search 和 page_fetch。修改搜索不会替换页面读取的选择。null 仅清除该后端选择，运行模式仍单独设置。 |
| `option:agents.web_search_backend.capabilities.*.route` | 只为当前能力提供服务的声明路由。仅在提供方当前支持时选择模型、接口服务或委派路由。 |
| `option:agents.web_search_backend.capabilities.*.offering` | 只为当前能力选择的确切提供方及已启用项目；不会借用另一能力的选择。 |
| `option:agents.web_search_backend.capabilities.*.offering.provider` | 此能力后端所属的账户；请求者仍需在所在位置策略下具有其当前使用权限。 |
| `option:agents.web_search_backend.capabilities.*.offering.offering` | 此能力所选模型或服务在提供方中的名称，必须符合路由支持范围及适用的模型或服务允许列表。 |
| `option:agents.web_search_backend.capabilities.*.config` | 仅填写所选路由文档说明的附加引用。凭据保留在提供方中，当前来源权限和额度限制不会因此改变。 |
| `option:agents.web_search_backend.default` | 没有覆盖条目时使用的明确保留后端，也可以保留已有模型或搜索与抓取服务选择。缺省表示未选择服务后端，不会自动发现付费项目。 |
| `setting:threads.agent_schedule_hours` | **智能体定时任务时长** — 重复定时任务的默认有效小时数。明确指定的结束时间可以更晚；这是默认值，不是上限。 |
| `setting:threads.agent_schedule_runs` | **智能体定时任务次数** — 重复定时任务的默认运行次数。智能体可以明确指定其他正数；这是默认值，不是上限。 |
| `setting:summary.targets` | **摘要目标** — 按目标设置自动刷新、触发时机及输入输出默认值，默认关闭，仍受摘要器选择和共享用量限额约束。帮助和搜索结果使用人工编写或已有的摘要。 |
| `setting:summary.follow_interval_secs` | **摘要跟踪间隔** — 摘要可运行或正在运行时的免费读取间隔；尚未到期的任务等待实际到期时间。默认 5 秒。 |
| `setting:summary.read_batch_size` | **摘要读取批量大小** — 一个搜索摘要窗口读取的不同来源数量，其余来源继续读取后续窗口。默认 100。 |
| `option:agents.web_search_backend.kind` | 选择由模型或接口服务提供搜索的后端。 |
| `option:agents.web_search_backend.provider_id` | 选定搜索供给所属的已启用提供方。 |
| `option:agents.web_search_backend.model_name` | 使用模型后端时，选定提供方上的原生搜索模型。 |
| `option:agents.web_search_backend.search_service` | 使用接口后端时，选定提供方声明的搜索服务。 |
| `option:agents.web_search_backend.fetch_service` | 选定提供方上可选的已声明网页抓取服务。 |
| `option:summary.targets.thread` | 线程摘要选项。 |
| `option:summary.targets.thread.automatic` | 允许此目标自动刷新。默认关闭；手动刷新是独立操作。 |
| `option:summary.targets.thread.on` | 可以触发自动刷新的事件。省略时采用目标的常规事件；空列表关闭事件触发。 |
| `option:summary.targets.thread.min_interval_secs` | 自动刷新的最短间隔，单位为秒；零表示不额外限制间隔。 |
| `option:summary.targets.thread.context_tokens` | 正数的输入词元预算；省略时使用摘要器的默认值 32,000，并受模型可用窗口减去输出的约束。 |
| `option:summary.targets.thread.output_chars` | 输出字符预算，必须为正数；不会覆盖提供方限制或用量限额。 |
| `option:summary.targets.topic` | 话题摘要选项。 |
| `option:summary.targets.topic.automatic` | 允许此目标自动刷新。默认关闭；手动刷新是独立操作。 |
| `option:summary.targets.topic.on` | 可以触发自动刷新的事件。省略时采用目标的常规事件；空列表关闭事件触发。 |
| `option:summary.targets.topic.min_interval_secs` | 自动刷新的最短间隔，单位为秒；零表示不额外限制间隔。 |
| `option:summary.targets.topic.context_tokens` | 正数的输入词元预算；省略时使用摘要器的默认值 32,000，并受模型可用窗口减去输出的约束。 |
| `option:summary.targets.topic.output_chars` | 输出字符预算，必须为正数；不会覆盖提供方限制或用量限额。 |
| `option:summary.targets.document` | 文档摘要选项。 |
| `option:summary.targets.document.automatic` | 允许此目标自动刷新。默认关闭；手动刷新是独立操作。 |
| `option:summary.targets.document.on` | 可以触发自动刷新的事件。省略时采用目标的常规事件；空列表关闭事件触发。 |
| `option:summary.targets.document.min_interval_secs` | 自动刷新的最短间隔，单位为秒；零表示不额外限制间隔。 |
| `option:summary.targets.document.context_tokens` | 正数的输入词元预算；省略时使用摘要器的默认值 32,000，并受模型可用窗口减去输出的约束。 |
| `option:summary.targets.document.output_chars` | 输出字符预算，必须为正数；不会覆盖提供方限制或用量限额。 |
| `option:summary.targets.chat` | 聊天补读摘要选项。 |
| `option:summary.targets.chat.automatic` | 允许此目标自动刷新。默认关闭；手动刷新是独立操作。 |
| `option:summary.targets.chat.on` | 可以触发自动刷新的事件。省略时采用目标的常规事件；空列表关闭事件触发。 |
| `option:summary.targets.chat.min_interval_secs` | 自动刷新的最短间隔，单位为秒；零表示不额外限制间隔。 |
| `option:summary.targets.chat.context_tokens` | 正数的输入词元预算；省略时使用摘要器的默认值 32,000，并受模型可用窗口减去输出的约束。 |
| `option:summary.targets.chat.output_chars` | 输出字符预算，必须为正数；不会覆盖提供方限制或用量限额。 |
| `option:summary.targets.chat_title` | 聊天标题选项；独立缓存的主题，默认最多80个字符，已有群聊名称优先显示。 |
| `option:summary.targets.chat_title.automatic` | 允许此目标自动刷新。默认关闭；手动刷新是独立操作。 |
| `option:summary.targets.chat_title.on` | 可以触发自动刷新的事件。省略时采用目标的常规事件；空列表关闭事件触发。 |
| `option:summary.targets.chat_title.min_interval_secs` | 自动刷新的最短间隔，单位为秒；零表示不额外限制间隔。 |
| `option:summary.targets.chat_title.context_tokens` | 正数的输入词元预算；省略时使用摘要器的默认值 32,000，并受模型可用窗口减去输出的约束。 |
| `option:summary.targets.chat_title.output_chars` | 输出字符预算，必须为正数；不会覆盖提供方限制或用量限额。 |
| `option:summary.targets.help` | 人工编写的帮助摘要没有模型生成选项。 |
| `option:summary.targets.search_results` | 已有的搜索结果摘要没有模型生成选项。 |
| `setting:ask.models` | **问一问模型列表** — 配置允许的 API 模型标识；子级只能缩小并保留父级顺序，首个可用项为默认。各级均未配置或有效列表为空时阻止提问。 |
| `setting:ask.paid_by` | **Ask 的费用承担方** — 由提问者或写入本设置的人付费。用量始终归属于提问者。 |
| `setting:ask.retention_days` | **Ask 历史保留时间** — 最后一条消息后多少天永久删除历史，且须符合会话根组织规定的最短和最长保留时间。 |
| `setting:ask.retention_min_days` | **Ask 最短保留时间** — 会话根组织的管理员规定的最短历史保留时间。 下级只能提高此下限。 |
| `setting:ask.retention_max_days` | **Ask 最长保留时间** — 会话根组织的管理员允许的最长历史保留时间。 |
| `setting:ask.max_steps` | **Ask 每轮模型步骤数** — 每个问题允许的模型步骤数，达到后请提问者选择继续。 |
| `setting:ask.max_output_tokens` | **Ask 回答令牌数** — 每次 Ask 模型请求允许的最大输出令牌数。请求按窗口减去此值来安排，且回答最多占窗口的一半。系统和组织设置中的最小值生效。 |
| `setting:ask.context_tokens` | **Ask 上下文令牌数** — 未设置时使用所选模型的上下文窗口；未记录窗口的模型不会被当作小窗口。设置后仅对 Ask 取较小值。窗口不超过回答令牌数两倍时，回答占其一半。系统和组织设置中的最小值生效。 |
| `setting:ask.max_tools` | **Ask 每轮读取操作数** — 每个问题允许的读取操作数，达到后请提问者缩小问题范围。 |
| `setting:ask.docs_hits` | **Ask 每次搜索的指南页数** — Ask 每次搜索帮助时交给模型的指南页面数上限；中英文同时匹配时交替给出。 |
| `setting:ask.docs_excerpt_tokens` | **Ask 指南摘录大小** — Ask 搜索帮助时交给模型的单个指南页面的最大令牌数，并附上该页面的链接。 |
| `setting:ask.max_operations` | **Ask 每次搜索的操作数** — Ask 每次搜索读取目录时返回的读取操作数上限。 |
| `setting:ask.max_links` | **Ask 每个来源的链接数** — Ask 在一次读取中保留的标准来源链接数上限。 |
| `setting:providers.model_groups_max` | **模型分组数** — 每个模型可加入的分组数，默认 8 个，可提高。设为 0 时不允许新加入分组，已有分组不变。 |
| `option:usage.rates.*.web_fetch_per_call` | 每次成功网页抓取调用的点数。 |
