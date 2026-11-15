---
group: providers
screens: [/help, /thread/$threadId, /resources/providers/$id, /settings/interface, /topic/$topicId]
keywords: [选项, 字段, 搜索, 参考, options, fields]
heading_id.条目: entries
summary: 随产品提供的请求、界面与提供方选项，以及它们改变什么。
heading_id.实时文件读取: live-file-reads
---

# 选项字段参考

只有接口或程序声明支持的选项才会提供，提供方限制和权限仍然适用。自定义结构中的字段沿用该结构的含义；本参考涵盖随产品提供的声明。设置中的字段见[设置参考](settings-reference.md)。作出改变前先审阅计划，选择运行方式前阅读[程序指南](register-your-program.md#choose-an-arrangement)。

## 条目

| 参考标识 | 含义 |
| --- | --- |
| `option:FilesPlan.source_policy` | 此次工作区读取或复制实际选择的来源域，捕获在已接受审核中，不由界面根据当前路径或环境推断。 |
| `option:FilesPlan.home_policy` | 实际测量的主目录复制采用整个自有数据根策略。不应用工作区排除规则；写入仍受普通调用者与操作权限约束。 |
| `option:FilesPlan.file_states_policy` | 已测量记录数据复制的整个自有数据根策略，与工作区排除规则独立。未捕获策略的旧计划仍显示尚未报告。 |
| `option:UserMessagePayload.native_image_delivery` | 原始已接受命令实际采用的图像传递决策，包括额度、解析时间与安全设置来源。缺失历史记录保持尚未报告；私有上级设置来源保持未共享。 |
| `option:ThreadInput.native_image_delivery` | 排队或已传递输入上的同一图像传递记录。之后更改设置不会重写记录。 |
| `state:NativeImageDelivery.native` | 原图像字节随已接受原生命令传递。 |
| `state:NativeImageDelivery.path` | 已接受输入使用已验证的图像文件路径。仅实际正数字节额度拒绝时显示超出图像额度；零显示图像通过路径传递。 |
| `state:NativeImageDelivery.unavailable` | 没有可用原生字节或已验证文件路径，不表示已通过路径传递。 |
| `option:FileTransferReply.transfer` | 当前环境、机器和代次已宣布的原生清单传输。它与浏览器上传会话无关；读取不会启动原生工作。 |
| `option:FileTransferFacts.expires_at` | 上次接收分片时间加上首个分片采用的空闲期限。原始记录缺失时不推断日期；之后更改设置不会改变此日期或来源。 |
| `option:FileTransferFacts.retention_source` | 捕获时的注册默认值或系统设置来源。清理过期分片后仍保留此记录。 |
| `state:FileTransferState.transferring` | 原生发布者宣布了尚未完成的传输，不表示已成功发布。 |
| `state:FileTransferState.interrupted` | 原生发布者报告了此次传输中断；其记录的到期时间仍适用。 |
| `state:FileTransferState.expired` | 清理确认在记录的空闲到期时间之后移除了未完成传输。 |
| `option:Preview.allowance` | 具体拒绝原因及捕获时的额度、已知测量量和设置来源；未知拒绝原因不推断额度。 |
| `option:Preview.viewer_allowance` | 交互查看器采用的原文件读取额度，与静态解码原文件字节数分开。 |
| `option:Preview.facts` | 在派生内容缓存之后添加已授权的原生来源事实；相同哈希的不同所有者不会共享来源策略或权限。 |
| `state:PreviewAllowanceKind.decoded_source_bytes` | 原文件字节数超过静态解码额度。 |
| `state:PreviewAllowanceKind.pixels` | 原图像素数超过解码或渲染额度。 |
| `state:PreviewAllowanceKind.encoded_preview_bytes` | 最小编码静态预览仍超过字节额度。 |
| `state:PreviewAllowanceKind.viewer_bytes` | 原文件超过交互查看器读取额度；具有普通下载权限时仍可下载。 |
| `option:DirListing.facts` | 随已授权的目录观察捕获的原生来源、适用排除规则与已安装观察器事实。缺失表示尚未报告；打开事实不会扫描目录树。 |
| `option:FileStat.facts` | 此次读取的原生来源策略与适用排除规则。保留清单使用捕获时的策略；未捕获的子目录规则来源显示尚未报告。 |
| `option:FileSourceFacts.policy` | 区分工作区、只读挂载与完整自有数据根。来源可写仍需要读取者具有普通操作权限。 |
| `option:FileSourceFacts.inclusion` | 按由浅到深顺序列出适用的已配置和上级本地 Orch 规则。较深规则优先，排除父目录会阻止遍历；智能体原生访问另行决定。隐藏规则名显示未共享，不披露模式或被省略的子目录。 |
| `option:FileSourceFacts.watch` | 已安装原生观察器的范围、备用刷新间隔与观察标识。之后更改配置不会改变已捕获间隔；保留清单没有实时观察器。 |
| `option:FileSourceFacts.observed_at` | 这些事实的原生捕获时间，与打开披露的时间无关。 |
| `state:FileSourceKind.workspace` | 附加工作区应用 Orch 排除规则与受保护控制文件规则。 |
| `state:FileSourceKind.mount` | 不应用工作区排除规则的只读挂载根。 |
| `state:FileSourceKind.owned_data` | 不应用工作区排除规则、整体复制的自有数据根。 |
| `state:FileRuleKind.configured` | 已配置的 Orch 模式参与决定此路径的纳入规则。 |
| `state:FileRuleKind.local` | 适用本地排除文件参与决定；其名称被排除时隐藏。 |
| `state:FileWatchSource.machine_configuration` | 备用刷新间隔由已安装观察器接受的机器配置提供。 |
| `option:ResourceNotificationQuery.notify` | 仅为此次删除通知已复核的受影响人员；省略或 false 时不通知，个人渠道仍单独选择。 |
| `option:ResourceNotificationQuery.notification_review` | 当前影响预览中绑定给操作者的受众；选择通知时必填，人员变化后需重新复核。 |
| `option:ResourceStopQuery.note` | 可选的单行备注，显示给因停用提供方而等待的线程。 |
| `option:ResourceStopQuery.notify` | 仅为此次停用提供方通知已复核的受影响人员；省略或 false 时不通知。 |
| `option:ResourceStopQuery.notification_review` | 当前影响预览中绑定给操作者的受众；选择通知时必填。 |
| `option:ResourceImpact.notification` | 此次操作当前去重后的受众；通知初始关闭，预览可查看人员列表。 |
| `option:ThreadOutlineGetQuery.selection` | conversation 读取普通大纲，messages 读取人的提示及发送者、所属轮次的更改信息，agents 读取包括较早历史在内的子智能体简要记录。省略时选择 conversation。 |
| `option:ThreadOutlineGetQuery.q` | 在当前所选且可读取的历史中按原文或发送者搜索，再进行分页。匹配词可以位于列表首行之后的提示正文中。 |
| `option:ThreadOutlineGetQuery.include_changes` | 是否读取消息所属整轮已记录的新增、删除行数；messages 选择默认开启。设为 false 时这些可选计数保持未报告，定位消息时不必读取更改。 |
| `option:SearchListQuery.stream` | 在获准搜索的范围内，仅匹配一个对话流（例如 main 或已报告的子智能体流），并在分页之前筛选。 |
| `option:SearchStreamListQuery.stream` | 流式搜索沿用普通搜索的对话流选择，其他对话的匹配项不会占用此结果页。 |
| `option:PersonalState.sidebar.open_links` | 个人的“链接打开方式”，与侧栏状态一同保存：默认自动，也可选择当前视图或新视图。自动时，同一目标区域共用一个视图；当前视图会替换正在查看的页面，新视图会另开页面。目标已经打开时会切换到它。在“界面”或标签菜单中选择；显式复制视图及浏览器的修饰键点击仍遵循原有行为。 |
| `option:TopicSearch.q` | 话题“概览”和“会话”页中的会话名称搜索。它筛选当前可见的实时集合，随话题地址传递，不作为设备默认值保存。 |
| `option:codex.cyber_access_program` | 模型提供、且已在提供方启用的 Codex 访问计划。所选名称会原样传给原生轮次，不限于 Daybreak 系列；模型、账号资格及提供方使用权限仍须满足。 |
| `option:InputScheduleCreateBody.plan` | 每次未来输入的原定 Plan 选择。创建时必填；true 请求规划轮次，false 请求执行工作。发生时保留此选择，普通模式边界需要等待时也不会改成另一模式。 |
| `option:InputScheduleCreateBody.delivery` | 未来输入必填的 queue、steer 或 interrupt 送达选择。保存后在到期时通过普通输入写入流程重新检查；创建计划不会立即发送。 |
| `option:InputScheduleUpdateBody.plan` | 明确修改 Plan 是仅限作者、检查修订号的定义编辑。省略时保留已经审阅的选择。 |
| `option:InputScheduleUpdateBody.delivery` | 明确修改 queue、steer 或 interrupt 是仅限作者、检查修订号的定义编辑。省略时保留已审阅的选择；仅暂停或恢复不会替换它。 |
| `option:ThreadGoalUpdateBody.action` | 暂停、恢复，或接受未经核实的完成报告。Accept 是用户明确选择“标记为达成”；对未经核实的报告执行 Resume 表示“继续工作”，不表示已验证。 |
| `option:AgentProfile.config.native_result_wait_ms` | Codex 可选的保留结果读取等待时间，单位毫秒。省略时采用系统设置默认值，并适配引擎的中断等待窗口。配置中明确给出的有效值会保留，无法适配时说明拒绝原因。 |
| `option:AgentProfile.config.native_result_buffer_bytes` | Codex 读取证据期间按顺序保留后续输入的可选字节预算。省略时使用系统设置默认值。缓冲区压力只结束可选证据读取，不会丢弃输入或停止其他会话。 |
| `option:SessionSettings.web_action_wait_secs` | 传给代理会话、沿所在位置继承并解析后的原生网页操作行等待时间。Codex 使用接收时间计时，并保留归属以接收之后真正到达的结束事件；此等待不会中断轮次。 |
| `option:SessionOpen.web_action_wait_secs` | 连接器打开会话命令中必填的已解析网页操作行等待时间，会传入会话设置及重新打开会话的指纹。 |
| `option:x_turn_end_delays.delays.*.default_setting` | 程序声明的系统设置键，用于提供配置省略时的毫秒等待默认值。引擎解析并适配中断等待窗口；不会缩短配置中明确给出的等待值。 |
| `option:x_turn_end_delays.delays.*.pending_buffer` | 程序声明的证据等待期间可选有序输入缓冲。遇到压力时应结束可选证据读取并继续处理输入，不能丢弃或拒绝输入帧。 |
| `option:x_turn_end_delays.delays.*.pending_buffer.pointer` | 指向程序配置中此字节预算的 JSON 指针。保留明确给出的有效配置值。 |
| `option:x_turn_end_delays.delays.*.pending_buffer.default_bytes` | 程序的字节后备默认值。配置省略该指针时，引擎提供所声明系统设置的已注册默认值。 |
| `option:x_turn_end_delays.delays.*.pending_buffer.default_setting` | 提供待处理输入字节默认值的系统设置键，必须指向可用的非负整数设置。 |
| `option:ask.models.*.model` | 已启用 API 模型的路由名称，与 provider_id 一起标识一个允许的问一问模型。子级列表可以缩小可选范围，并保留父级顺序。 |
| `option:ask.models.*.provider_id` | 此项允许的问一问模型所使用的 API 提供方标识。提问者和当前付费人仍须有权在对话所属组织中使用该提供方及模型。 |
| `option:AccessOptions.purposes` | 申请使用提供方的用途：Ask 或其他工作。仅申请 Ask 不会授权同一模型的普通线程轮次。 |
| `option:ProviderUse.purposes` | 此条目允许 Ask、其他工作或两者。省略时两者都允许；空列表两者都不允许。用途、模型、思考强度和运行位置必须由同一个条目共同允许。 |
| `option:ProviderModelDefaultsQuery.model_name` | 预览提供方所属位置的目录策略下，此确切路由名将使用的默认值。此读取仅使用已保存的事实，不发起发现，也不添加模型。创建请求明确提供 parameters 对象时，按完整定义保存，清除的可选项保持关闭。 |
| `option:Meta.interface_defaults` | 安装级别的最近关闭视图保留数量、加载提示延迟与读取新鲜时间。每个人都可在自己的界面偏好中覆盖这些默认值。 |
| `option:InterfaceDefaults.kept_closed_tabs` | 额外保留的最近关闭视图数量。打开的视图始终保留。 |
| `option:InterfaceDefaults.progress_delay_ms` | 读取开始到显示加载图标之间的毫秒数。 |
| `option:InterfaceDefaults.read_freshness_ms` | 目录、搜索、报表与状态读取的新鲜时间，单位为毫秒。 |
| `option:PersonalState.interface` | 当前登录者的阅读偏好。未给出的字段使用安装默认值；修改偏好会保留其他字段。显示标签页不会修改偏好。 |
| `option:PersonalState.interface.kept_closed_tabs` | 该用户保留的已关闭视图、显示状态及重新打开记录的数量。零仅保留最近一项的重新打开位置。 |
| `option:Workspace.closedLimit` | 该用户设备工作区中最近确认的保留数量。它在界面基线返回前限制关闭记录；下一次确认的原生偏好始终优先。 |
| `option:PersonalState.interface.progress_delay_ms` | 该用户的加载提示延迟，单位为毫秒。零表示立即显示。 |
| `option:PersonalState.interface.read_freshness_ms` | 该用户对目录、搜索、报表与状态读取的新鲜时间覆盖值，单位为毫秒。 |
| `state:PersonalStateKey.interface` | 单独保存界面阅读偏好的原生用户状态键，与命名布局、侧栏选择和快捷键配置分开。 |
| `option:ThreadItemOutputGetQuery.expected_blob` | 读取者选定的精确保留输出文件。若输出已更换，此页会在读取字节前被拒绝，避免混合不同输出的分页。 |
| `option:OutputRef.inline_limit` | 已接受的内联输出字节限额。较小的捕获预算会让实时与持久化末尾同样变小；原始总长度仍计算所有已产生的字节。 |
| `option:OutputRef.retained` | 此输出文件保留的精确原始字节区间；仅保留内联文本时为空。保留的文件可能只包含部分原始输出。 |
| `option:OutputRef.retained.offset` | 保留区间在原始输出中的起始字节位置。此边界之前的字节可能未保留。 |
| `option:OutputRef.retained.len` | 实际保留的文件字节数，不含人为插入的缺失标记。起始位置加长度表示不含在内的原始结束边界。 |
| `option:SessionLost.uncertain` | 原生结果当前不可用，而非确认丢失。已接受的轮次和输入会保留，不会自动重试。 |
| `option:EverythingListQuery.resource_rows` | 返回紧凑的表格行，并在每页中为各个归属位置附带一次可读的共享摘要。打开资源时再读取其完整配置。 |
| `option:EverythingListQuery.resource_details` | 未请求表格行时，返回资源的完整视图。需要展示完整配置的选择器使用此选项。 |
| `option:ThreadOpeningGetQuery.limit` | 此次首屏读取的正数条目数，覆盖解析后的 threads.opening_items 默认值。较早条目仍可通过历史游标继续读取。 |
| `option:ThreadOpeningGetQuery.continuation_id` | 选择此次读取的历史延续，不改变当前执行的延续。 |
| `option:ThreadOpeningGetQuery.projection_revision` | 指定已知的历史投影版本。版本过旧时，返回 sync.complete=false 的原生重置和当前选择器；使用该选择器可重新读取当前窗口。 |
| `option:ThreadModelParameter.options_partial` | 为真表示仅提供首屏参数特征和当前或可见历史轮次的模型。打开控件时，再读取完整参数以取得所有选项、帮助及表单。 |
| `option:ThreadOpening.screen` | 此实际线程独立的活动与计划答复信息。若该部分读取失败，通过 GET /threads/{id}/screen-facts 重试，无须加载模型元数据或整份设置。 |
| `option:ThreadScreenFacts.silent_after_secs` | 生效的 thread.silent_after_secs 秒数，用于运行中线程的静默提示；未由位置覆盖时使用注册表默认值。 |
| `option:ThreadScreenFacts.plan_answered_by` | 生效的 thread.plan_answered_by 策略，可为 writers 或 managers。计划答复启用前读取此策略，与模型选项分开。 |
| `option:Feed.provider_row` | provider-row:{id} 实时流提供与线程首屏相同的紧凑提供方事实，每次送达前重新检查提供方查看权限。不携带模型、来源配置或私有登录进度；提供方页面仍使用完整资源流。 |
| `option:ProviderRowEvent.provider` | changed 事件中的当前线程提供方事实。为 null 时撤回先前事实，表示已失去原生查看权限。 |
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
| `option:protocols.google-interactions.web_search.search_types` | Gemini 原生搜索类型：web_search 返回文本来源，image_search 返回图像来源。这些选项配置已启用的实时搜索工具，并保留调用方的函数工具。 |
| `option:EndpointOptions.auth` | 端点接收凭据的方式。Gemini Interactions 默认使用 x-goog-api-key；其他端点仍可选择 bearer、x-api-key、指定请求头或不使用凭据。 |
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
| `option:AgentDescriptor.features` | 程序声明的功能标识及初始开关状态；agent.features 中的明确设置会覆盖这些值，包括 false。 |
| `option:ProviderModel.parameter_forms` | 只读的请求选项表单，按接口索引，并由模型的有效约束投影生成；线程和位置默认参数编辑器使用同一份表单。 |
| `option:AgentModel.parameter_forms` | 代理模型条目附带的有效请求选项表单；是否适用仍由所选程序和模型决定。 |
| `option:SettingParameterFormBody.definitions` | 所选程序描述中的原生参数声明；仅解析表单元数据，不会打开会话或调用模型。 |
| `option:SettingParameterFormBody.model` | 所选模型已开放的参数及有效原生约束；仅由发现列出的候选项不会自动启用。 |
| `option:SettingParameterFormBody.feature_defaults` | 所选程序描述中声明的初始功能状态，随后叠加位置的明确选择。 |
| `option:SettingParameterFormBody.features` | 位置解析后的明确功能选择；每项覆盖程序的初始值，false 表示保持关闭。 |
| `option:SettingParameterFormBody.provider_kind` | 所选提供方类型 api 或 subscription，用于评估声明的参数适用条件。 |
| `option:SettingParameterFormBody.scope` | 可选的 org、project、topic 或 thread 范围；表单只包含可在该范围设置的参数。 |
| `option:SettingParameterFormBody.harness_version` | 已知时填写所选程序报告的版本，用于评估依赖版本的参数条件。 |
| `option:ResolvedSetting.inherited` | 在省略此位置自身贡献后生效的值及其来源；读取者具有位置上下文时提供。保存的选择仍然是自身贡献。 |
| `option:SettingInheritance.value` | 继承选项给出的值；null 表示没有任何范围提供该值。它由相同的原生合并和默认规则计算。 |
| `option:SettingInheritance.source` | 提供继承值的位置或声明的默认来源；隐藏名称仍遵守现有隐私边界。 |
| `option:ThreadInterruptBody.turn_id` | 要停止的确切轮次。轮次尚未创建时，省略此字段可停止下一条消息的准备过程。停止操作不需要队列或历史修订号。 |
| `option:ThreadStop.state` | 停止请求的即时确认：仍有工作等待结束时为 stopping，目标已结束时为 stopped。已接受的请求会在响应后继续执行。 |
| `option:ThreadStop.requested_at` | 发出停止请求的时间；停止耗时从此时起计算，包括等待其他变更完成的时间。 |
| `option:ThreadStop.turn` | 确认响应中所指定轮次的当前记录。停止尚未创建轮次的准备过程时不提供此字段。 |
| `option:Thread.stopping_since` | 最早一个未完成停止请求的开始时间。没有未完成请求时省略；停止过程中线程仍可能显示当前运行或准备阶段。 |
| `option:Turn.interrupted_at` | 有人要求停止此轮次的时间。无人停止时省略；它与最终完成时间分别记录。 |
| `option:MarkerSessionReopened.reason` | 轮次开始前重新打开会话的已记录原因，例如指令变化或程序重启。可观察的指令文件没有变化时沿用原会话。 |


### 实时文件读取

| 参考标识 | 含义 |
| --- | --- |
| `option:Thread.starting_since` | 本次准备开始的时间；未在启动时省略。 |
| `option:Thread.starting_work` | 当前准备工作、对应命令和正在前方执行的工作。 |
| `option:StartingWork.command` | 本次准备已接受的原生命令，包括正在执行或排队的命令。 |
| `option:StartingWork.behind` | 实际占据前方执行通道的工作；已完成捕获后的上传不会占住后续启动。 |
| `option:Turn.files_changed` | 从完整前后状态或原生变更日志得到的本轮更改路径数量；省略表示未知。 |
| `option:Turn.files_changed_paths` | 本轮已知的相对更改路径，也包括没有文件工具补丁的更改。 |
| `option:Turn.files_changed_complete` | true 表示完整改动覆盖；false 表示范围内的已知改动，并保留工具另外报告的编辑。 |
| `option:WorkspaceObserveResult.changed_paths_complete` | 原生监视是否完整覆盖该观测区间；覆盖降级时仍返回实际观察到的路径。 |
| `option:WorkspaceObserve.sealed` | 省略时冻结已准备的结束元数据与观测路径；传入该确切结果后发布，且不读取更新的实时文件。 |
| `option:WorkspaceObserveResult.end_patch_blob` | 基于已准备基底的不可变改动元数据；完整清单在后台编码与发布，同时保留完整的结束状态覆盖。 |
| `state:RecordKind.source` | 任务所有的“当前文件”捕获，独立于自动录制天数与字节策略，并由原生工作树恢复日志保留。 |
| `option:Turn.recording_notice` | 首个受影响轮次保留文件记录覆盖原因；后续刷新不会把提示移到其他轮次。 |
| `option:Turn.recording_indexing` | 本轮在整体记录就绪前启动时的索引进度；缺失的进度不会被猜测。 |
| `option:TurnRecordingProgress.done` | 轮次启动时已准备的条目数。 |
| `option:TurnRecordingProgress.total` | 该准备过程的条目总数；仅在已知时显示百分比。 |
| `option:EnvironmentFileSearchQuery.cursor` | 继续同一名称或内容查询、来源和筛选条件；省略则重新开始。 |
| `option:FileSearchNames.entries` | 本页匹配路径的观察结果；结果旁会说明来源。 |
| `option:FileSearchNames.next_cursor` | 同一遍历的下一部分；空值表示已到末尾。 |
| `option:FileSearchNames.source` | 来源为实时遍历，或上次完整缓存的文件索引。 |
| `option:FileSearchNames.searched` | 目前已检查的条目数，不代表整个目录只有这些条目。 |
| `option:FileSearchNames.as_of` | 本页观察结束时间，或缓存清单的时间。 |
| `option:FileSearchNames.observed_from` | 实时观察开始的时间；缓存结果保留自身的观察时间。 |
| `option:FileSearchContent.next_cursor` | 在本窗口后继续内容搜索，也可继续尚未找到匹配的时间片。 |
| `option:FileSearchContent.observed_from` | 本次实时内容遍历开始的时间。 |
| `option:EnvironmentArchiveListQuery.cursor` | 继续同一已打开归档和文件夹，不会反复读取压缩数据的前缀。 |
| `option:EnvironmentArchiveListQuery.q` | 不区分大小写地匹配所选归档文件夹中的子项名称。 |
| `option:ArchiveListing.next_cursor` | 继续同一归档读取器；时间片可以尚无条目，但仍能继续。 |
| `option:EnvironmentGitDiffGetQuery.cursor` | 继续同一原生 Git 进程的输出；省略则重新读取差异。 |
| `option:EnvironmentGitDiffGetQuery.max_bytes` | 本页差异的期望字节数；省略时使用工作区默认值，同时遵守传输页面大小。 |
| `option:DocumentFileArchiveListQuery.cursor` | 继续当前仍有权读取的不可变文档归档。 |
| `option:DocumentFileArchiveListQuery.q` | 按子项名称筛选所选归档文件夹。 |
| `option:UploadBlobGetQuery.cursor` | 仅在调用者再次独立持有来源数据的读取权后继续归档页面。 |
| `option:UploadBlobGetQuery.q` | 列出草稿归档时筛选子项名称。 |
| `option:UploadBlobGetQuery.limit` | 本次归档窗口的行数；省略时使用当前归档行数默认值。 |
| `option:EnvAttach.workspace` | 本次连接解析后的文件读取默认值；不会因此使大型目录不可用。 |
| `option:FsRecord.wait_for_indexing` | 主动选择等待时也等待索引；轮次默认会跳过首次索引等待。 |
| `option:FsRecordResult.indexing` | 整体记录尚未就绪时的当前准备进度。 |
| `option:FsRecordResult.reason` | 整体记录不可用的原因；cancelled 表示被停止，恢复输入后会重新请求。 |
| `option:FsSearch.scope` | 名称或文本内容，两者都支持实时续读。 |
| `option:FsSearch.cursor` | 继续完全相同的实时查询和连接。 |
| `option:FsArchiveList.cursor` | 继续同一已打开的归档遍历。 |
| `option:FsArchiveList.query` | 不区分大小写地匹配所显示子项名称的子串。 |
| `option:GitDiff.cursor` | 继续同一进程的输出，不会重新执行已经读取的前缀。 |
| `option:GitDiff.max_bytes` | 在现有传输表示范围内，下一页期望的字节数。 |
| `option:CmdAck.key_phase` | 实际的排队、运行或释放状态；仅被接受并不会暂停记录的等待计时。 |
| `option:EnvironmentFacts.recording` | 当前整体记录的覆盖情况；不完整覆盖不会被报告为完整状态。 |
| `option:EnvironmentFacts.exclusion_problem` | 无法应用的排除规则来源、已知行号及安全原因；不会披露规则内容。 |
| `option:ExclusionProblem.path` | 需要修正的相对排除规则来源路径，或配置规则的名称。 |
| `option:ExclusionProblem.line` | 已知时显示从一开始计数的无效行号。 |
| `option:ExclusionProblem.reason` | 安全原因，例如无法读取、链接、过大、编码无效或模式无效。 |
| `option:WorkspaceLimits.directory_entries` | 本次连接解析后的 workspace.directory_entries 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.search_results` | 本次连接解析后的 workspace.search_results 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.archive_results` | 本次连接解析后的 workspace.archive_results 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.index_entries` | 本次连接解析后的 workspace.index_entries 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.index_scan_secs` | 本次连接解析后的 workspace.index_scan_secs 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.ignore_max_bytes` | 本次连接解析后的 workspace.ignore_max_bytes 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.ignore_cache_bytes` | 本次连接解析后的 workspace.ignore_cache_bytes 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.directory_idle_secs` | 本次连接解析后的 workspace.directory_idle_secs 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.directory_slots` | 本次连接解析后的 workspace.directory_slots 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.directory_cache_bytes` | 本次连接解析后的 workspace.directory_cache_bytes 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.directory_builds` | 本次连接解析后的 workspace.directory_builds 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.directory_page_bytes` | 本次连接解析后的 workspace.directory_page_bytes 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.search_slice_secs` | 本次连接解析后的 workspace.search_slice_secs 默认值；显式请求和续读保留对应操作。 |
| `option:WorkspaceLimits.git_diff_bytes` | 本次连接解析后的 workspace.git_diff_bytes 默认值；显式请求和续读保留对应操作。 |
| `option:NodeSettingSetBody.model_choice` | 仅用于线程的 agent.model 写入：已核对的投入级别和参数清理与模型一起提交；任一自有版本已变更时整体拒绝。 |
| `option:WithheldParameter.requirement` | 实际未满足的公开条件及声明的替代条件，不暴露私有配置表达式。 |
| `option:WithheldParameter.inapplicable_selected_value` | 当前未应用的已保存选择；显式空值与未保存选择保持区分。 |
| `option:ParameterRequirement.kind` | 所有、任一、排除、凭据路径、提供方种类、放置位置、模型能力或选项、程序版本、代理支持、功能启用或提供方规则。 |
| `option:ParameterRequirement.requirements` | 需要满足此组的所有声明条件。 |
| `option:ParameterRequirement.alternatives` | 满足任一声明的替代条件即可；不会根据宽泛的拒绝类别推测替代条件。 |
| `option:ParameterRequirement.requirement` | 需要不成立的类型化条件。 |
| `option:ParameterRequirement.path` | 程序声明的原生、转发、共享或移交模型凭据路径。 |
| `option:ParameterRequirement.provider_kind` | 声明要求的 API 密钥或订阅提供方种类。 |
| `option:ParameterRequirement.placement` | 声明要求的托管或已连接环境种类。 |
| `option:ParameterRequirement.feature` | 此参数需要的已声明模型能力。 |
| `option:ParameterRequirement.list` | 声明要求的投入级别、速度或访问计划列表。 |
| `option:ParameterRequirement.value` | 需要的已声明模型列表选项。 |
| `option:ParameterRequirement.range` | 声明要求的公开代理程序版本范围。 |
| `option:ThreadParameters.approval_choices` | 此代理实际声明的审批方式，包括当前不可用的方式；普通写入仍重新检查所有条件。 |
| `option:ApprovalModeChoice.mode` | 原生声明的审批方式；不会虚构未构建且未声明的方式。 |
| `option:ApprovalModeChoice.available` | 策略、审核模型使用权及实际隔离当前是否允许此选择。 |
| `option:ApprovalModeChoice.reason` | 实际已知的审核模型、策略、隔离或构建拒绝类别。 |
| `option:ApprovalModeChoice.requirement` | 缺少的原生审核模型、模型使用权、策略允许、隔离或构建支持。 |
| `option:ToolError.rate_limit` | 工具限流判定时记录的额度、来源及当时需等待的时间；普通错误不包含此项。 |
| `option:ToolPayload.rate_limit` | 通过原生工具结果传递的类型化拒绝事实；缺失不代表零值，也不承诺当前会重试。 |
| `option:ToolRateLimit.allowed_calls` | 此次拒绝发生时生效的单线程调用额度。 |
| `option:ToolRateLimit.period_secs` | 已记录额度对应的周期秒数。 |
| `option:ToolRateLimit.retry_at` | 当时报告的可重试时刻；仅无法表示时缺失，不会新增自动重试。 |
| `option:ToolRateLimit.observed_at` | 记录拒绝的时间；之后修改设置或重启限流器不会改写此记录。 |
| `option:ToolRateLimit.source` | 提供该额度的设置来源标识；其当前名称仍受当前读取者权限约束。 |
| `option:ProviderRefreshReply.trial` | 实际恢复检查的结果记录；不会根据凭据错误状态推测已预留试运行。 |
| `option:ProviderTrial.state` | 已预留、原生正在运行、已使用或未预留；没有记录时明确表示尚未报告。 |
| `option:ProviderTrial.at` | 该状态的记录时间；预留不代表登录成功，也不承诺启动。 |
| `option:ProviderTrial.input_ref` | 确切预留的输入，仅在当前可查看其实时会话和输入时提供。 |
| `option:ProviderTrial.thread_ref` | 当前有权查看的会话引用；仅允许读取提供方的密钥不会获得此信息。 |
| `option:ProviderTrial.reserved_for` | 预留时记录的负责人，仅在当前可读取原输入时显示。 |
| `state:ProviderTrialState.reserved` | 一个确切输入持有这次唯一试运行的预留。 |
| `state:ProviderTrialState.running` | 预留的原生轮次确实正在运行。 |
| `state:ProviderTrialState.consumed` | 已接纳的试运行已结束，或确认恢复后其预留已使用；不表示服务方一定接受。 |
| `state:ProviderTrialState.not_reserved` | 此次观察到的检查没有活动的试运行预留。 |
| `option:ThreadInput.delivery_resolution` | 仅在首次排入此输入时记录的原始公告回退决定。 |
| `option:AnnouncementPayload.delivery_resolution` | 公告结果保留的同一输入决定；不会根据之后的设置重建。 |
| `option:AnnouncementPayload.delivered_as` | 接收方实际排队、即时引导、中断或延后通知的结果，与请求方式分开。 |
| `option:InputDeliveryResolution.requested` | 原始请求的公告送达方式。 |
| `option:InputDeliveryResolution.applied` | 实际应用的排队、即时引导或中断后发送方式。 |
| `option:InputDeliveryResolution.reason` | 原接收方无法接收即时引导时，原因为即时引导不可用。 |
| `option:InputDeliveryResolution.source` | 记录的回退设置来源；显式为公告选择时为空；来源名称需要当前访问权。 |
| `option:InputDeliveryResolution.resolved_at` | 作出原始送达决定的时间。 |
| `option:ThreadBinding.call_route` | 当前已接纳的故障切换路径及返回上下文；未报告时缺失，包括保留记录检查。 |
| `option:ThreadCallRoute.configured` | 用户配置的路径，与临时覆盖分开；旧记录中未知的原路径保持尚未报告。 |
| `option:ThreadCallRoute.active_override` | 实际已接纳的覆盖路径；用户更改绑定或选择后失效。 |
| `option:ThreadCallRoute.original` | 记录的原始路径，不含其私有参数。 |
| `option:ThreadCallRoute.return_policy` | 当前就绪后下一轮返回或继续使用覆盖路径的策略；下次调用时重新检查。 |
| `option:ThreadCallRoute.return_state` | 实际正在返回的状态或当前策略；不会根据等待中的候选模型推测已就绪。 |
| `option:ThreadCallRoute.observed_at` | 观察当前路径事实的时间。 |
| `option:CallRouteChoice.provider_id` | 已接纳路径的提供方标识，不包含提供方配置或凭据。 |
| `option:CallRouteChoice.model` | 已接纳模型的路由名称；普通屏幕使用其安全资源名称。 |
| `option:CallRouteChoice.display_name` | 接纳路径时记录的显示名称；旧记录中可能尚未报告。 |
| `state:CallReturnState.when_ready` | 就绪后在后续符合条件的轮次返回，不承诺启动。 |
| `state:CallReturnState.stay_on_failover` | 继续使用已接纳的故障切换路径，直至普通更改。 |
| `state:CallReturnState.returning` | 引擎确已开始返回原路径。 |
| `state:CallReturnState.unreported` | 未记录原路径或返回事实。 |
| `option:ModelChoiceSettings.effort` | 与模型同时提交已变更且可写的投入级别；省略时保留未变更或独立锁定的投入级别。 |
| `option:ModelChoiceSettings.params` | 移除不兼容选项后的已核对自有参数映射，在同一事务中提交。 |
| `option:ModelChoiceSetting.value` | 相关设置的显式值；null 表示使用模型的默认投入级别。 |
| `option:ModelChoiceSetting.expected_revision` | 界面读取到的自有版本；零要求没有自有记录。任一部分过期都会拒绝整个模型选择。 |
| `option:ProviderModelUpdateBody.expected` | 打开草稿时读取的定义；编辑器会提交它，基线已变更时拒绝整个保存。省略表示有意进行完整替换，不要求事先读取。 |
| `option:ProviderModelState.parameters` | 草稿所依据的已启用模型选项，在模型记录锁内比较。 |
| `option:ProviderModelState.groups` | 草稿所依据的有序模型分组。 |
| `option:BindingDefaults.locks` | 每项创建默认值分别锁定；固定一项不会锁住无关选项。 |
| `option:BindingDefaultLocks.provider` | 默认提供方是否固定；其中的模型仍可独立选择。 |
| `option:BindingDefaultLocks.agent` | 默认代理配置是否固定。 |
| `option:BindingDefaultLocks.model` | 模型路由名称是否固定，与投入级别独立。 |
| `option:BindingDefaultLocks.effort` | 投入级别是否固定；兼容模型仍可选择。 |

| `option:BindingAgent.approval_resolution` | 启动提案中的审批首选方式及声明的实际替代方式；程序未报告方式时缺省。 |
| `option:ApprovalDefaultResolution.preferred` | 有效的继承审批首选方式。 |
| `option:ApprovalDefaultResolution.effective` | 按创建时相同规则选择的程序方式；没有声明常规替代方式时为 null。 |
| `option:ApprovalDefaultResolution.reason` | 继承的首选方式需要替换时的原因。 |
| `option:ApprovalDefaultResolution.source` | 首选方式的受保护设置来源。 |
| `option:ApprovalDefaultResolution.revision` | 标识所观察到的首选方式、来源和声明方式；创建仍检查当前权限及输入。 |
| `state:ApprovalDefaultReason.not_declared` | 程序未声明首选方式。 |
| `state:ApprovalDefaultReason.no_ordinary_mode` | 程序未声明常规替代方式。 |

| `option:ParameterRequirement.actual` | 相同资格检查时观察到的安全凭据、提供方或放置类别，或已报告的程序版本；缺省表示尚未报告。 |
| `option:ThreadParameters.orch_tools` | 当前原生会话已接受的工具列表简要信息；当前设置不会改写它。 |
| `option:OrchToolsSummary.session_revision` | 标识确切的已接受目录和当时输入。 |
| `option:OrchToolsSummary.offered_count` | 实际提供的数量，包括测得的零。 |
| `option:OrchToolsSummary.accepted_at` | 接受工具列表的时间。 |
| `option:OrchToolFact.key` | 声明的原生工具键。 |
| `option:OrchToolFact.offered` | 已接受的会话是否获得此工具。 |
| `option:OrchToolFact.reason` | 当时的构建、设置、续接、目标或能力排除原因。 |
| `option:OrchToolFact.source` | 提供设置的来源，仅在当前设置读取权限允许时返回。 |
| `option:OrchToolsQuery.cursor` | 同一已接受会话工具列表内的原生续页游标。 |
| `option:OrchToolsQuery.search` | 在保留目录中筛选声明的工具名称。 |
| `option:OrchToolsQuery.session_revision` | 要求显示的已接受列表版本；会话改变后须重新读取。 |
| `option:OrchToolsPage.summary` | 实际会话简要信息；未捕获工具列表时为 null。 |
| `option:OrchToolsPage.tools` | 已提供和已排除声明的一个原生页面；按续页游标继续读取。 |
| `option:NativeImageReceipt.native_image_bytes` | 准入时有效的原生图像字节上限；零表示使用物化路径。 |
| `option:NativeImageReceipt.source` | 当时额度的来源；名称仍受当前权限保护。 |
| `option:NativeImageReceipt.resolved_at` | 解析器实际观察的时间。 |
| `state:OrchToolUnavailable.not_built` | 此引擎未实现该声明。 |
| `state:OrchToolUnavailable.off_here` | 此处未开启该工具。 |
| `state:OrchToolUnavailable.read_only` | 此续接仅允许读取工具。 |
| `state:OrchToolUnavailable.no_tools` | 此续接不接收工具。 |
| `state:OrchToolUnavailable.goal_required` | 没有活动目标提供目标工具。 |
| `state:OrchToolUnavailable.capability_unavailable` | 原生能力检查未提供此工具。 |
| `state:OrchToolUnavailable.program_unsupported` | 程序不接受 Orch 工具。 |
| `state:OrchToolUnavailable.environment_tools_off` | 环境关闭了 Orch 工具。 |
| `state:OrchToolUnavailable.capability_work` | 此能力任务续接不接收 Orch 工具。 |
| `option:DirListing.revision` | 本次文件夹观察的标识；稳定的小文件夹使用不可变快照，宽大或持续变化的文件夹则继续同一原生读取器。每页在自身窗口内排序，并说明观察时间范围。 |
| `option:FsListPageResult.revision` | 按字节分页时保留同一原生文件夹观察标识，不代表整个文件夹在某一瞬间的完整切面。 |
