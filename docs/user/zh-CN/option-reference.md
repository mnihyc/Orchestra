---
group: providers
screens: [/help, /thread/$threadId, /resources/providers/$id, /settings/interface, /topic/$topicId]
keywords: [选项, 字段, 搜索, 参考, options, fields]
heading_id.条目: entries
heading_id.提供方操作事实: captured-provider-operation-facts
heading_id.已记录的放置计算: recorded-placement-calculation
heading_id.原生审批检查: native-approval-checks
summary: 随产品提供的请求、界面与提供方选项，以及它们改变什么。
heading_id.资源池目标: resource-pool-targets
heading_id.实时文件读取: live-file-reads
---

# 选项字段参考

只有接口或程序声明支持的选项才会提供，提供方限制和权限仍然适用。自定义结构中的字段沿用该结构的含义；本参考涵盖随产品提供的声明。设置中的字段见[设置参考](settings-reference.md)。作出改变前先审阅计划，选择运行方式前阅读[程序指南](register-your-program.md#choose-an-arrangement)。

## 条目

提供方的 revision 表示可编辑配置的版本。事实刷新、运行状态和来源描述的学习不会让该版本过期；并发的配置编辑仍会使显式提交的旧版本失效。

| 参考标识 | 含义 |
| --- | --- |
| `option:ThreadOutlineGetQuery.q` | 在原有投影的大纲标题、目标条件或轮次编号中搜索，再填充结果页；保留原有历史归属与冻结文本。 对消息和子代理列表则按完整原文或发送者搜索。 |
| `option:ThreadOutlineGetQuery.kinds` | 用逗号分隔的大纲种类，在分页前筛选；省略时包含原有的全部种类。 |
| `option:ThreadHistoryListQuery.q` | 在已投影的会话段代理、提供方名称和编号，或轮次的模型、状态和编号中搜索，再填充结果页。 |
| `option:ThreadRawListQuery.q` | 在按当前规则遮盖后的协议帧内容中搜索；隐藏的原始秘密不会参与匹配。 |
| `option:ThreadRawListQuery.direction` | 先按传输方向筛选，再填充匹配的协议帧页。 |
| `option:ThreadRawListReply.next_before` | 从此原始事件编号向前继续，也适用于没有匹配行的搜索进度页；没有更早页时省略。 |
| `option:ModelInputListQuery.q` | 在原有来源权限内，对块名称、来源标签和已保存预览先搜索再分页。此查找不搜索尚未打开的完整文本；“阅读完整文本”和复制保留全部字节。 |
| `option:NotificationListQuery.q` | 对已投影的通知标题、正文及当前可见的地点、人员文字搜索，再填充结果页。按地点分组保留完整分组的已读及撤销计数；续页绑定本次搜索。 |
| `option:AdminProgramBuildListQuery.q` | 在每个原有构建来源中，先匹配版本、操作系统、架构与生效检查状态的文字，再应用分页数量。 |
| `option:AdminSearch.buildQ` | 管理页面地址中的构建列表搜索。修改搜索会清除此列表的续页位置；其他程序筛选保持独立。 |
| `option:StorageQuery.q` | 在原有存储权限与名称投影之后，按可见的记录、原生句柄、地点、主机、所有者及依赖标签搜索，再计算汇总和分页；隐藏的主机名称不会参与匹配。 |
| `option:StorageQuery.limit` | 每页记录数；默认五十，可传任意正的 u32 数值。工作缓冲不限制完整清单的条目总数。 |
| `option:StorageQuery.cursor` | 继续读取相同原生筛选下的保留记录。 |
| `option:PkiInfo.machines_next_cursor` | 在相同管理员、机器名称搜索与固定证书到期窗口内继续读取。报告显示一页机器，同时保留三十天内到期的完整计数。 |
| `option:HistoryListQuery.q` | 匹配当前获准呈现的历史文本、动作、人员和已披露的线程或路径。在原生投影后、填充结果页前筛选；隐藏的原始详情不会匹配。 使用 upload 时，先匹配文件名及路径再分页，并保留原始不可变的位置。 |
| `option:HistoryListQuery.kinds` | 环境历史使用原有 HistoryKind 值；节点和其他资源历史使用准确的 AuditKind 分类，包括当前页尚未出现的类型。在限制结果数量与汇总分组前筛选。 |
| `option:DocumentRelatedListQuery.q` | 在填充排名页之前匹配当前可读条目的名称。保留原有关联分数、原因和可见条件。 |
| `option:DocumentRelatedListQuery.limit` | 每页条目数，接受任意正 u32；省略时采用场所已有的关联数量默认值。 |
| `option:DocumentRelatedListQuery.cursor` | 延续同一来源、人员、搜索词和可见排名结果。来源或搜索变更时从头读取。 |
| `option:DocumentReferencesListQuery.q` | 匹配可读来源或目标名称、上下文及文档中已写出的标签或路径。指定 of 时搜索该来源全部提及的已呈现文本。无权读取的目标名称不会匹配。 |
| `option:DocumentReferencesListQuery.limit` | 每页来源、目标或指定 of 后匹配的提及数，接受任意正 u32。默认值来自场所已有的链接分页和提及设置。访问检查工作预算耗尽时提供续页，不限制列表总量。 |
| `option:NodeTagListQuery.q` | 在分页前匹配可见标签名称中的词语，精确名称优先于部分匹配。 |
| `option:NodeTagListQuery.picker` | 选择器空输入时，优先显示此人最近使用且当前可用的标签定义。其他读取保持原有定义场所顺序。 |
| `option:NodeTagListQuery.limit` | 每页标签定义数，默认五十，接受任意正 u32。 |
| `option:NodeTagListQuery.cursor` | 延续同一人员、场所、搜索词和个人顺序。顺序改变时从头读取。 |
| `option:HolderSearchQuery.picker` | 打开人员或主体选择器时，优先提供此人最近使用的人员和群组；从首个字符搜索，保留原有场所定位语法。 |
| `option:HolderSearchQuery.kinds` | 逗号分隔的主体类型，原生筛选在填充结果页前执行。省略时包含全部类型。 |
| `option:HolderSearchQuery.limit` | 每页主体数，默认五十，接受任意正 u32，不限制可发现的人员和群组总数。 |
| `option:HolderSearchQuery.cursor` | 延续同一人员、来源场所、组织树、主体类型和精确定位搜索。可见来源变更时从头读取。 |
| `option:HolderSearchReply.next_cursor` | 同一可见来源的原生续页游标，到达末尾时省略。 |
| `option:SemanticSearchOverview.orgs_next_cursor` | 继续同一管理员和根组织名称搜索。先确定可显示的组织并匹配名称，再分页；仅为本页读取实时索引详情。 |
| `option:CapabilityListQuery.q` | 在分页前匹配可见定义名称、键或说明中的字面词语。 |
| `option:CapabilityListQuery.limit` | 每页权限定义默认 50，可设为任意正 u32。 |
| `option:CapabilityListQuery.cursor` | 继续同一管理员、搜索词和公共别名键选择。 |
| `option:CapabilityListQuery.aliases` | 以逗号分隔的公共权限项键，对应同时匹配其内置本地化名称或说明的条目。在分页前补充存储文本搜索，不会绕过系统管理员权限。 |
| `option:MePickerRecentReceipt.count` | 写入回执中的当前系统分组长度。仅写权限的操作不会返回保存的私有历史。 |
| `option:MePickerRecentReceipt.remembered` | 此次选择已记住时为 true；关闭记忆时为 false。关闭时不记录此次选择，也不删除先前偏好。 |
| `option:UserSearchQuery.include_external` | 已选组织时，也提供组织外原有的共同组织成员与精确公开身份搜索。默认关闭；分页前仍检查当前组织查看权限和账户范围。 |
| `option:UserSearchReply.org_member_ids` | include_external 为 true 时，给出本页结果中实际属于所选组织的人员 ID。其他情况省略；选择器不会根据已加载页面猜测访客身份。 |
| `option:UserSearchQuery.picker` | 选择器从首个字符开始筛选；空输入提供此人最近使用的活跃账户及共同组织的人。指定组织仍会限制成员范围；分页前检查当前账户作用域。 |
| `option:UserSearchQuery.limit` | 每页人数默认 20，可设为任意正 u32；不限制能找到的总人数。 |
| `option:UserSearchQuery.cursor` | 继续同一操作者、组织、选择模式及字面搜索。打开时的顺序改变后重新读取。 |
| `option:MePickerRecentChoice.kind` | 原生资源类型或 model。同一类型的所有页面共用此人的私有历史。 |
| `option:MePickerRecentChoice.key` | 稳定的不透明选项键；模型选项包含提供者。只记录主动选择，不记录权限或名称。 |
| `option:MePickerRecents.count` | 每个类型记住并显示的条目数，来自系统设置；零表示不再记住。 |
| `option:MePickerRecents.choices` | 按类型返回此人最近选择的去重键，一次读取。可用选项仍由当前原生选择器决定。 |
| `option:PersonalStateKey.picker_recents` | 所有选择器共用的私有服务器偏好。POST /me/picker-recents 原子更新一个类型的选项；向 PUT /me/state/picker_recents 提交空对象可清空。 |
| `option:NameSearchListQuery.picker` | 选择模式：空输入提供所有当前可见的名称，最近选择排在前面；输入首个字符即可在服务器筛选。 |
| `option:NameSearchListQuery.cursor` | 继续同一操作者、范围、类型、参与和收藏条件及搜索。最近使用的顺序改变后重新读取。 |
| `option:NameSearchPage.next_cursor` | 继续读取当前授权名称的下一页；没有更多时省略。 |
| `option:ProviderModelSuggestionListQuery.refresh` | 为 true 时显式重新读取源；否则分页和搜索使用最近成功读取的目录，尚未读取的源会加载一次。也可使用提供方的“刷新”。 |
| `option:ProviderModelSuggestionListQuery.q` | 按源建议的可见模型名称搜索，在分页之前匹配。 |
| `option:ProviderModelSuggestionListQuery.limit` | 本页建议的条目数，默认 50；可继续读取后续各页。 |
| `option:ProviderModelSuggestionListQuery.cursor` | 同一提供方、搜索词及 unlisted 选择返回的 next_cursor。 |
| `option:ProviderModelSuggestionListQuery.unlisted` | 为 true 时，在分页之前排除提供方中已保存的模型名称。为 false 或省略时仍可读取完整发现目录。 |
| `option:AdminRuntimeListQuery.q` | 在分页前按可见的单元、资源或模板名称搜索。 |
| `option:AdminRuntimeListQuery.limit` | 每页条目数；默认 50，可设为任意正的 u32。 |
| `option:AdminRuntimeListQuery.cursor` | 继续读取相同主机、类型和名称搜索的结果。 |
| `option:EverythingListQuery.resource_kind` | 按模板的单元类型、环境的机器或模板来源，或资源池的模型、提供方、环境、主机类型筛选。用 system 选择没有归属位置的代理配置。筛选在名称搜索和分页之前执行。 |
| `option:NativeSessionListQuery.q` | 在已读取的原生会话目录中，按标题或文件夹中的词搜索，再分页。 |
| `option:NativeSessionListQuery.limit` | 每页条目数；默认 50，可设为任意正的 u32。 |
| `option:NativeSessionListQuery.cursor` | 继续读取相同来源和搜索的结果，不能借此选择其他目录。 |
| `option:NativeSessionListQuery.source` | 第一页返回的来源引用；省略时重新读取原生程序。引用绑定调用者、提供方、配置和凭据代次；过期或来源变化时返回字段错误，界面重新开始这次读取。 |
| `option:InvitationListQuery.active` | 为 true 时，在分页前排除过期、撤回及用尽的链接；false 或省略时仍可读取有权查看的完整历史。 |
| `option:InvitationListQuery.q` | 按可见的目标位置、邮箱地址、备注或邀请者中的词搜索。 |
| `option:InvitationListQuery.limit` | 每页条目数；默认 50，可设为任意正的 u32。 |
| `option:InvitationListQuery.cursor` | 继续读取相同组织、有效性选择和搜索的结果。 |
| `option:NotificationChannelListQuery.q` | 分页前按可见的渠道名称或通知器类型搜索。 |
| `option:NotificationChannelListQuery.limit` | 每页条目数；默认 50，可设为任意正的 u32。 |
| `option:NotificationChannelListQuery.cursor` | 继续读取相同归属范围和搜索的结果。 |
| `option:EnvironmentMcpGetQuery.thread_id` | 读取此线程当前环境中原生程序的 MCP 连接观测。调用者还须拥有该线程的查看权限；省略时只显示发现的配置，不借用其他线程的运行状态。 |
| `option:EnvironmentMcpGetQuery.collection` | 原生目录：默认是 servers 连接，或 found 发现的定义。 |
| `option:EnvironmentMcpGetQuery.q` | 在选定目录的可见名称、程序或目录路径中搜索词语。 |
| `option:EnvironmentMcpGetQuery.limit` | 每页条目数；默认 50，可用任意正 u32。 |
| `option:EnvironmentMcpGetQuery.cursor` | 继续同一环境、线程选择、目录和搜索。 |
| `option:HostRuntimeListQuery.q` | 在可读的环境或提供者名称、驱动或状态中搜索词语。 |
| `option:HostRuntimeListQuery.limit` | 每页条目数；默认 50，可用任意正 u32。 |
| `option:HostRuntimeListQuery.cursor` | 继续同一主机、类型和搜索。 |
| `option:MachineMountListQuery.collection` | 默认 locations 位置、available 可添加项，或 mount_id 所挂接的 environments。 |
| `option:MachineMountListQuery.mount_id` | 当 collection=environments 时，选择要读取其挂接环境的已有挂载。 |
| `option:MachineMountListQuery.q` | 在可见挂载名称、路径或挂接的环境名称中搜索词语。 |
| `option:MachineMountListQuery.limit` | 每页条目数；默认 50，可用任意正 u32。 |
| `option:MachineMountListQuery.cursor` | 继续同一机器、集合、挂载和搜索。 |
| `option:NotificationRouteListQuery.retained` | true 只保留渠道已移除的路由，false 只保留当前路由；省略则列出所有已保存的路由。 |
| `option:NotificationRouteListQuery.q` | 分页前，在安全的渠道名称、类型或连接标签中搜索词语。 |
| `option:NotificationRouteListQuery.limit` | 每页条目数；默认 50，可用任意正 u32。 |
| `option:NotificationRouteListQuery.cursor` | 继续同一用户、保留项选择和搜索。 |
| `option:NodeChildListQuery.q` | 用原生索引名称匹配，在分页前搜索可见子项名称中的词语。 |
| `option:NodeDocumentListQuery.q` | 分页前，在可读和仅管理名称两个阶段中搜索可见标题词语。 |
| `option:InputScheduleListQuery.q` | 分页前搜索可见的计划名称词语。 |
| `option:InputScheduleListQuery.thread_id` | 仅保留可读目标及代理来源均指向此线程的计划。 |
| `option:SharedListQuery.q` | 在原生投影后、分页前，匹配可见名称、描述及已披露路径中的词语。 |
| `option:SharedListQuery.kind` | 仅选择 org、project、topic、thread、document 或 document_container；省略则包含所有类型。 |
| `option:SharedListQuery.limit` | 每页请求条目数；默认 50，可用任意正 u32。 |
| `option:SharedListQuery.cursor` | 用返回的游标继续同一用户、类型和搜索。 |
| `option:PermissionPeopleListQuery.q` | 分页前匹配可见姓名或用户名词语；实际人员目录包含所有符合条件的人，不受拒绝提示预览数量限制。 |
| `option:OrgChatListQuery.include_empty` | 在 nodes 范围中包含尚无消息的可读场所会话，供转发选择。默认关闭；普通活动列表仍保持原意。 |
| `option:OrgChatListQuery.ids` | 可选的逗号分隔会话 ID，批量经过原有组织、范围、参与状态和当前账户权限检查。无权访问的会话省略；提供 ID 不会授予权限。 |
| `option:OrgChatListQuery.limit` | 每页会话数，默认五十，接受任意正 u32。 |
| `option:OrgChatListQuery.cursor` | 延续同一人员、根组织、范围、参与状态、身份筛选和搜索文字。条件变更时从首页重读。 |
| `option:OrgChatListQuery.q` | 分页前，通过原生名称投影匹配可见会话标题、参与者姓名或已披露的节点名称。 |
| `option:ChatGetQuery.members_q` | 在分页前，由 ChatGet 投影可见群组成员的姓名和用户名；省略时保留普通完整元数据视图。 |
| `option:ChatGetQuery.members_limit` | 请求的成员页条目数；可用任意正 u32，成员页面默认请求 50。 |
| `option:ChatGetQuery.members_cursor` | 同一聊天和搜索返回的 members_next_cursor。 |
| `option:CollectionQuery.q` | 在服务器上按已公开的目录文字搜索词语或引号内的短语，无最短长度要求。引号内的空格按原样匹配。更改搜索时从新的游标开始。 已发送消息列表匹配标题与记录的发送人名称；文档的活跃通知任务匹配状态、阶段、进度文字与日期。 |
| `option:CollectionQuery.limit` | 本页请求的条目数，默认 50，可指定任意正的 u32 数值。它只决定分页大小，不限制集合总量。 |
| `option:CollectionQuery.cursor` | 同一集合、同一搜索返回的 next_cursor；继续读取直到为 null。不可用于其他父级或搜索。 |
| `option:ChildCollectionQuery.collection` | 子目录：MachineGet 为 agents/providers/environments；HostGet 为 kept_layers/kept_data；MountGet 为 locations/kept/lost_locations。只返回所选子目录；其他子目录数组为空。省略全部子目录查询字段时，返回完整的原生资源读取。 |
| `option:ChildCollectionQuery.q` | 按所选子目录中已公开的名称和位置文字搜索。不检索隐藏的名称或路径。 |
| `option:ChildCollectionQuery.limit` | 单页的正整数条目数，默认 50；不会限制结果总数。 |
| `option:ChildCollectionQuery.cursor` | 同一阅读者、资源、子目录和搜索条件下的游标。更改子目录或搜索时从第一页开始。 |
| `option:HostKeptLayers.latest` | 这是否是该工作区最新的可见保留版本。原生完整观察会在分页前决定此标记。 |
| `option:EnvironmentGetQuery.inspect_deleted` | 查看保留数据时，使用工作区当前的恢复权限。普通实时读取不变。 |
| `option:EnvironmentGetQuery.threads_q` | 按已公开的绑定线程、话题、所有者或目录文字搜索。服务器先检索完整的原生可见投影，再选择分页。 |
| `option:EnvironmentGetQuery.threads_limit` | 显式请求绑定线程分页时的正整数页大小（默认 50）。省略所有 threads_* 字段时，保留功能输入所用的完整环境读取。 |
| `option:EnvironmentGetQuery.threads_cursor` | 同一阅读者、工作区和搜索条件下的下一页游标。更改搜索时清除游标。 |
| `option:AdminTokenListQuery.q` | 按管理令牌目录中可见的名称搜索，然后分页。游标只适用于同一管理员、所有者和搜索条件。 |
| `option:NodeDeletedEnvironmentListQuery.q` | 在服务器上按保留工作区的当前名称搜索，然后分页。更改搜索时清除游标。 |
| `option:OrgGroupListQuery.q` | 分页前按组名中的词搜索。 |
| `option:OrgGroupListQuery.limit` | 每页条目数；默认 50，可设为任意正数。 |
| `option:OrgGroupListQuery.cursor` | 继续读取相同组织、子组织范围和搜索的结果。 |
| `option:JobRef.clone` | 当前源仓库克隆已接收的字节数、对象数及空闲时限。 |
| `option:CloneProgress.bytes` | 本次克隆已接收的仓库传输字节数。 |
| `option:CloneProgress.objects` | 本次克隆中 Git 已接收的对象数。 |
| `option:CloneProgress.idle_seconds` | 本次克隆允许连续无进展的秒数。 |
| `option:Problem.allowed_upstreams` | 仅向当前机器管理者披露的允许上游地址。空列表表示未登记地址；省略表示不披露列表。 |
| `option:Problem.field` | 结构化拒绝事实对应的确切请求字段；机器上游拒绝会指向相应上游地址字段。 |
| `option:Problem.reason` | machine_upstream 表示 provider.upstreams 策略拒绝，并向机器管理者提供允许地址；harness_error 保留原生启动原因。 |
| `option:Problem.policy` | 原生门或上游检查拒绝操作时报告的具体策略键。此事实在公共错误清理后仍保持结构化；界面显示该策略及恢复操作。 |
| `option:AgentRuntime.build` | 缺少代理程序时使用的系统登记构建。机器复用主机的校验缓存，仍受 agent.programs 和 tools.fetch 限制；明确指定的本地程序优先。 |
| `option:AgentRuntime.build.name` | 登记的代理程序种类，必须与所选代理一致。 |
| `option:AgentRuntime.build.version` | 所选构建版本，须符合代理配置固定的版本。 |
| `option:AgentRuntime.build.url` | 构建目录提供的下载地址，不接受会话提供的本地程序路径。 |
| `option:AgentRuntime.build.sha256` | 发布或执行前必须匹配的构建 SHA-256。 |
| `option:AgentRuntime.build.job_id` | 代理启动时为空；只有供应任务拥有工具下载进度。 |
| `option:ProviderDiscoverModels.max_report_bytes` | 本次模型发现固定使用的完整响应字节数，包括标准化后的完整列表；来自 providers.upstream_whole_bytes，默认 16 MiB。 |
| `option:UploadSessionPutReply.expires_at` | 接收载体在确认字节前缀后保留的空闲到期时间。进展按创建时的间隔续期；后续默认值更改不改变现有会话。 |
| `option:UploadSessionCreateBody.to` | 在发送字节前选择工作区目标。省略时保留为 API 上的私有上传。文件转送选择本机载体；实际机器和环境实例在该会话结束前保持绑定。 |
| `option:UploadDestination.environment_id` | 逻辑目标环境。暂存和发布仍受其当前文件管理权限与远程写入策略约束。 |
| `option:UploadDestination.path` | 初始工作区相对路径，包含文件名。上传后通过替换或保留两者选择最终核对的路径。 |
| `option:UploadDestination.thread` | 可选的所在会话，不授予环境写入权限。 |
| `option:UploadedBlob.relay` | 内容仅暂存在目标位置时的已核验本机上传会话。它不授予保留的私有附件来源；省略时表示普通存储上传的持有关系。 |
| `option:UploadSessionCompletion.file` | ready 分支中的实际已核验文件回执。核验尚未完成时没有文件回执。 |
| `option:UploadSessionCompletion.retry_after_ms` | 再次检查本机核验前采用的文件转送重试间隔。再次询问不会重新发送已接收的字节。 |
| `option:EnvironmentUploadWriteBody.relay` | 此次工作区写入使用的已核验本机会话。任务在不占用环境串行键时准备，核对目标版本后发布；省略时使用已获授权的保留内容。 |
| `option:FetchState.reason` | 无法读取所选文件的原因：实时转送使用 offline；缓存模式没有保留版本时使用 offline_not_cached；或 ignored、too_large、not_found。源端散列计算在本机完成或失败前保持 fetching。 |
| `option:CredentialForm.expires` | 访问令牌副本按签发方报告的时间到期，即使未声明固定时长。副本不包含续期凭据。 |
| `option:ArrangementConsentForm.expires` | 共享凭据的到期时间由登录凭据报告；未给出固定时长不表示永久有效。 |
| `option:TokenCreated.token` | 只在创建时显示一次的 API 密钥：opat_，后接密钥自身带连字符的小写 UUID、下划线、承载 256 位随机秘密值的 43 位无填充 base64url，再接前述全部文本的六位补零 Base62 CRC-32/ISO-HDLC 校验码。不接受旧式随机格式。 |
| `option:TokenCreateReceipt.token` | 与 TokenCreated.token 相同的规范化密钥，在确认回执中显示一次；重复创建的回执为 null。 |
| `option:CreateProvider.runtime_location` | 连接的位置：按主机和模板选择新建容器、使用个人机器，或使用调用者可使用的现有环境。环境内的连接随该环境的可用状态和持久主目录运行。 |
| `option:ProviderConnectionLocation.kind` | 通过同一个控件选择 container（容器）、machine（机器）或 environment（环境）。 |
| `option:ProviderConnectionLocation.machine_id` | 机器位置所选的机器；采用其实际声明的连接服务，并检查当前使用权限。 |
| `option:ProviderConnectionLocation.environment_id` | 所选的逻辑环境；移动或重建后由其当前专属连接器提供连接。放在其中的凭据可被该环境的代理和终端用户读取。 |
| `option:ProviderSetup.environments` | 调用者可使用的现有环境及其实际连接器服务；不可用的选项会保留原因。 |
| `option:ProviderSetupEnvironment.environment` | 连接位置选择中显示的环境名称和当前状态。 |
| `option:ProviderSetupEnvironment.connector` | 当前专属连接器及其声明的服务；没有专属连接器时为空。 |
| `option:Provider.connection_environment` | 提供此连接的环境，包括已停止或已移除状态；另外选择的凭据保管位置仍然独立。 |
| `option:ProviderListQuery.connection_environment_id` | 列出连接位于该环境中、且调用者可查看的提供方。 |
| `option:AgentRuntime.environment_home` | 将环境内连接的独立代理主目录保存在该环境的持久主目录中，从而随正常的主目录迁移一起转移；明确选择的现有目录仍按该选择使用。 |
| `option:FileRenameResult.kept_entries` | 跨文件系统移动后，在原位置观察到的保留条目数（包括其所在目录）。只按先子项后目录的顺序删除身份和版本仍与副本一致的条目；被忽略、新增或已更改的条目会保留。无法读取的子树计为一项。文件系统无法提供稳定文件身份时，保留原位置的副本。 |
| `option:ProviderStartStopBody.force` | 即使提供方自身的工作卡住，也可停止提供方。其他工作或运行中的轮次仍可能需要另行处理。记录立即变为已停止；系统持续通知主机，直到主机确认。 |
| `option:HostRuntimeStopBody.force` | 从主机页面对提供方运行实例执行同样的强制停止。环境运行实例不支持此选项，并会明确拒绝。 |
| `option:HostRuntimeDeleteQuery.force` | 结束阻碍删除的提供方自身工作，再按所选的数据卷处理方式移除运行实例。已经提交的工作和被替换的运行实例仍保留各自的保护规则。 |
| `option:ProviderRuntime.detail` | 当前运行状态的原因。强制停止时先说明仍在等待主机确认，确认后改为通常的手动停止原因。主机提供的诊断信息保持原文。 |
| `option:native_model.program_output.max_output_tokens` | 已允许的程序模型步骤的输出词元额度；进程重启后仍保留同一请求的额度。 |
| `option:native_model.program_output.schema` | 适配器支持强制结构化输出时，由模型使用方提供的回答格式；不授予来源访问权或工具权限。 |
| `option:BackendRef.config.environment_id` | 程序会话后端的来源线程没有环境时，要准入的确切环境。普通线程工作始终保留其实际环境与目录；不会选择第一个在线环境。 |
| `option:BackendRef.config.helper_topic_id` | 真实来源没有话题时，程序辅助线程在来源治理根组织内使用的现有话题。来源的实际话题优先。放置位置不会授予创建、环境或提供方权限。 |
| `option:CapabilityBackendQuery.place` | 能力后端预览的实际节点。仅选择全局摘要或问答模型策略的默认值时可省略；此时只读取提供方元数据和当前使用权限，不代表来源策略、执行位置或额度检查通过。实际执行始终检查其自身来源。 |
| `option:SummaryModelChoice.backend` | 一次显式摘要请求的完整后端，包括路径、登录账号、模型与执行位置。重试保留该选择及选项，不会重新构造仅适用于 API 的模型身份。 |
| `option:SummarySnapshot.problem.reason.source_changed` | 模型可能已经开始执行后，来源内容发生变化。保留原摘要；“重新生成摘要”是一次新的显式付费操作。 |
| `option:SummarySnapshot.problem.reason.settings_changed` | 已接受请求的摘要策略发生变化。保留原摘要，重新发起前先检查当前选择。 |
| `option:SummarySnapshot.problem.reason.content_changing` | 模型获准执行前，来源内容已用尽所设的免费重新读取次数。保留原摘要，等待内容稳定或主动重新发起。 |
| `option:SummarySnapshot.program` | 已保留的订阅执行事实。仅在当前可读取助手会话时显示助手及其所有者；仅在当前可读取提供方时显示实际登录及其套餐。展开这些信息不会调用模型。 |
| `option:SessionOpen.program_work.input_files` | 按文件传给所选模型的已冻结图片内容哈希。连接器校验图片内容，并用哈希生成的文件名将其写入临时隔离单元。 |
| `option:DriverDescriptor.program_units` | 此运行位置对隔离能力单元的实际支持和不可用原因。必须具备受限代理和强制网络边界；其他位置的支持不为此处授权。 |
| `option:TicketWanted.program.expires_at` | 私有程序通道所绑定原任务的绝对截止时间。重新签署已存命令不会延长任务期限，也不授予来源读取权限。 |
| `option:ProgramPlacement.environment_id` | 无线程程序消费方的明确环境引用。null 表示未设置；缺失、格式错误或空引用不能启动工作。 |
| `option:ProgramPlacement.helper_topic_id` | 生产方没有实际话题时的明确辅助话题。它必须属于生产方的治理根组织。提供方凭据与任意程序输入不属于放置位置字段。 |
| `option:CapabilityFailure.reason.program_sign_in_changed` | 已打开的私有程序上下文属于先前的登录账号。新的请求可使用当前账号。 |
| `option:Resolution.reason.program_unit_unavailable` | 所选登录账号没有可运行隔离能力单元的节点。 |
| `option:Resolution.reason.program_unit_network_unavailable` | 节点驱动需要受限的提供方代理和隔离网络才能运行此能力。 |
| `option:SummaryWait.reason.usage_measurement` | 自动订阅任务需要等待当前套餐用量数据，才能判断预留额度。 |
| `option:SummaryWait.reason.plan_reserve` | 当适用窗口的剩余额度不高于所配置的预留比例时，自动订阅任务将等待；原有段落仍然可读。 |
| `option:Resolution.reason.program_placement_invalid` | 所选程序后端包含格式错误的放置引用，或包含不属于放置配置的字段。请修正选择；保留原后端，不会自动改选其他登录或环境。 |
| `option:Support.execution` | 实际程序适配器报告已实现的输入与结果编解码器，以及经过验证的流式输出、读取工具桥和工具隔离支持。仅有委派模式或提供方名称不能证明这些能力。 |
| `option:Route.execution` | 此路径所需的输入与结果编解码器和执行设施。预览与执行均要求匹配的已安装代码；选择路径不会启动工作。 |
| `option:ResolvedRoute.execution` | 与所选路径一同保留、直到执行时检查的实际消费方要求。这些是技术要求，不是凭据或访问授权。 |
| `option:ExecutionSupport.input` | 已安装适配器理解的确切输入编解码器标识。不同请求形状需要其实际编解码器。 |
| `option:ExecutionSupport.result` | 确切的结果编解码器标识。文本回答、查看过的文件或附加图像不能证明另一种结果类型。 |
| `option:ExecutionSupport.streaming` | 仅在已实现原生增量事件与持久回放时为真；流式回答消费方需要此支持。 |
| `option:ExecutionSupport.read_tools` | 仅在已实现绑定真实来源的读取工具桥时为真。这不会授予来源的读取权限。 |
| `option:ExecutionSupport.confined_tools` | 仅在验证原生执行将工具限制在已获准的消费方目录时为真。要求程序遵守规则的提示词不足以证明隔离。 |
| `option:Resolution.reason` | 所选能力路径不可用的安全原因。execution_codec_unavailable 表示已安装执行器缺少所需编解码器或执行设施；program_execution_unavailable 表示原生适配器尚未验证此消费方。不会自动改选其他提供方。 |
| `state:TerminalNotAllowedReason.environment_stopping` | 环境已接受停止请求，不能再打开或连接终端。 |
| `state:BrowsersOff.environment_stopping` | 环境正在执行已接受的停止请求，浏览器与显示屏连接已停用。 |
| `state:BundledNodeEnrollment.waiting_for_administrator` | 内置主机正在等待首位管理员完成设置. |
| `state:BundledNodeEnrollment.waiting_for_node` | 内置主机已登记，正在等待节点连接. |
| `state:BundledNodeEnrollment.token_file_unreadable` | API 无法读取内置主机的注册令牌文件. |
| `state:BundledNodeEnrollment.invalid_token_file` | 内置主机的注册令牌文件无效. |
| `state:BundledNodeEnrollment.different_authority` | 内置主机的注册令牌来自另一套安装. |
| `state:BundledNodeEnrollment.registration_unavailable` | 内置主机未能在数据库中登记. |
| `option:MachineFacts.data_volume_bytes` | 连接器数据目录所在文件系统的总字节数。 |
| `option:MachineFacts.capacity_refresh_secs` | 机器在线时常规报告的最长间隔（秒）。主动探测会重新测量。 |
| `option:BindingEnvironment.capacity` | 提供此线程环境的已连接机器最近报告的可见容量。 |
| `option:MoveMachine.capacity` | 此目录目标机器最近报告的可见容量。 |
| `option:PlaceEnvironment.capacity` | 提供此目录的已连接机器最近报告的可见容量。 |
| `option:MachineCapacity.cpus` | 连接器可用的处理器数。 |
| `option:MachineCapacity.memory_bytes` | 机器总内存（字节）。 |
| `option:MachineCapacity.data_volume_bytes` | 连接器数据文件系统的容量（字节）。 |
| `option:MachineCapacity.capacity_refresh_secs` | 常规容量报告间隔（秒）。 |
| `option:EnvironmentDisplayCreateBody.desktop` | 使用机器提供的桌面共享程序共享本地桌面。 |
| `option:EnvironmentSupportVnc.desktop` | 已连接的机器可以共享本地桌面。 |
| `option:EnvironmentFactsVnc.desktop` | 机器检测到了本地 X11 桌面和已安装的共享程序。 |
| `option:DisplayAdopt.target` | 选择本地端点或机器桌面。 |
| `option:DisplayAdoptResult.endpoint` | 机器上选定的本地端口、套接字或桌面名称。 |
| `state:DisplayTarget.desktop` | 向已获准的显示屏观看者共享本地桌面。 |
| `state:DisplayTarget.endpoint` | 使用指定的本机 RFB 服务。 |
| `option:EnvironmentDisplayCreateBody.attach` | 连接机器上已有的 VNC 服务。 |
| `option:DisplayServer.port` | 机器回环接口上的非零 TCP 端口。 |
| `option:DisplayServer.password` | 机器每次连接 VNC 时使用可选的加密口令。观看者使用显示屏票据。 |
| `option:Machine.execution_approvals` | 由服务器为此机器视图核实的当前执行决定；本机报告不能指定其中的人员。“程序”页复用此投影，无须额外请求。 |
| `option:MachinePrograms.approvals` | 由服务器核实确认人和时间的当前执行决定；机器报告不能为此列表指定人员。 |
| `option:MachineExecutionApproval.subject` | 当前所选程序的准确类型和路径。 |
| `option:MachineExecutionApproval.by` | 服务器记录的管理者，以人员引用显示。 |
| `option:MachineExecutionApproval.at` | 该管理者记录决定的时间。 |
| `option:ExecutionApproval.by` | 实际作出决定的管理者；历史姓名不可用时不会虚构其他确认人。 |
| `option:ExecutionApproval.at` | 决定的记录时间；决定未变时保留原时间。 |
| `state:MachineExecutionSubject.system` | 管理者已确认此操作系统及处理器架构上的指定连接器程序。 |
| `option:MachineExecutionSubject.build` | 此确认所涵盖的原生程序构建。 |
| `option:MachineFacts.runtime_platform` | 未经验证的系统上可供确认的当前原生构建。 |
| `option:CleanupPlan.mount_switch_off_blocked` | 无法取消挂载的可见位置，以及其他受限位置的数量。 |
| `option:Problem.mount_switch_off_blocked` | 删除挂载被拒绝时，账户可查看已确认且仍然可见的受限位置，以及其他受限位置的数量。重新读取删除计划可获取当前列表。 |
| `option:MachinePolicy.platform_review` | 此机器当前可确认的构建及已生效的决定。 |
| `option:MachinePlatformReview.build` | 待确认的系统、架构和可执行文件。 |
| `option:MachinePlatformReview.approved` | 当前连接器构建是否与确认记录一致。 |
| `option:MachinePlatformReview.approval` | 已核实的委托确认者及时间。 |
| `option:EnvironmentContext.reviewed_native_system` | 本机连接器有与此未经验证的原生构建匹配的确认。 |
| `option:PlatformBuild.os` | 当前连接器的操作系统。 |
| `option:PlatformBuild.arch` | 当前连接器的处理器架构。 |
| `option:PlatformBuild.executable` | 连接器可执行文件的 BLAKE3 摘要。 |
| `option:PlatformConsent.state` | 此机器是否已确认某个构建。 |
| `state:PlatformConsent.off` | 尚未确认任何未经验证的原生构建。 |
| `state:PlatformConsent.approved` | 机器所有者或管理者已确认指定的原生构建。 |
| `option:PlatformConsent.build` | 已确认的操作系统、架构和连接器摘要。 |
| `option:PlatformConsent.confirmation` | 委托确认时由服务器核实的管理者和时间。 |
| `option:LocalPolicy.exec.platform` | 由本机策略控制的原生系统确认。 |
| `state:PolicyKey.exec.platform` | 在未经验证的系统上确认当前连接器的机器策略。 |
| `option:MachineExecutionSubject.kind` | 机器执行确认对象的类型。 |
| `option:MachineExecutionSubject.harness_kind` | 此决定所覆盖的程序所属适配器。 |
| `option:MachineExecutionSubject.path` | 当前所选程序的路径；其他路径不能继承此确认人的记录。 |
| `state:MachineExecutionSubject.program` | 按机器实际本地策略选中的程序。 |
| `option:PolicyHarness.confirmation` | 服务器记录的管理者和时间；缺省表示本地选择，不虚构确认人。此记录不授予权限。 |
| `option:Confirmation.by` | 服务器记录的实际管理者，不从本地配置推断。 |
| `option:Confirmation.at` | 管理者作出此决定的时间；决定未变时保留原时间。 |
| `option:MachineProgramsRequest.request` | 原始的本机程序操作；HTTP 请求不提供确认人记录。 |
| `option:MachineProgramsRequest.confirmation` | 确认程序时由服务器记录的管理者和时间；其他程序操作为 null。 |
| `option:McpBinding.server_id` | 与机器链路绑定一致的已注册 MCP UUID；面向适配器的对应结构在读取和写回时保留该标识。 |
| `option:Job.kind` | mount_copy 任务按已确认的计划独立复制挂载数据，使用共用的进度和结果记录；server_update 在随服务器安装的节点上运行发行版更新程序。 mount_copy 的“通知”沿用同一条收件箱通知记录进度与结果，每位受影响者至多一条；关闭时不发送。 复制进度仅向当前有权读取机器的查看者显示所等待的机器名称。 |
| `option:JobProgress.message` | `mount_copy_custody_pending` 表示清理仍保留运行状态，直到剩余复制数据有持久的保留记录；进度行和提示按查看者的语言显示。 |
| `option:MountCopyPreflightBody.source_machine_id` | 保存此挂载位置的现有机器；服务器记录该位置的确切关联、路径或自有卷。 |
| `option:MountCopyPreflightBody.target_machine_id` | 允许的目标机器；服务器根据其当前数据存储能力选择存储位置。 |
| `option:MountCopyPreflightBody.into` | 目标已有数据时选择替换或新位置；两种选择均保留原有数据。 |
| `option:MountCopyPlan.preflight_id` | 当前用户保存的复制计划；确认复制及查看可见通知对象均使用此标识。 |
| `option:MountCopyPlan.expires_at` | 计划的到期时间；实际数据来源发生变化时须提前重新查看计划。 |
| `option:MountCopyPlan.mount` | 当前可读、即将复制的挂载。 |
| `option:MountCopyPlan.source` | 读取数据的原始机器，仅在可读时显示。 |
| `option:MountCopyPlan.target` | 可读的目标机器。 |
| `option:MountCopyPlan.copy` | 共用的复制计划，包含测量结果、目标数据及其保留方式；未完成的统计显示为未知。 |
| `option:MountCopyPlan.blockers` | 当前阻止确认的原因；查看受阻计划不会更改数据。 |
| `option:MountCopyPlan.notification` | 共用的通知选择，以及去重后的受影响人数。 |
| `option:MountCopyPlanReceipt.projection` | 已保存计划的受限回执，不包含现有资源内容。 |
| `option:MountCopyPlanReceipt.mount_id` | 本次请求指定的挂载。 |
| `option:MountCopyPlanReceipt.preflight_id` | 已保存计划的标识，不包含测量数据或私有位置详情。 |
| `option:MountCopyPlanReceipt.expires_at` | 确认此计划的截止时间。 |
| `option:MountCopyCreateBody.preflight_id` | 需要确认的当前计划；重复确认会返回同一个持久任务。 |
| `option:MountCopyCreateBody.notify` | 显式通知选择；省略时使用计划中的默认值，每次操作对每位接收者只生成一条汇总通知。 |
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
| `option:LifecycleTarget.observed` | 此次原始操作先前观察到的物理来源。没有先前来源时也必须明确传入 null；未跟踪的运行实例不会被解释为已知不存在。 |
| `option:LifecycleTarget.opening` | 在首次产生效果前记录的新建实例或新一次启动的独立标识。新建不会接管先前观察到或尚待确认的实例；启动复用仍在运行的实例时保留其实际来源。 |
| `option:LifecycleTarget.pending` | 先前已获准、可能已产生效果但回复尚未收到的准确新建或启动标识。必须明确传入数组，没有则为空；它不授予权限，也不包含后来无关的来源。 |
| `state:LifecycleAction.create` | 具有独立标识的新建操作，不接管先前观察到或尚待确认的实例。产生效果前须确认能够全新分配；仅有 null 不能证明不存在。 |
| `option:RuntimeSpec.labels.orch.birth` | 由已获准的新建标识设置的原生全新分配标记。驱动不会为它复用或改写已有实例；准确的原始重试保留已记录的物理引用。 |
| `option:NodeRuntimeStopResult.outcome` | 停止结果报告实际停止的来源，或证明其准确的原始目标已不存在。空结果或未知结果不能证明物理实例不存在。 |
| `option:NodeRuntimeStopResult.source` | 在原生效果锁内确认已停止的实际、已跟踪来源，供下次启动和只针对原始目标的结果核对使用。 |
| `option:NodeRuntimeStopResult.originals` | 已证明不存在的准确原始来源或已获准启动标识；不会据此选择或清除无关的当前实例。 |
| `state:NodeRuntimeStopResult.stopped` | 指定的实际原生来源已停止；数据遵循现有审核后的保留选择。 |
| `state:NodeRuntimeStopResult.absent` | 准确的原始目标已证明不存在或已关闭；未跟踪的来源不属于此类证据。 |
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
| `option:MountUpdateBody.notify` | 对启用状态变更明确选择是否通知，同一请求须包含 enabled。省略或 false 均不通知；true 仅为已提交的实际变更通知当前受影响者。再次保存相同启用状态不会重复通知。 |
| `option:NotificationOperation.kind` | mount_delete 表示一次已确认的挂载删除，清理结束时更新原有分组通知。mount_enable 和 mount_disable 表示已提交的启用状态变更，每位受影响者至多一条，操作者本人不收到通知。 |
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
| `option:HostRuntimeStopReceipt.projection` | 已接受对指定单元的停止请求，不披露现有运行时内容；这不表示已确认物理单元不存在。 |
| `option:HostRuntimeStopReceipt.host_id` | 本次请求选定的主机。读取运行时内容除 Manage 外还需要同一操作的 Read。 |
| `option:HostRuntimeStopReceipt.runtime_id` | 本次选定的确切单元；后续运行时或原生启动不能替换此回执。 |
| `option:ProviderStopReceipt.projection` | 已完成对指定提供方的停止请求，不披露现有提供方内容；这不表示已确认物理单元不存在。 |
| `option:ProviderStopReceipt.provider_id` | 本次操作选定的提供方。读取其现有内容还需要同一操作的 Read。 |
| `option:Cmd.authority_until` | 服务端记录的原始命令凭据和操作的绝对到期时间。原生队列或准备过程的等待不会延长它。必须明确传入 null 才表示没有有限的时间界限；此字段不授予任何权限。 |
| `option:PagePreview.server_host_use` | 向随服务端安装的主机授予使用、管理或所有者权限前的当前提示和精确确认凭据；没有新增授权时省略。 |
| `option:ServerHostUseDisclosure.hosts` | 本次页面或默认权限变更会新增或更改授权的受保护服务端主机数量，不公开主机标识或名称。 |
| `option:ServerHostUseDisclosure.confirmation` | 通过 orch-server-host-use-confirmed 请求头确认此次权限变更及当前原生授权集合；变更意图改变时需重新确认，此凭据本身不授予访问权限。 |
| `option:ProviderModelPauseQuery.note` | 停用模型时的简短备注，显示在使用该模型的受影响会话中。 |
| `option:ProviderModelPauseQuery.run_waiting` | 启用时只有 true 才运行符合条件的等待消息；省略或 false 时仍等待手动恢复。 |
| `option:ProviderModelPauseQuery.notify` | 停用时明确选择向该模型的每位受影响者发送一条持续更新的生命周期通知；省略时不发送。对话框按“操作通知”设置初始选项。 |
| `option:Notification.data` | 模型生命周期通知的 operation_kind 为 provider_model_disable 或 provider_model_delete，状态为 running、done 或 failed；进度和结果更新同一条合并通知。 |
| `option:ResourceDeleteQuery.notify` | 删除提供方、其中的模型或 MCP 定义时，明确选择向每位受影响者发送一条合并通知；省略时不通知。提供方子任务进度更新同一条收件箱通知。MCP 通知记录定义移除，机器上的清理仍通过持久命令继续。 |
| `option:EnvironmentListRow.lifecycle` | 环境列表中的启动和停止操作使用驱动报告的生命周期，区分运行单元和挂接目录。 |
| `option:HostListRow.bundled_with_server` | 在主机列表中标识与此服务器一起注册的受保护主机。 |
| `option:ResourceImpact.notification` | 当前提供方、模型、挂载或 MCP 服务操作的已确认通知对象、人数和“通知”初始选项。确认操作前可查看通知对象。 |
| `option:ResourceOperationNotification.preview_id` | 与当前用户绑定的受影响人员确认记录。 |
| `option:ResourceOperationNotification.people` | 已确认位置中符合通知条件的总人数。 |
| `option:ResourceOperationNotification.audience` | 可显示的受影响人员，以及关联关系不可见的人员数量；不包含操作确认者，每人只计一次。 |
| `option:ResourceOperationNotification.default_on` | 根据此次影响继承的“操作通知”初始值；确认者可在执行前修改选项。 |
| `option:ExportLimits.max_file_bytes` | 当前服务器对单个导出文件的大小额度，在计划中与整个压缩包的额度并列显示。较大的文件会被省略并写入报告；这不会提高或替代当前位置的压缩包总额度。 |
| `option:ExportJobResult.omitted_items` | 已生成压缩包实际省略的项目数。最终元数据无法读取时仍表示未知。有省略内容的可下载压缩包标为“部分导出”；下载内容包含省略报告，已保存的报告可用时也会在下载旁单独提供。 |
| `option:SystemComponent.issue` | 服务现有原因旁的已诊断运行问题，不会替代服务自身的详情，也不表示 API 已停止服务。 |
| `state:SystemComponentIssue.metrics_listener_unavailable` | API 无法绑定可选的指标监听。管理页在 API 状态旁说明此情况；API 仍继续服务，证书续期和备份记录核对保持原有行为。 |
| `option:MetricSeries.resolution` | 图表说明返回的读数来自分钟级还是小时级存储数据，不据此声称历史完整、最早保留边界已知，或每个点对应一个原始样本。 |
| `option:ThreadQueue.eligibility` | 现有运行前检查确认的选择，而非当前载入的第一条消息；读取不预留工作。 |
| `option:QueueEligibility.input_ref` | 准确选中的等待输入，不作为裸标识显示。 |
| `option:QueueEligibility.safe_author` | 通过当前安全人员投影提供原始作者。 |
| `option:QueueEligibility.safe_authority` | 实际检查其运行权限的人员，可以与作者不同。 |
| `option:QueueEligibility.blocking_reason` | 仍阻止此合格输入开始的会话级类型化条件。 |
| `option:QueueEligibility.evaluated_at` | 实际记录此资格观察的时间。 |
| `option:QueueEligibility.queue_revision` | 此观察核查的队列版本；依赖或队列更改后，重新核查前为尚未报告。 |
| `state:QueueEligibility.selected` | 某条等待输入实际通过了针对个人的资格规则。 |
| `state:QueueEligibility.none` | 检查后的队列没有合格输入，不从载入页面为空来推断。 |
| `state:QueueEligibility.unreported` | 观察记录缺失或已失效；打开详情不会再次执行运行前检查。 |
| `option:Turn.start_wait` | 此已接纳原生启动当时的策略及绝对截止时间；缺失记录仍为尚未报告。 |
| `option:ThreadStartWait.stage` | 原生接收、原生启动确认或实际事件接收宽限阶段。 |
| `option:ThreadStartWait.began_at` | 服务器进入此观察阶段的时间。 |
| `option:ThreadStartWait.deadline` | 此启动实际采用的绝对截止时间；有限时长超出日历范围不表示无限等待。 |
| `option:ThreadStartWait.after_millis` | 毫秒表示范围内提供当时的有限阶段等待额度。 |
| `option:ThreadStartWait.admitted_default_source` | 接纳时记录的设置来源范围，不以之后的默认值替换。 |
| `option:ThreadStartWait.closed_at` | 确认原生启动或任务终止后关闭此等待；旧记录不描述之后的工作。 |
| `state:ThreadStartWaitStage.acceptance` | 等待原生启动命令被接收。 |
| `state:ThreadStartWaitStage.start` | 接收已成功；按当时最迟接收界限继续等待原生启动事件。 |
| `state:ThreadStartWaitStage.events` | 此会话的原生事件仍在当时记录的额外宽限内接收。 |
| `option:Approval.delivery` | 原生决定送达与已接受的决定分开；旧记录缺失时不假定已送达。 |
| `option:ApprovalDelivery.recipient_kind` | 实际接收方类别，不暴露其他人的送达渠道。 |
| `option:ApprovalDelivery.state` | 等待中、已确认送达，或无法确认送达后的未知状态。 |
| `option:ApprovalDelivery.last_attempt_at` | 最近一次实际新送达尝试，而非之后读取或重看同一命令的时间。 |
| `option:ApprovalDelivery.problem` | 类型化送达问题，不改写已接受的人工回答，也不暴露原生私有文本。 |
| `state:ApprovalDeliveryRecipient.native_harness` | 原生程序接收此保留决定。 |
| [决定已接受 · 等待送达](state-reference.md#state:ApprovalDeliveryState.pending) | 决定已接受；收到原生确认前仍待送达。 |
| [已送达](state-reference.md#state:ApprovalDeliveryState.applied) | 原生送达命令已确认成功。 |
| [决定已接受 · 送达情况未知](state-reference.md#state:ApprovalDeliveryState.unknown) | 尚未确认送达，不表示决定已丢失。 |
| `state:ApprovalDeliveryProblem.rejected` | 原生请求未能接受此决定。 |
| `state:ApprovalDeliveryProblem.timed_out` | 实际送达命令已超过截止时间。 |
| `state:ApprovalDeliveryProblem.cancelled` | 送达命令已取消。 |
| `state:ApprovalDeliveryProblem.delivery_failed` | 该尝试无法确认原生程序已接收。 |
| `option:InputSchedule.timing_policy` | 当前计划预览的类型化规则，与旧执行记录分开。 |
| `option:InputScheduleRun.outcome.timing_policy` | 判定此执行次数时实际记录的规则，不受之后编辑时间影响。 |
| `option:InputScheduleOccurrenceOutcome.timing_policy` | 当次执行记录的规则；缺失时保持尚未报告。 |
| `option:InputTimingPolicy.clock` | 当地时间、世界协调时 cron 或明确选定的固定时刻。 |
| `option:InputTimingPolicy.repeated_time` | 重复当地时间使用较早时刻；单次计划保留明确选定的时刻。 |
| `option:InputTimingPolicy.missing_time` | 重复计划跳过不存在的当地时刻；单次计划拒绝无效时刻；世界协调时没有当地时间跳空。 |
| `option:InputTimingPolicy.overdue` | 重复计划积压合并至最近一次；单次计划只有一次。 |
| `option:InputTimingPolicy.paused_dates` | 计划恢复时不补发暂停期间的次数。 |
| `state:InputTimingClock.fixed_instant` | 明确选定的绝对时刻。 |
| `state:InputTimingClock.civil` | 所选时区中的重复当地日期和时间。 |
| `state:InputTimingClock.utc` | cron 按世界协调时计算。 |
| `state:InputTimingFold.explicit_instant` | 作者明确选择了时刻。 |
| `state:InputTimingFold.earlier_instant` | 重复计划的重复时刻仅在较早时间执行一次。 |
| `state:InputTimingFold.not_applicable` | 不适用当地重复时刻规则。 |
| `state:InputTimingGap.refused` | 不能保存无效的单次时刻。 |
| `state:InputTimingGap.skipped` | 不存在的重复当地时刻记为跳过。 |
| `state:InputTimingGap.not_applicable` | 不适用当地不存在时刻规则。 |
| `state:InputTimingOverdue.single_occurrence` | 该计划只有一次执行。 |
| `state:InputTimingOverdue.latest_occurrence` | 较早积压次数合并到最近一次到期执行。 |
| `state:InputTimingPause.not_replayed` | 恢复后不补发暂停期间的次数。 |
| `option:ThreadInput.delivery_resolution` | 仅在此次输入实际回退时记录原始请求及实际送达方式。 |
| `option:AnnouncementPayload.delivery_resolution` | 与保留公告一同提供当时的送达判定，不受之后编辑影响。 |
| `option:InputDeliveryResolution.requested` | 原始请求方式，不从当前能力推断。 |
| `option:InputDeliveryResolution.applied` | 输入实际采用的方式，不代表原生程序已接收。 |
| `option:InputDeliveryResolution.reason` | 当时记录的类型化回退原因。 |
| `option:InputDeliveryResolution.source` | 接收时记录的此次明确选择或实际设置来源。 |
| `option:InputDeliveryResolution.resolved_at` | 接收方接受此送达判定的时间。 |
| `option:InputFallbackOrigin.value` | 当时的回退值，策略之后更改也不替换。 |
| `option:InputFallbackOrigin.scope` | 当时的设置范围；未经核查的上级节点身份仍保密。 |
| `state:InputFallbackOrigin.request` | 发送者明确为此次发送选择了回退方式。 |
| `state:InputFallbackOrigin.setting` | 实际生效的公告回退设置决定了此选择。 |
| `state:InputDeliveryResolutionReason.steer_unavailable` | 接收方此回合当时无法即时引导。 |
| `state:InputDeliveryResolutionReason.steer_unavailable_and_reach_limited` | 当时无法即时引导，且发送者无权中断此回合。 |
| `option:MarkerSessionReopened.outcome` | 此次打开操作确认的原生结果；先前意图或旧记录不代表恢复成功。 |
| `option:MarkerAgentHistory.outcome` | 由原生导入接收或实际交接送达确认的历史重建结果，与此历史记录一同保留。 |
| `option:Segment.session_outcome` | 用于检查的最近确认结果，不证明原生会话当前仍存在，也不替代旧标记自身的事实。 |
| `option:NativeSessionOutcome.kind` | 已验证的原生恢复与已应用的记录历史重建是不同结果。 |
| `option:NativeSessionOutcome.reason` | 已记录且已知的重新打开原因；缺失表示尚未报告。 |
| `option:NativeSessionOutcome.carried_method` | 实际重建方式；原生恢复未携带替代历史时不适用。 |
| `option:NativeSessionOutcome.coverage` | 记录轮次选择与已送达转录文件的覆盖范围分开，不代表原生事件或原始输出字节完整。 |
| `option:NativeSessionOutcome.coverage.recorded_turns` | 表示记录中的全部轮次或尾部轮次；摘要未记录选择范围时为尚未报告。 |
| `option:NativeSessionOutcome.coverage.transcript_file` | 实际提供给新会话的完整记录转录文件的覆盖范围；仅原生导入不代表传输了文件。 |
| `option:NativeSessionOutcome.at` | 服务器观察到原生确认成功或实际重建送达的时间。 |
| `state:NativeSessionOutcomeKind.verified_resume` | 原生程序确认恢复了保留的会话身份。 |
| `state:NativeSessionOutcomeKind.reconstructed` | 记录的对话已实际导入或交接已送达。 |
| `state:SessionHistoryCoverage.complete` | 表示所选范围的全部记录轮次，或已提供完整的记录转录文件。 |
| `state:SessionHistoryCoverage.partial` | 仅表示记录中最新的一段尾部轮次。 |
| `state:SessionHistoryCoverage.not_applicable` | 此次结果不涉及携带该类内容。 |
| `state:SessionHistoryCoverage.not_reported` | 此保留结果未记录该覆盖范围，不会默认为完整。 |
| `option:Turn.offline_episode` | 此回合当前或最近一次记录的离线事件；缺失表示尚未报告，而非无限期等待。 |
| `option:ThreadOfflineEpisode.began_at` | 本次离线开始时记录的时间，不受之后修改设置或重启 API 影响。 |
| `option:ThreadOfflineEpisode.deadline` | 本次实际离线事件记录的有限或无限期策略标签。 |
| `option:ThreadOfflineEpisode.deadline.after_seconds` | 记录的有限等待时长，不另加上限；超出日历范围不等于无限期等待。 |
| `option:ThreadOfflineEpisode.deadline.at` | 可表示时提供记录的绝对截止时间；否则仍保留有限时长。 |
| `option:ThreadOfflineEpisode.policy_source` | 记录的设置来源范围；不会暴露未经核查的私有上级节点身份。 |
| `option:ThreadOfflineEpisode.episode_revision` | 仅在此回合出现新的离线事件时更改，重复离线报告不会重新计时。 |
| `option:ThreadOfflineEpisode.closed_at` | 该观察记录不再当前有效的时间，不证明原生任务已结束。 |
| `state:OfflineEpisodeDeadline.finite` | 记录的有限时长及可表示的截止时间。 |
| `state:OfflineEpisodeDeadline.indefinite` | 本次离线事件明确记录为没有截止时间。 |
| `option:RetainedInput.id` | 原始输入标识，而非伪造的原生条目。 |
| `option:RetainedInput.continuation_id` | 在选定读取投影下，该输入所属的分支；读取不会切换执行分支。 |
| `option:RetainedInput.author` | 通过当前安全人员引用提供原始发送者；姓名缺失时不显示标识。 |
| `option:RetainedInput.submitted_at` | 用于显示的原始提交时间；定位仍由锚点决定。 |
| `option:RetainedInput.native_action` | 与原始输入一同保留的明确原生操作；仍遵循该输入的可见性。 |
| `option:RetainedInputContinuation.after_item` | 需要继续读取保留输入的锚点；为空表示位于所有条目之前。 |
| `option:RetainedInputContinuation.cursor` | 用于同一锚点及已观察历史切片的普通时间线游标；读取不会重复条目或启动工作。 |
| `option:InputScheduleRun.outcome` | 当次计划执行时记录的判定；省略表示尚未报告，与输入之后的排队、暂缓、开始或失败状态分开。 |
| `option:InputScheduleRun.outcome.reason_code` | 类型化的接收、上次输入未完成而跳过、当地时刻不存在、计划结束或接收被拒原因，不从英文说明推断。 |
| `option:InputScheduleRun.outcome.coalesced_count` | 实际合并到此次已接收输入的先前次数；零不显示合并说明，跳过时不声称已接收输入。 |
| `option:InputScheduleRun.outcome.scheduled_for` | 当次记录的执行时刻；当地时刻不存在时为空，之后修改计划不会替换此记录。 |
| `option:InputScheduleRun.outcome.zone` | 当次执行记录的时区，与计划的当前定义分开。 |
| `option:InputScheduleRun.outcome.accepted_input_ref` | 仅在目标当前可读时提供已接收的输入标识，否则不提供。 |
| `option:ThreadInput.cancellation` | 保留的取消原因、时间及操作者；不会根据当前权限或默认值推测缺失事实。 |
| `option:InputCancellation.cause` | 实际记录的原因：权限更改、人工移除、停止、编辑移除、被取代、会话结束或选定来源不可用；停止后暂缓的队列不等于取消。 |
| `option:InputCancellation.at` | 记录的取消时间，而非读者打开会话的时间。 |
| `option:InputCancellation.by` | 通过当前安全人员引用提供的记录操作者；自动结果或未共享身份时为空。 |
| `option:ThreadSync.retained_inputs` | 首次窗口或同步中的已取消人工输入，与条目分开，即使没有原生会话段也可返回；仍遵循输入本身的可见性。 |
| `option:ThreadSync.retained_inputs_next` | 锚点组超过窗口共享字节额度时，供按需继续读取的游标。 |
| `option:ThreadItemListReply.retained_inputs` | 属于本页锚点的保留输入，在正常游标遍历中各返回一次。 |
| `option:ThreadItemListReply.retained_inputs_next` | 各返回锚点的后续输入记录，不重复条目正文。 |
| `option:ThreadItemListQuery.retained_inputs_cursor` | 通过正常时间线读取继续某个保留输入组，并重新检查当前会话及历史读取权限。 |
| `option:RetainedInput.after_item` | 前一个会话条目；位于所有条目之前时无锚点。客户端按锚点及记录的输入顺序定位，不比较时钟。 |
| `option:RetainedInput.seq_created` | 同一锚点下输入的不可变提交顺序。 |
| `option:RetainedInput.seq_updated` | 用于合并实时取消结果的版本。 |
| `option:RetainedInput.outcome` | 已记录的取消事实；为空表示原因及时间尚未报告。 |
| `option:RetainedInput.parts` | 在输入原有可见性下返回的人工原文，不伪造原生送达。 |
| `option:ThreadData.retained_inputs` | 来自同一当前已授权会话投影的实时保留输入记录。 |
| `option:ThreadData.retained_inputs_next` | 与同一已授权实时切片一起提供的保留输入游标。 |
| `state:InputScheduleOccurrenceReason.accepted` | 正常输入已接收；合并次数另行记录。 |
| `state:InputScheduleOccurrenceReason.previous_input_pending` | 因上次输入仍在等待而跳过本次执行。 |
| `state:InputScheduleOccurrenceReason.nonexistent_local_time` | 当时记录的当地时刻在其时区中不存在。 |
| `state:InputScheduleOccurrenceReason.schedule_ended` | 此次领取前已达到记录的计划额度。 |
| `state:InputScheduleOccurrenceReason.admission_failed` | 当前接收检查拒绝本次执行；具体问题另行记录。 |
| `state:InputCancellationCause.access_changed` | 原作者已无权向此会话发送消息。 |
| `state:InputCancellationCause.person` | 人工明确移除了排队输入。 |
| `state:InputCancellationCause.stop` | 停止操作实际取消了此输入；仅暂缓队列不属于此原因。 |
| `state:InputCancellationCause.edited_away` | 已接受的编辑从后续工作中移除了此输入。 |
| `state:InputCancellationCause.superseded` | 另一条已接受输入取代了此输入。 |
| `state:InputCancellationCause.thread_ended` | 会话在接收此输入前结束。 |
| `state:InputCancellationCause.source_unavailable` | 选定的来源已被移除或停用。取消的原文仍保留在会话中；这不表示会话本身已结束。 |
| `option:TurnLost.reason` | 连接器记录的轮次失联原因。 |
| `option:TurnLost.turn_id` | 标识连接器重启后无法再跟踪的已受理轮次。 |
| `option:ThreadSideCreateBody.acting` | 选择后在父线程的工作目录中启动可执行操作的独立会话。省略时只允许经验证的只读侧线程。 |
| `option:ThreadSides.acting` | 父线程已绑定智能体、服务商和环境，可供侧线程执行操作。侧线程策略仍然适用。 |
| `option:system.web_push_contact` | 部署文件 `orch.toml` 中提供给浏览器推送服务的公开 HTTPS 联系页面或 `mailto:` 地址。默认使用公开的 HTTPS 站点地址。本机 HTTP 安装无需填写联系方式即可注册浏览器；部分推送服务投递时要求提供联系方式。 |
| `option:NoticeDecision.requested_but_suppressed` | 本轮通知未发送时，若读者当时已明确选择了相应的关注级别、允许接收此类通知，且没有在查看会话，则为真。 |
| `option:AskMoveOutPlan.preflight_id` | 当前调用者的完整内置对话复制及原会话原生删除审阅。确认时重新检查复制、导出、目标创建、所有权及每项将被删除的资源。 |
| `option:AskMoveOutPlan.delete_plan` | 与其他会话移动相同的原生原会话删除范围。范围改变须重新审阅，拒绝时不创建副本。 |
| `option:AskMoveOutPlan.open_side_threads` | 移动主会话前须关闭或保留的独立开放侧对话。名称按当前查看权限显示。 |
| `option:AskMoveOutBody.preflight_id` | 在一个事务中确认审阅，保留内置程序、复制全部对话历史、采用目标访问默认值及目标组织的治理。创建重试只确认同一结果，即使原会话已经结束。 |
| `option:SessionSpec.opening_command` | 实际原生启动所回报的控制器开启命令。在内部传递，不由用户填写，也不取自会话凭据。 |
| `option:SessionOpened.mcp` | 适配器实际提供的 Orch 服务器名称。未回报时保持尚未报告，直到延迟启动的原生程序报告配置；空列表表示确认未提供任何服务器。 |
| `option:SessionOpenResult.mcp` | 连接器确认中的同一实际提供回执。请求提供服务器本身不能证明已经提供；仅改变模型传输时保留既有工具成员。 |
| `option:MarkerHarnessChanged.value.opening_command` | 将原生 MCP 配置关联到确切的控制器开启。另一次开启或已关闭的进程不能继承此提供事实；使用原生同名定义不算作被忽略的 Orch 资源的调用。 |
| `option:ThreadMoveAudience.people` | 当前所有符合条件的原对话贡献者，不含移动者。作者读取在内部翻页，通知选择没有固定的人数上限。 |
| `option:ThreadMoveAudience.preview_id` | 现有可搜索、分页人员列表使用的同一私有移动审阅；仅在没有接收者时为空。 |
| `option:ThreadMoveOutPreviewResponse.notification` | 可选移动通知所审阅的完整贡献者人数；只有写入权限的计划回执不包含此项。 |
| `option:EnvironmentMcp.connections` | 所请求线程中程序的实际连接，包含未报告静态可导入配置的服务器，避免不完整的配置发现隐藏正在运行的连接。 |
| `option:McpConnection.name` | 该次程序开启所报告的服务器名称。 |
| `option:McpConnection.source` | 报告此连接的程序类型。 |
| `option:McpConnection.state` | 该次实际开启所报告的状态；报告尚未到达时为未报告。恢复或重启后的程序不会继承上一次开启的就绪状态。 |
| `option:McpConnection.using` | 本次开启选择的程序自身配置或 Orch 定义。 |
| `option:MarkerHarnessChanged.value.epoch` | 用于关联同一次实际程序开启的 MCP 配置与连接报告。它属于观测元数据，无需用户输入。 |
| `option:MarkerHarnessChanged.value.servers` | 程序开启时的 MCP 配置快照。即使新配置没有服务器，也会替换旧证据；每项注明服务器名称和提供的定义来源。 |
| `option:McpServer.spec.imported.origin` | 随导入副本保留的不透明原生发现来源。私有提供方和档案路由仅保存在控制器证据中，不会写入公开的资源配置。 |
| `option:EnvironmentMcpImportCreateBody.origin` | 现有列表中所选发现行携带的不透明标识，用于区分不同配置档案中的同名原生服务器；仅在条目没有歧义时可省略。用户无需输入此标识。 |
| `option:McpFound.origin` | 导入及重试回执使用的稳定原生条目标识。提供方身份保持私有，列表行和操作会自动携带此值。 |
| `option:McpFound.profile` | 可读取的原生代理档案名称；无权查看该档案时省略。连接观测和不透明导入标识不会授予档案访问权限。 |
| `option:McpProfileRef.id` | 该命名原生配置所对应、已获准显示的代理档案，使用现有资源标签呈现。 |
| `option:McpProfileRef.name` | 可读取的原生档案名称；这里不披露其主目录、命令、地址或凭据。 |
| `option:McpImport.profile` | 发现时选定的实际提供方和代理档案。连接器从这个确切的已托管档案复制定义，不会选择另一个同名服务器。 |
| `option:McpImportAsk.profile` | 本机导入通过已认证的请求通道保留同一个所选原生档案。 |
| `option:EnvironmentFactsMcp.profile` | 托管连接器为已发现原生定义记录的私有提供方与档案关联，用于区分同名服务器，不披露启动信息。 |
| `option:McpFound.state` | 所选线程当前原生片段中，程序最后报告的连接状态。没有报告则显示未报告；旧片段、子对话或接入 Orch 前的历史都不能充当当前状态。 |
| `option:McpFound.using` | 该程序连接使用的定义来源：程序自身配置或 Orch 服务器。导入本身不会修改程序原来的定义。 |
| `state:McpDefinitionSource.native` | 本机程序保留自己的全局或目录配置。同名时保留这份原生定义；导入的 Orch 副本仍可在其他环境使用。 |
| `state:McpDefinitionSource.orch` | 程序收到的是 Orch 为此环境启用的服务器。程序的连接状态与环境宿主的服务器状态分别报告。 |
| `option:MarkerHarnessChanged.value.definition_source` | 原生 MCP 连接观测注明会话开启时提供的定义来源：程序自身或 Orch，不含命令、地址或凭据。来源未知时仍为未报告。 |
| `option:FilesPlan.source_policy` | 此次工作区读取或复制实际选择的来源域，捕获在已接受审核中，不由界面根据当前路径或环境推断。 |
| `option:FilesPlan.home_policy` | 实际测量的主目录复制采用整个自有数据根策略。不应用工作区排除规则；写入仍受普通调用者与操作权限约束。 |
| `option:FilesPlan.file_states_policy` | 已测量记录数据复制的整个自有数据根策略，与工作区排除规则独立。未捕获策略的旧计划仍显示尚未报告。 |
| `option:UserMessagePayload.native_image_delivery` | 原始已接受命令实际采用的图像传递决策，包括额度、解析时间与安全设置来源。缺失历史记录保持尚未报告；私有上级设置来源保持未共享。 |
| `option:ThreadInput.native_image_delivery` | 排队或已传递输入上的同一图像传递记录。之后更改设置不会重写记录。 |
| `state:NativeImageDelivery.native` | 原图像字节随已接受原生命令传递。 |
| `state:NativeImageDelivery.path` | 已接受输入使用已验证的图像文件路径。仅实际正数字节额度拒绝时显示超出图像额度；零显示图像通过路径传递。 |
| `state:NativeImageDelivery.unavailable` | 没有可用原生字节或已验证文件路径，不表示已通过路径传递。 |
| `option:UploadSessionCreateReply.policy` | 创建此上传时采用的分片和共享窗口策略；之后的设置修改不会改写它。 |
| `option:UploadSessionCreateReply.chunk_bytes` | 采用的单片字节上限，来自保存的传输策略。 |
| `option:UploadSessionPutQuery.offset` | 文件范围内的任意偏移；可重试相同的已确认字节，冲突字节会被拒绝。 |
| `option:UploadSessionStatus.received` | 已持久确认的不重复字节总数，包含中间缺片之后的已确认后缀。 |
| `option:UploadSessionStatus.ranges` | 读取会话状态返回的已持久化区间，已排序并合并；仅重试其中空缺。 |
| `option:UploadSessionStatus.policy` | 此会话采用的传输策略；读取状态不会修改上传字节。 |
| `option:UploadRange.start` | 从零开始计数的首个字节位置。 |
| `option:UploadRange.len` | 字节数；区间不包含 start + len 位置。 |
| `option:TransferStatus.progress` | 不重复的接收字节数；complete 表示已验证大小及完整内容哈希。 |
| `option:TransferStatus.ranges` | 机器传输的 JSON 状态包含持久化区间，避免把区间列表塞入 HTTP 头。 |
| `option:TransferStatus.policy` | 机器上传采用的策略，或传输开始前的当前系统策略。 |
| `option:TransferPolicy.initial_requests` | 初始小型带宽探测的数量，受共享请求数和字节上限约束。 |
| `option:TransferPolicy.initial_piece_bytes` | 尚未测得线路速率时，用于首次带宽测量的字节数。 |
| `option:TransferPolicy.min_piece_bytes` | 普通文件分片的最小字节数；最后一片可更小。 |
| `option:TransferPolicy.max_piece_bytes` | 单个文件分片的最大字节数。未完成的上传保留创建时采用的策略。 |
| `option:TransferPolicy.target_window_ms` | 共享窗口的目标数据时长，再加上测得的往返时间余量。 |
| `option:TransferPolicy.window_rtts` | 在字节上限内，提前保持在途数据所覆盖的往返次数。 |
| `option:TransferPolicy.max_window_bytes` | 同一发送者的并行文件共享的最大在途字节数。 |
| `option:TransferPolicy.requests` | 文件共享的并行分片请求数。默认五个，为 HTTP/1.1 的其他操作留出一个连接。 |
| `option:UploadSessionStatus.completed` | 在会话到期前保留的完成回执；丢失完成响应后可读取回执，无需重发文件，也不会恢复已释放的保管权限。 |
| `option:FileTransferReply.transfer` | 当前环境、机器和代次已宣布的原生清单传输。它与浏览器上传会话无关；读取不会启动原生工作。 |
| `option:FileTransferFacts.expires_at` | 上次接收分片时间加上首个分片采用的空闲期限。原始记录缺失时不推断日期；之后更改设置不会改变此日期或来源。 |
| `option:FileTransferFacts.retention_source` | 捕获时的注册默认值或系统设置来源。清理过期分片后仍保留此记录。 |
| [正在传输](state-reference.md#state:FileTransferState.transferring) | 原生发布者宣布了尚未完成的传输，不表示已成功发布。 |
| [已中断](state-reference.md#state:FileTransferState.interrupted) | 原生发布者报告了此次传输中断；其记录的到期时间仍适用。 |
| [已到期](state-reference.md#state:FileTransferState.expired) | 清理确认在记录的空闲到期时间之后移除了未完成传输。 |
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
| `state:DirEntryKind.permission_denied` | 机器可以列出此名称，但其运行账户没有权限读取该项。 |
| `state:DirEntryKind.vanished` | 读取目录时，该名称已消失。 |
| `state:DirEntryKind.unreadable` | 机器读取此项时遇到了其他文件系统错误。 |
| `state:FileSourceKind.workspace` | 附加工作区应用 Orch 排除规则与受保护控制文件规则。 |
| `state:FileSourceKind.mount` | 不应用工作区排除规则的只读挂载根。 |
| `state:FileSourceKind.owned_data` | 不应用工作区排除规则、整体复制的自有数据根。 |
| `state:FileRuleKind.configured` | 已配置的 Orch 模式参与决定此路径的纳入规则。 |
| `state:FileRuleKind.local` | 适用本地排除文件参与决定；其名称被排除时隐藏。 |
| `state:FileWatchSource.machine_configuration` | 备用刷新间隔由已安装观察器接受的机器配置提供。 |
| `option:Item.text_replacements` | 对此条目收到的文本和键名中的 U+0000 应用的替换次数。安全显示使用 U+FFFD；再次收到完整更新时可产生新的替换操作。 |
| `option:Item.storage_gap` | 修复文本后仍无法保存的最近一个源事件。条目保留上次有效内容和安全的拒绝信息，后续事件继续处理。 |
| `option:ItemStorageGap.sqlstate` | PostgreSQL 的数据或程序限制拒绝代码，不包含被拒绝的值或数据库语句。 |
| `option:ItemStorageGap.event_seq` | 原生事件提供的被拒绝事件序号；不是原生事件时可以没有此值。 |
| `option:ItemStorageGap.at` | 记录拒绝的时间。 |
| `option:OutputRef.inline_source_len` | 修复后的内联预览代表的原始源字节数，为正数；空预览不含此成员。替换字符的 UTF-8 字节数更大时，仍保持保留范围的字节边界准确。 |
| `error:storage.data` | 修复后仍无法保存源值。条目保留明确的缺口；不重复尝试该值，后续原生事件继续处理。 |
| `option:Turn.sandbox_resolution` | 连接器记录的本轮请求沙箱与实际沙箱。缺失表示尚未报告；之后修改机器策略不会改写此记录。 |
| `option:ThreadBinding.sandbox_resolution` | 当前原生会话最近一次记录的沙箱决定，与后续轮次的决定分开保存。 |
| `option:SandboxResolution.requested` | 机器应用上限之前请求的沙箱。 |
| `option:SandboxResolution.applied` | 实际传给原生程序的沙箱，而非机器当前的设置。 |
| `option:SandboxResolution.governing_policy` | 决定本次选择的策略来源类型，不公开本机策略内容或无关设置。 |
| `option:SandboxResolution.command_id` | 记录实际沙箱时对应的原生命令；此标识不授予查看该命令的权限。 |
| `option:SandboxResolution.at` | 连接器确认应用该命令的时间，而非之后读取记录的时间。 |
| `option:Machine.spool` | 上次经过身份验证的带外输出存储报告。权威队列写入受阻时仍可报告；机器离线时标为上次报告。 |
| `option:MachineListRow.spool` | 紧凑机器行中的同一存储背压事实，无需额外读取详情。 |
| `option:SpoolStatus.problem` | 存储当前无故障时为 null；否则为 full、write、sync、damaged 或 event_rejected。不包含原生文本或文件系统路径。 |
| `option:SpoolStatus.waiting_events` | 发布者持有的事件数，直到完整记录存储并持久化成功。追加失败会重试，不会确认或清退原生事件。 |
| `state:SpoolProblem.full` | 存储空间或配额已满。权威输出等待并自动重试，不会为满足诊断字节额度而丢弃。 |
| `state:SpoolProblem.write` | 写入或回滚失败。原始发布仍被持有，机器仍可报告状态并处理控制命令。 |
| `state:SpoolProblem.sync` | 将已接受的字节同步到持久存储失败。未同步版本和发布者回执会保留，等待再次同步。 |
| `state:SpoolProblem.damaged` | 启动时发现保留输出损坏。此报告与普通网络断开区分。 |
| `state:SpoolProblem.event_rejected` | 执行程序保留超出当前连接限制的事件以供重试。其他流仍可发送输出。机器无权发布的流会立即拒绝相应报告。 |
| `option:ToolCallPayload.masked_fields` | 捕获遮蔽所更改的原生公开字段的 JSON 指针。更改后的地址仅显示为文本，不作为可执行链接；原始执行参数以加密私有数据保留。 |
| `option:InstructionSource.path_redacted` | 原生来源地址已遮蔽。来源行会说明为何不能打开该地址；原生来源仍为私有数据。 |
| `option:RawEvent.masking` | 原生诊断文本被隐藏的原因：等待原生结束、连续性未知或匹配上下文不完整。完整捕获的文本会先拼接再遮蔽。 |
| `state:NativeTraceMasking.pending` | 适配器声明的原生文本流尚未结束；期间仅保留加密私有副本。 |
| `state:NativeTraceMasking.unclassified` | 原生协议未标明文本连续性；已启用的捕获遮蔽会隐藏该文本。 |
| `state:NativeTraceMasking.incomplete` | 匹配前缀未被捕获或已超出保留范围；剩余文本继续隐藏。适配器声明的新字段可使用独立完整上下文。 |
| `option:NativeTraceText.start` | 适配器声明原生字段开始，具有新的完整匹配上下文。捕获缺口之后，在新的开始或完整快照之前不推断文本连续性。 |
| `option:RawFrame.text` | 适配器声明的语义文本流、片段 JSON 指针和原生结束标记。其参数为私有数据；遮蔽会在普通诊断存储前拼接片段。 |
| `option:RawFrame.text_complete` | 适配器确认帧中的其余文本均为完整文本。启用原生遮蔽时，连续性未知的文本将被隐藏。 |
| `option:ResourceNotificationQuery.notify` | 仅为此次删除通知已复核的受影响人员；省略或 false 时不通知，个人渠道仍单独选择。 |
| `option:ResourceNotificationQuery.notification_review` | 当前影响预览中绑定给操作者的受众；选择通知时必填，人员变化后需重新复核。 |
| `option:ResourceStopQuery.note` | 可选的单行备注，显示给因停用提供方而等待的线程。 |
| `option:ResourceStopQuery.notify` | 仅为此次停用提供方通知已复核的受影响人员；省略或 false 时不通知。 |
| `option:ResourceStopQuery.notification_review` | 当前影响预览中绑定给操作者的受众；选择通知时必填。 |
| `option:ResourceImpactQuery.skill_publication` | 发布技能时，只统计当前已交付且有效选择为跟随最新版的线程上下文，同时遵循个人选择及上层锁定。固定版本即使恰好是当前最新版也不计入。省略或 false 时，按资源整体变更统计全部当前交付；此选项仅用于技能。 |
| `option:CreateThread.workspace_edits` | 保存在启动计划中的文件编辑；打开编辑器不会创建工作区。创建时在来源准备及环境初始化完成后、智能体收到第一条输入前应用。冲突会保留两份文本并暂停启动。写入成功后消费此计划，后续轮次读取真实共享文件。 |
| `option:WorkspaceFileEdit.path` | 相对于所选线程目录的文件路径，不可重复、为绝对路径或越过父目录。共享说明编辑器使用 AGENTS.md。 |
| `option:WorkspaceFileEdit.text` | 用户保存的完整文件文本，在条件写入成功前保留于启动计划中；会话元数据不披露文件内容。 |
| `option:WorkspaceFileEdit.expected_hash` | 用户读到的来源版本。省略或 null 表示仅在文件不存在时创建；未知来源带来现有文件时须先复核，不会无条件覆盖。 |
| `option:WorkspaceFileEdits.revision` | 当前启动文件计划及绑定工作区的标识。替换编辑时须提供此版本，避免静默覆盖较新的编辑、目录或环境。 |
| `option:WorkspaceFileEdits.edits` | 尚待应用的文件编辑，同时按绑定线程的查看权限及环境的 files.view 权限读取。应用成功后为空。 |
| `option:EnvironmentSourceEditsQuery.thread_id` | 与所选环境实际绑定、要读取启动文件编辑的线程；此字段不会额外授予文件或线程访问权限。 |
| `option:EnvironmentSourcePrepareBody.workspace_edits` | 复核来源后明确替换待应用的文件编辑；空列表表示采用来源文件，省略则保留已保存的编辑。写入仍须通过现有来源和工作区权限检查。 |
| `option:EnvironmentSourcePrepareBody.workspace_edit_revision` | 替换 workspace_edits 时必填；版本不符会拒绝旧编辑并保留较新的计划。 |
| `option:MountSpec.resource_id` | 实际原生挂载携带的资源标识，不授予任何权限。名称或目标路径变化后仍可关联到原资源；没有标识时保持未报告状态。 |
| `option:ResourceImpact.applications` | 当前原生工具和技能的交付，以及仍保留挂载或配置配方的运行中单元。每组按生效时点分别统计去重后的线程、环境或提供方，空组不显示；显式授予范围和历史恢复引用不算当前使用者。 |
| `option:ResourceImpact.application_complete` | 原生来源是否能说明所有当前交付和挂载。为 false 时明确保留证据缺失状态，不把未知当作没有受影响的工作。 |
| `option:ResourceApplicationGroup.when` | 这次资源变更到达该组现有原生依赖项的时点。 |
| `option:ResourceApplicationGroup.object_kind` | 计数单位为线程、环境或提供方，不把不同种类相加成没有单位的数字。 |
| `option:ResourceApplicationGroup.count` | 当前依赖项的去重数量，包括此读者无权查看名称的项。同一挂载的重复观测只计一次。 |
| `option:ResourceApplicationGroup.visible` | 按本次影响操作的原生命名来源可独立读取的名称；能读取资源本身不等于能读取这些名称。 |
| `option:ResourceApplicationGroup.hidden` | 名称和标识保持私密的当前依赖项，只显示数量。 |
| `option:ResourceApplicationGroup.apply` | 应用这些待更新单元输入的现有更新或重建操作；调用或轮次边界没有此操作。 |
| `option:ResourceApplicationGroup.apply_targets` | 仅包括名称可独立显示且当前调用者有权管理的依赖项。确认前仍通过现有计划重新核查每个目标及影响。 |
| `state:ResourceApplicationWhen.now` | 在这些当前线程上下文中，经由已停用或删除的 MCP 资源发起的调用立即被拒绝。 |
| `state:ResourceApplicationWhen.before_next_turn` | 在下一轮开始前，重新解析这些当前工具或技能交付。 |
| `state:ResourceApplicationWhen.updated` | 这些运行中单元仍使用旧挂载或配置配方。通过提供的更新或重建计划应用变更；仅重启同一个单元不会应用新定义。 |
| `state:ResourceApplicationAction.update` | 通过现有更新计划审阅并应用待更新配置，需要时会替换单元；这不是重启同一单元的操作。 |
| `option:ThreadOutlineGetQuery.selection` | conversation 读取普通大纲，messages 读取人的提示及发送者、所属轮次的更改信息，agents 读取包括较早历史在内的子智能体简要记录。省略时选择 conversation。 |
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
| `option:AskModelOption.unavailable_reason.selected_backend_unavailable` | 已保存的后端不可用。请选择当前允许的模型，再发送下一条问题。 |
| `option:AskModelOption.unavailable_reason.placement_unavailable` | 程序需要可用的环境和对话根组织中有权使用的辅助话题。 |
| `option:AskModelOption.unavailable_reason.invalid_backend` | 请在模型策略中选择模型和已声明的问答路线。 |
| `option:ask.models.*.backend` | 完整的对话路线、账号模型和适配器配置。子组织按上级顺序缩小整个条目。 |
| `option:ask.models.*.backend.route` | 此模型使用的已声明路线。 |
| `option:ask.models.*.backend.offering` | 一起选定的账号和已启用模型。 |
| `option:ask.models.*.backend.offering.provider` | 所选账号。提问者和当前付费人都需要使用权限。 |
| `option:ask.models.*.backend.offering.offering` | 此账号上已启用模型的路由名称。 |
| `option:ask.models.*.backend.config` | 所选路线的配置，包括所需的运行位置。 |
| `option:AccessOptions.purposes` | 申请使用提供方的用途：Ask 或其他工作。仅申请 Ask 不会授权同一模型的普通线程轮次。 |
| `option:ProviderUse.purposes` | 此条目允许 Ask、其他工作或两者。省略时两者都允许；空列表两者都不允许。用途、模型、思考强度和运行位置必须由同一个条目共同允许。 |
| `option:ProviderUpdateBody.expected_revision` | 可编辑配置的版本。事实、运行状态和来源观察不会使其过期；其他配置编辑后需要重新读取当前版本。 |
| `option:ProviderModelDefaultsQuery.model_name` | 预览提供方所属位置的目录策略下，此确切路由名将使用的默认值。此读取仅使用已保存的事实，不发起发现，也不添加模型。创建请求明确提供 parameters 对象时，按完整定义保存，清除的可选项保持关闭。 |
| `option:Meta.interface_defaults` | 安装级别的最近关闭视图保留数量、加载提示延迟与读取新鲜时间。每个人都可在自己的界面偏好中覆盖这些默认值。 |
| `option:InterfaceDefaults.kept_closed_tabs` | 额外保留的最近关闭视图数量。打开的视图始终保留。 |
| `option:InterfaceDefaults.progress_delay_ms` | 读取开始到显示加载图标之间的毫秒数。 |
| `option:InterfaceDefaults.catch_up_above_items` | 安装级自动近况摘要卡片阈值。缺失时保持尚未报告，不算作零；用户保存的界面选择会覆盖此值。 |
| `option:InterfaceDefaults.catch_up_above_items_source` | 同一近况摘要条目数的全站或内置来源。未捕获来源时保持尚未报告。 |
| `option:ThreadBinding.session_skills` | 实际已接收会话的小型状态；未捕获判定时缺省，不包含技能内容。 |
| `option:SessionSkillsSummary.session_revision` | 隔离详情分页的已接收会话开启标识。 |
| `option:SessionSkillsSummary.segment_id` | 拥有此已接收会话的片段。 |
| `option:SessionSkillsSummary.applied_at` | 实际接收并应用的时间，不是后续资源修改时间。 |
| `option:SessionSkillsSummary.supported` | 已接收的程序是否支持技能；不支持与尚未报告不同。 |
| `option:SessionSkillsSummary.selected` | 实际提供给此会话的技能数。 |
| `option:SessionSkillsSummary.skipped` | 当时选择判定中跳过的候选数。 |
| `option:SessionSkillRef.id` | 当前可读取的技能标识。 |
| `option:SessionSkillRef.name` | 当前按权限投影的资源名称，不使用留存的私密名称兜底。 |
| `option:SessionSkillVersion.id` | 精确的已接收版本标识，仅在当前可读取该技能时提供。 |
| `option:SessionSkillVersion.number` | 为已接收版本捕获的版本号，不使用当前资源的最新版本。 |
| `option:SessionSkillSource.kind` | 已记录的来源类别：位置或个人。 |
| `option:SessionSkillSource.setting` | 当前安全的位置及来源投影；不可读取的来源保持未共享。 |
| `option:SessionSkillDecision.kind` | 已记录的判定类别：已选或跳过。 |
| `option:SessionSkillDecision.version` | 已接收的版本引用；当前不可读取技能时隐藏。 |
| `option:SessionSkillDecision.reason` | 候选被跳过时捕获的类型化原因。 |
| `option:SessionSkillEntry.skill` | 当前可读取的标识；不可读取时为空，不以私密标识或名称兜底。 |
| `option:SessionSkillEntry.contributor` | 判定时实际核验使用权的提供者当前安全引用。 |
| `option:SessionSkillEntry.source` | 按当前来源名称权限展示的已记录个人或位置选择。 |
| `option:SessionSkillEntry.decision` | 实际已选或跳过的判定，不受后续编辑影响。 |
| `option:ThreadSessionSkills.projection` | 此回答精确的所选继续版本读取范围。 |
| `option:ThreadSessionSkills.summary` | 实际已接收会话的小型状态。 |
| `option:ThreadSessionSkills.entries` | 仅包含已授权引用的一页；用其游标读取后续所有条目。 |
| `option:ThreadSessionSkills.next_session_changed` | 当前仅元数据的提议是否不同于此活动会话；未报告或不适用时为空。 |
| `option:ThreadSessionSkills.observed_at` | 此当前提议和投影的实际观察时间。 |
| `option:ThreadSessionSkillsQuery.continuation_id` | 明确的会话读取选择；省略时跟随活动执行。 |
| `option:ThreadSessionSkillsQuery.projection_revision` | 后续分页的读取投影隔离版本。 |
| `option:ThreadSessionSkillsQuery.inspect_deleted` | 对留存的已删除内容使用现有当前恢复权限。 |
| `option:ThreadSessionSkillsQuery.segment_id` | 拥有已捕获结果的精确片段。 |
| `option:ThreadSessionSkillsQuery.session_revision` | 不可变的已接收开启标识，不自动查询最新值。 |
| `option:ThreadSessionSkillsQuery.cursor` | 与同一不可变会话及位置关联的后续游标。 |
| `option:ThreadSessionSkillsQuery.limit` | 请求的正数分页条目数；省略时使用普通列表默认值，不另加产品上限。 |
| `option:SessionSkillEntryPage.items` | 此不可变分页中当前安全的判定条目。 |
| `option:SessionSkillEntryPage.next_cursor` | 原生后续游标，仅在提供完所有判定后为空。 |
| `option:ThreadSkill.contributor` | 原生使用权核验的提供者；公开投影另行按权限隐藏或展示安全引用。 |
| `option:SkippedSkill.contributor` | 与已记录跳过结果关联的原生提供者。 |
| `state:SessionSkillSource.place` | 选择来自已记录的位置行。 |
| `state:SessionSkillSource.personal` | 选择来自发起者的个人技能。 |
| `state:SessionSkillDecision.selected` | 该版本实际提供给已接收的会话。 |
| `state:SessionSkillDecision.skipped` | 选择判定因已捕获原因而跳过此候选。 |
| `state:SkillSkip.missing` | 未找到。 |
| `state:SkillSkip.disabled` | 已关闭。 |
| `state:SkillSkip.deleted` | 已删除。 |
| `state:SkillSkip.no_version` | 尚无版本。 |
| `state:SkillSkip.version_not_found` | 版本不可用。 |
| `state:SkillSkip.not_usable` | 判定时提供者缺少当前使用权。 |
| `state:SkillSkip.name_taken` | 另一项已选技能使用此名称；仅在可读取时展示胜出技能。 |
| `option:SystemInfo.effective_configuration` | 实际报告进程已捕获的安全运行值。缺少数据表示尚未报告，不推定为内置默认值。 |
| `option:EffectiveConfiguration.service` | 这些值实际应用于哪个进程，不推测其他服务的环境。 |
| `option:EffectiveConfiguration.process_revision` | 本次启动观察的不透明标识，用于隔离读取，不显示为设置值。 |
| `option:EffectiveConfiguration.measured_at` | 实际捕获配置的时间，不受后续文件编辑影响。 |
| `option:EffectiveConfiguration.values` | 已授权的安全白名单分页行；当前 API 进程仅报告自身已应用的数据库期限。 |
| `option:EffectiveConfigurationValue.public_key` | 明确安全白名单中的名称，界面使用公开标签，不展示加载器原始数据。 |
| `option:EffectiveConfigurationValue.typed_value` | 有限期限或明确关闭的期限；缺少数据不等于无限期。 |
| `option:EffectiveConfigurationValue.origin` | 已捕获的内置、文件或实际环境覆盖来源；未知来源保持尚未报告。 |
| `option:EffectiveConfigurationValue.application` | 后续更改需要重启进程，还是由该使用方在运行中应用。 |
| `option:EffectiveConfigurationValuePage.items` | 当前授权分页中的安全运行值行。 |
| `option:EffectiveConfigurationValuePage.next_cursor` | 原生后续游标；已经包含全部报告行时为空。 |
| `option:ConfigurationValue.kind` | 已捕获的有限期限或明确关闭的期限；缺少配置数据表示尚未报告。 |
| `option:ConfigurationValue.milliseconds` | 精确的无符号十进制毫秒文本，保留原生范围，不经浏览器数值取整。 |
| `state:ConfigurationValue.finite` | 已捕获的有限期限，包含精确毫秒值。 |
| `state:ConfigurationValue.indefinite` | 启动时明确关闭了此期限。 |
| `state:ConfigurationOrigin.compiled` | 使用内置默认值，没有明确的文件或环境覆盖。 |
| `state:ConfigurationOrigin.file` | 文件明确提供了此成员，即使其值等于内置默认值。 |
| `state:ConfigurationOrigin.environment` | 加载器实际应用了环境覆盖。 |
| `state:ConfigurationApplication.restart` | 使用方在启动时建立，更改后须重启该进程。 |
| `state:ConfigurationApplication.live` | 实际使用方在运行中应用更改。 |
| `state:ConfigurationKey.database.acquire_timeout_ms` | 安全公开的期限键，仅提供报告进程实际应用的值。 |
| `state:ConfigurationKey.database.request_statement_timeout_ms` | 安全公开的期限键，仅提供报告进程实际应用的值。 |
| `state:ConfigurationKey.database.background_statement_timeout_ms` | 安全公开的期限键，仅提供报告进程实际应用的值。 |
| `state:ConfigurationKey.database.idle_transaction_timeout_ms` | 安全公开的期限键，仅提供报告进程实际应用的值。 |
| `option:InterfaceDefaults.read_freshness_ms` | 目录、搜索、报表与状态读取的新鲜时间，单位为毫秒。 |
| `option:PersonalState.interface` | 当前登录者的阅读偏好。未给出的字段使用安装默认值；修改偏好会保留其他字段。显示标签页不会修改偏好。 |
| `option:PersonalState.interface.kept_closed_tabs` | 该用户保留的已关闭视图、显示状态及重新打开记录的数量。零仅保留最近一项的重新打开位置。 |
| `option:Workspace.closedLimit` | 该用户设备工作区中最近确认的保留数量。它在界面基线返回前限制关闭记录；下一次确认的原生偏好始终优先。 |
| `option:ReadAttemptFacts.read_attempt` | 当前浏览器实际发起的安全读取尝试，从一开始计数。多个观察者共享同一读取；替换后的用户不会继承先前的事实。 |
| `option:ReadAttemptFacts.reader` | 拥有此观察的浏览器用户代际。它用于隔离，不是显示身份或权限授予。 |
| `option:ReadAttemptFacts.operation` | 此观察描述安全读取。打开它不会重复修改操作。 |
| `option:ReadAttemptFacts.phase` | 本次实际读取、已接受的等待或失败阶段。取消后停用。 |
| `option:ReadAttemptFacts.scheduled_retry_at` | 实际调度器接受重试时记录的设备时钟时间。它表示可重试时间，不保证送达，也不按当前默认值重建。未记录的时间显示“尚未报告”。 |
| `state:ReadAttemptPhase.reading` | 本次已接受的读取正在进行。打开事实不会发起请求。 |
| `state:ReadAttemptPhase.waiting` | 安全读取已有接受的重试等待。取消或离开所属视图会结束等待并停用其事实。 |
| `state:ReadAttemptPhase.failed` | 实际读取失败，且没有接受的后续重试。受影响部分保留现有恢复操作。 |
| `option:SessionState.renewal` | 此浏览器观察到的续期阶段和时间，不含凭据或其他会话的信息。替换授权会停用先前的观察。 |
| `option:RenewalObservation.scheduled_at` | 此浏览器实际安排的下一次定时器。手动续期失败时保留已有安排；未知的下次尝试显示“尚未报告”。 |
| `option:RenewalObservation.renewed_at` | 当前持有会话成功续期时记录的设备时钟时间。直接登录或替换授权不会被当作先前的续期。 |
| `option:RenewalObservation.phase` | 此浏览器实际安排、进行、完成或失败的续期阶段，与原生会话列表分开。 |
| `state:RenewalPhase.scheduled` | 此浏览器已安排续期。正常安排不会添加常驻说明。 |
| `state:RenewalPhase.in_progress` | 当前用户的实际续期正在进行。并发请求共享此续期。 |
| `state:RenewalPhase.completed` | 此持有会话已实际续期。已记录的完成时间与下次定时器分开保留。 |
| `state:RenewalPhase.failed` | 本次续期失败。临时网络失败会保留持有会话，不会编造会话结束原因或重放修改操作。 |
| `option:SessionState.sessionEnd` | 仅记录此浏览器自身已结束会话的可靠原生结果。替换登录后停用；缺少证据时保留通用的会话结束提示。 |
| `option:SessionEndObservation.at_if_known` | 已知的原生结束时间，否则显示“尚未报告”。浏览器不会按令牌寿命或当前时间推算。 |
| `option:SessionEndObservation.cause` | 仅保留此浏览器持有会话的可靠原生到期、撤销或账号不可用原因。不保留其他原生字段。 |
| `option:DeviceChoiceObservation.scope` | 此用户浏览器中选择原有的类型、字段和可选项目。范围标识不作为显示名称，也不代表其他用户的设备清单。 |
| `option:DeviceChoiceObservation.reader` | 可以读取此记录的登录用户代际。替换用户后，先前写入函数不再生效，其事实也停用。 |
| `option:DeviceChoiceObservation.origin` | 实际地址、项目、类型、默认值或当前页面来源。每个控件按实际提供的选择判断可用性。 |
| `option:DeviceChoiceObservation.read_result` | 提供此值的同一次存储读取结果，区分可读、被拒绝、无效和内存保留。详情不会再次读取。 |
| `option:DeviceChoiceObservation.write_result` | 捕获的已保存、失败或尚未检查结果。读取成功不代表写入成功。当前控件保留被拒绝的值，不受未显示回退缓存的预算影响。 |
| `option:DeviceChoiceObservation.chosen` | 应用可用性规则前，实际请求或保留的选择。不可用的已保存选择会被保留。 |
| `option:DeviceChoiceObservation.effective` | 此控件实际使用的当前值。明确地址优先；回退不会改写不可用的已保存选择。 |
| `option:DeviceChoiceObservation.saved` | 从此选择的实际读取中已知的保存值。未知或不可读的名称显示“尚未报告”，不替换为不透明标识。 |
| `option:DeviceChoiceObservation.saved_choice_availability` | 根据控件实际提供的选择，区分可用、不可用和尚未报告。它与设备存储能否读写分开记录。 |
| `state:DeviceChoiceOrigin.address` | 当前明确地址提供此值。 |
| `state:DeviceChoiceOrigin.item` | 此项目明确保存的设备选择提供此值。 |
| `state:DeviceChoiceOrigin.kind` | 此类型保存的设备选择提供此值。 |
| `state:DeviceChoiceOrigin.default` | 控件使用实际默认值，并保留不可用的已保存意图。 |
| `state:DeviceChoiceOrigin.page` | 本页面保留的选择或当前内存回退提供此值，不保证持久保存。 |
| `state:DeviceChoiceRead.available` | 原有设备读取成功。没有记录时仍使用默认值，不当作已保存的零或关闭。 |
| `state:DeviceChoiceRead.unavailable` | 实际设备读取被拒绝或不可用。设备清单不能将此状态当作空清单。 |
| `state:DeviceChoiceRead.invalid` | 已保存字节可以读取，但不是可用的选择。它与存储读取失败分开显示。 |
| `state:DeviceChoiceRead.memory` | 当前选择保留在此浏览器的回退内存中。未挂载的回退值之后可能被回收。 |
| `state:DeviceChoiceSave.saved` | 观察到的设备写入成功。不保证浏览器永久保留，也不代表其他设备的状态。 |
| `state:DeviceChoiceSave.failed` | 实际设备写入失败。当前控件保留内存选择；控件消失后只保留安全的类型与保存状态。 |
| `state:DeviceChoiceSave.not_checked` | 没有捕获当前写入结果，不推断为保存成功。 |
| `option:DeviceChoiceFailure.kind` | 控件消失后，拒绝保存所对应的安全允许类型，包括文件查看器、窗口和列表选择。不在这里保留私有值、标识或虚构的已存储字节数。 |
| `option:DeviceChoiceFailure.write_result` | 当前用户实际失败的保存，在“在这台设备上”中保留。清除它不会声称先前保存成功。 |
| `option:Started.setupObservation` | 提供最近一次成功配置的同一次私有服务器读取，限定于当前账户读取代际。详情不会额外读取，已保存的选择仅通过当前原生选项显示；记住的布置不代表新的凭据授权。 |
| `option:PersonalState.interface.progress_delay_ms` | 该用户的加载提示延迟，单位为毫秒。零表示立即显示。 |
| `option:PersonalState.interface.catch_up_above_items` | 该用户的非负自动近况摘要阈值。省略时沿用安装默认值，零表示任意非空未读范围；手动摘要独立于此阈值。保存其他界面选择时保留此字段，不会额外创建覆盖值。 |
| `option:PersonalState.interface.read_freshness_ms` | 该用户对目录、搜索、报表与状态读取的新鲜时间覆盖值，单位为毫秒。 |
| `state:PersonalStateKey.interface` | 单独保存界面阅读偏好的原生用户状态键，与命名布局、侧栏选择和快捷键配置分开。 |
| `option:ThreadItemOutputGetQuery.expected_blob` | 读取者选定的精确保留输出文件。若输出已更换，此页会在读取字节前被拒绝，避免混合不同输出的分页。 |
| `option:OutputRef.inline_limit` | 已接受的内联输出字节限额。较小的捕获预算会让实时与持久化末尾同样变小；原始总长度仍计算所有已产生的字节。 |
| `option:OutputRef.retained` | 此输出文件保留的精确原始字节区间；仅保留内联文本时为空。保留的文件可能只包含部分原始输出。 |
| `option:OutputRef.retained.offset` | 保留区间在原始输出中的起始字节位置。此边界之前的字节可能未保留。 |
| `option:OutputRef.retained.len` | 实际保留的文件字节数，不含人为插入的缺失标记。起始位置加长度表示不含在内的原始结束边界。 |
| `option:SessionLost.uncertain` | 会话级报告表示原生结果未知时，输入会保留。连接器重启后若无法恢复已识别的轮次，会单独报告该轮次失联，Orch 随即结束该轮次。 |
| `option:EverythingListQuery.resource_rows` | 返回紧凑的表格行，并在每页中为各个归属位置附带一次可读的共享摘要。打开资源时再读取其完整配置。 |
| `option:EverythingListQuery.resource_details` | 未请求表格行时，返回资源的完整视图。需要展示完整配置的选择器使用此选项。 |
| `option:SearchListQuery.source_details` | 仅读取此流式查询的回执游标中已记录的索引事实；不重新搜索，也不请求模型。读取时重新检查当前搜索权限和可共享名称。 |
| `option:SearchListQuery.source_offset` | 继续读取同一查询已记录的索引来源。使用返回的下一偏移；原生序号的间隔不代表省略了来源。 |
| `option:SearchListQuery.source_filter` | 分页前，按当前共享的范围名称或已记录的模型名称筛选索引来源。不搜索未共享的名称或标识符。 |
| `option:AuthThreadOpening.session` | 打开线程地址时，POST /auth/session/thread/{id} 恢复浏览器会话。没有会话 Cookie 时返回 null。 |
| `option:AuthThreadOpening.opening` | 已登录读者获得的线程首屏数据。无法读取该线程时，此字段返回相应错误。 |
| `option:ThreadOpeningGetQuery.limit` | 此次首屏读取的正数条目数，覆盖解析后的 threads.opening_items 默认值。较早条目仍可通过历史游标继续读取。 |
| `option:ThreadOpeningGetQuery.continuation_id` | 选择此次读取的历史延续，不改变当前执行的延续。 |
| `option:ThreadOpeningGetQuery.projection_revision` | 指定已知的历史投影版本。版本过旧时，返回 sync.complete=false 的原生重置和当前选择器；使用该选择器可重新读取当前窗口。 |
| `option:ThreadModelParameter.options_partial` | 为真表示仅提供首屏参数特征和当前或可见历史轮次的模型。打开控件时，再读取完整参数以取得所有选项、帮助及表单。 |
| `option:ThreadOpening.screen` | 此实际线程独立的活动与计划答复信息。若该部分读取失败，通过 GET /threads/{id}/screen-facts 重试，无须加载模型元数据或整份设置。 |
| `option:ThreadOpening.parameters` | 首屏使用的当前值及来源、默认值信息。审批选项、审核模型详情、子代理控制和端口工具信息，在打开相应控件时通过完整的 GET /threads/{id}/parameters 读取。 |
| `option:ThreadComposer.approval_default` | 原生审批选项由当前线程之外的位置提供时为 true。关闭的审批控件无需读取整份设置，即可显示默认值标记。 |
| `option:ThreadComposer.sandbox_mode` | 紧凑控件所示的有效原生沙箱值；未报告时为 null。完整选项、锁定信息和可写版本在打开控件后读取。 |
| `option:ThreadComposer.sandbox_default` | 沙箱选项由当前线程之外的位置提供时为 true。读取设置面板之前，紧凑控件仍保留默认值标记。 |
| `option:ThreadScreenFacts.silent_after_secs` | 生效的 thread.silent_after_secs 秒数，用于运行中线程的静默提示；未由位置覆盖时使用注册表默认值。 |
| `option:ThreadScreenFacts.plan_answered_by` | 生效的 thread.plan_answered_by 策略，可为 writers 或 managers。计划答复启用前读取此策略，与模型选项分开。 |
| `option:Feed.provider_row` | provider-row:{id} 实时流提供与线程首屏相同的紧凑提供方事实，每次送达前重新检查提供方查看权限。不携带模型、来源配置或私有登录进度；提供方页面仍使用完整资源流。 |
| `option:ProviderRowEvent.provider` | changed 事件中的当前线程提供方事实。为 null 时撤回先前事实，表示已失去原生查看权限。 |
| `option:AccessOptions.models` | 此使用权限涵盖的模型。 |
| `option:InvitationCreateBody.expires_in_minutes` | 邀请有效时长，以整分钟计。默认时长和最长时长分别由 invitations.default_minutes 和 invitations.max_minutes 决定；到期时刻向下取整到显示的分钟。策略默认时长超出公开时间格式时，到期点取 9999 年的最后一分钟；明确指定的时长若超出该表示范围则会被拒绝。 |
| `option:OrgTreeGetQuery.parent_id` | 仅读取指定根组织中该节点的直接子项。续读位置绑定分支和读者；分支使用快照读取，不能同时指定 since 序号。 |
| `option:OrgTreeGetQuery.limit` | 层级页面的正整数条数。分支默认采用 navigation.tree_page_size，也允许明确请求更大的页面；整树读取保留通常的同步批量默认值。 |
| `option:AccessOptions.services` | 此使用授权允许的已启用提供方服务，可按名称、系列或分组指定。省略时允许所有已启用服务；它与生成模型列表相互独立。 |
| `option:AccessOptions.efforts` | 此使用权限涵盖的推理强度。 |
| `option:AccessOptions.where` | 被拒绝的使用所针对的环境，或将要创建的环境所在的主机或机器；它说明提供方的条目没有覆盖到哪里。 |
| `option:BindingOptions.viewer_default` | 新建会话的当前查看者默认值及允许公开的来源；重新绑定现有会话时可缺省。 |
| `option:BindingOptions.environment_readers` | 选定已有环境对预期会话查看者的开放情况；新环境在创建事务中检查。 |
| `option:BindingOptions.defaults` | 设置决定的默认值。 |
| `option:BindingOptions.providers` | 每个提供方及其模型和智能体。 |
| `option:BindingOptions.environments` | 当前读者可管理的已有环境，当前话题的环境优先。 |
| `option:BindingOptions.environment_default` | 未另行选择时，新会话如何选择环境。 |
| `option:BindingOptions.default_environment` | 最近一层设置指定的默认环境（无论设在哪一层），以及当前读者无法使用它时的原因；没有设置指定默认环境、或重新绑定现有会话时缺省。 |
| `option:BindingOptions.source` | 已安全解析的仓库设置及仓库地址的来源位置。 |
| `option:BindingOptions.access` | 在这里创建的会话最初采用的权限页。 |
| `option:BindingOptions.agent_access` | 此创建上下文可选的智能体权限；重新绑定不会创建新贡献。 |
| `option:BindingOptions.new_environment` | 现在为此话题创建环境时，谁能查看、谁能管理。 |
| `option:BindingOptions.can_create` | 当前读者是否可以创建新环境。 |
| `option:BindingOptions.hosts` | 可创建环境的主机、容量、可用性及原因，并标出调度器选择；无法创建时为空。 |
| `option:BindingOptions.machines` | 当前读者自己的已连接机器，通过其目录环境运行会话。 |
| `option:ChangeOptions.active_turn` | 如何处理正在运行的轮次。 |
| `option:ChangeOptions.wait_minutes` | 等待运行轮次的分钟数；超时后停止，不作改变。 |
| `option:ChangeOptions.verify_timeout_secs` | 健康检查的超时时间，单位为秒。 |
| `option:ChangeOptions.keep_previous` | 保留旧侧内容，不安排移除。 |
| `option:ChangeOptions.stop_processes` | 从其他会话仍在使用的环境切换出去时，结束本会话在那里的进程。 |
| `option:AppliedChangeOptions.active_turn` | 如何处理正在运行的轮次。 |
| `option:AppliedChangeOptions.wait_minutes` | 等待运行轮次的分钟数；超时后停止，不作改变。 |
| `option:AppliedChangeOptions.verify_timeout_secs` | 健康检查的超时时间，单位为秒。 |
| `option:AppliedChangeOptions.keep_previous` | 保留旧侧内容，不安排移除。 |
| `option:AppliedChangeOptions.stop_processes` | 从其他会话仍在使用的环境切换出去时，结束本会话在那里的进程。 |
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
| `option:protocols.openai-responses.native_tools.image_generation.request.model` | 托管工具使用的图片模型，与对话模型分开选择。应选择端点支持且符合选项限制的模型。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.action` | 选择 auto、generate 或 edit。auto 由工具判断生成还是编辑；编辑仍需要请求中提供的图片引用。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.background` | 选择自动、不透明或透明背景。所选图片模型和输出格式必须支持该背景。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.output_compression` | 提供方对 JPEG 或 WebP 输出采用的压缩设置，范围为 0 到 100，仅在支持时生效；不会改变请求的图片数量。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.output_format` | 请求的图片编码：PNG、JPEG 或 WebP。这不能证明返回字节的实际格式；结果保留实际报告的格式。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.partial_images` | 请求 0 到 3 张流式预览。预览不算另一张已完成图片；提供方可能将预览工作计入令牌用量。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.quality` | 请求的绘制质量。只能使用所选图片模型及其选项限制支持的级别。 |
| `option:protocols.openai-responses.native_tools.image_generation.request.size` | 请求的尺寸或自动尺寸，必须在所选图片模型支持的范围内；不会覆盖提供方限制。 |
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
| `option:ModelParameters.purposes` | 提供方管理者明确填写的模型用途会替代发现结果：文本生成、向量嵌入、图像生成、语音转写、语音生成、重排序或结构化决策。省略时采用已报告用途；空列表表示不为任何工作提供此模型。 |
| `option:ModelRecent.provider_id` | 此人所选模型的提供方；其他提供方的同名路由模型是不同选项。历史记录不会授予提供方访问权。 |
| `option:ModelRecent.model_name` | 所选模型的完整路由名。明确选择及已获准的原生使用会更新个人记录；仅打开选择器不会更新。 |
| `option:ModelRecents.recent` | 此人在不同设备及线程上使用的模型标识，按服务器接受顺序排列；绝不包含其他人的历史。选择器只匹配当前适用且可见的选项。 |
| `option:ModelRecents.recent_count` | “最近使用”分组采用的 interface.recent_models 当前默认数量；其余模型保持原有顺序，仍可搜索。 |
| `option:PersonalState.thread_setup` | 与线程创建在同一事务中保存的最近一次成功配置；复用前重新验证。位置的明确默认值及锁定优先，环境仅在原位置复用。创建失败不会替换此值。 |
| `state:PersonalStateKey.thread_setup` | 调用者自己的跨设备成功配置偏好，不是他人的记录，也不授予资源使用权。 |
| `state:UserEvent.model_recents` | 此人的模型历史已更新；打开的选择器重新读取个人顺序，不会替换其他偏好。 |
| `option:ModelOffered.purposes` | 模型列表报告的用途。共享列表不代表每个条目都能对话；这些事实与已开放的速度、费用及访问权分开。 |
| `option:ProviderModel.purpose` | 模型及当前端点用途的只读事实；不会改写可编辑参数，也不会使未变化的模型编辑器被误判为过期。 |
| `option:ModelOption.purpose` | 线程或提问模型选项上的同一份当前用途事实，不包含提供方配置或凭据。 |
| `option:ModelPurposeInfo.declared` | 管理者设置的用途优先。否则依次依据模型列表、可识别的模型系列或路由名以及专用端点判断。用途未知的模型仍可选择，并标注“用途未报告”。 |
| `option:ModelPurposeInfo.reachable` | 当前端点声明能够承载的工作类型。覆盖模型用途不会凭空增加接口；未知接口不会产生猜测出的限制。 |
| `state:ModelPurpose.generation` | 对话文本、代理工作轮次、提问和摘要。 |
| `state:ModelPurpose.embedding` | 用于语义检索的文本向量。 |
| `state:ModelPurpose.image_generation` | 图像生成，不是对话文本。 |
| `state:ModelPurpose.transcription` | 将语音转写为文本。 |
| `state:ModelPurpose.speech_generation` | 生成语音，不是对话文本。 |
| `state:ModelPurpose.rerank` | 对已有的检索结果重新排序。 |
| `state:ModelPurpose.decision` | 从给定的结构化选项中作出决策；仅此用途不表示支持对话生成。 |
| `option:providers.presets.*.credential_hints` | 公开的密钥格式提示，用来建议此预设及凭据用途；不会验证密钥或启用服务。保存后首次正常读取会确认提供方信息。 |
| `option:providers.presets.*.credential_hints.*.prefix` | 密钥格式的公开前缀，直接按原文填写。匹配只会建议选项，完整密钥在保存前仍留在表单中。 |
| `option:providers.presets.*.credential_hints.*.role` | 建议的用途：模型访问，或数据源的管理凭据。客户端不认识的用途需要由使用者明确选择受支持的用途。 |
| `option:providers.presets.*.credential_hints.*.field` | 可选的已声明秘密字段名，用来将建议的管理凭据填入所选信息数据源。该字段必须由数据源提供，并与模型访问凭据分开。 |
| `option:TranslationOptions.allowed` | 当前所属位置的策略是否允许所选组合进行格式转换。 |
| `option:TranslationOptions.output_tokens` | 请求和模型均未给出输出额度时使用的已记录默认值。 |
| `option:ProtocolTranslation.from` | 所选转换中调用方或代理程序使用的接口。 |
| `option:ProtocolTranslation.to` | 实际发送给上游的接口。 |
| `option:ProtocolTranslation.title` | 随所选组合显示的转换器名称。 |
| `option:ProtocolTranslation.losses` | 这次转换中保留、近似处理与省略的行为。 |
| `option:ConversionLosses.preserved` | 跨接口时含义保持不变的行为。 |
| `option:ConversionLosses.approximated` | 以近似或简化形式保留的行为，包括自定义工具语法与缓存。 |
| `option:ConversionLosses.dropped` | 转换中没有对应形式的功能；选择组合前会说明。 |
| `option:Isolation.translation` | 已报告的所选转换及其限制；没有该项表示原生接口或尚未报告。 |
| `option:Resolution.translation` | 随能力解析一同记录的实际生成接口转换。 |
| `option:Provider.translated_protocols` | 提供方额外可用的转换接口，与原生端点分别列出。 |
| `option:UpstreamSide.model` | 为本次转换请求记录的模型声明和已报告最大输出量。 |
| `option:UpstreamSide.output_tokens` | 仅在请求和模型均没有输出额度时使用的已记录默认额度。 |
| `option:UpstreamSide.session` | 不含原会话标识或人员信息的稳定不透明会话缓存键。 |
| `option:ProviderStart.translation_output_tokens` | 启动提供方时解析并记录的转换默认输出额度。 |
| `option:Provider.relay` | 密钥提供方明确设置的公开 API 转发选项，仅向当前配置读取者显示。 |
| `option:ProviderRelayOptions.enabled` | 允许直接模型 API 调用，默认关闭。 |
| `option:ProviderRelayOptions.models` | 向公开调用开放的已启用模型的完整名称；空列表表示不开放任何模型。 |
| `option:ProviderRelay.lease` | 本次公开调用获准使用的模型租约，不含模型凭据。 |
| `option:ProviderRelay.relay_id` | 绑定到命令的内存交换，先传递请求，再传递响应。 |
| `option:ProviderRelay.path` | 公开 SDK 基址下该接口允许的请求路径。 |
| `option:ProviderRelay.headers` | 获准传递的接口请求头，不含 Orch 凭据。 |
| `option:ProviderRelay.request_bytes` | 规范化请求的准确长度，用于限定输入阶段。 |
| `option:ProviderRelay.request_hash` | 访问上游前校验的请求摘要。 |
| `option:ProviderRelay.idle_secs` | 已记录的内存交换静默额度。 |
| `option:ProviderRelay.retry_ms` | 已记录的临时故障重试间隔。 |
| `option:ProviderRelayHead.status` | 上游的实际 HTTP 状态。 |
| `option:ProviderRelayHead.headers` | 可重复的响应头字节，已移除连接、凭据与 Cookie 请求头。 |
| `option:ProviderRelay.max_request_bytes` | 同一已记录公开请求设置也限定原生发出的请求，提高额度无需另调连接器设置。 |
| `state:UsagePurpose.relay` | 经密钥提供方发出的直接模型 API 调用，计入实际调用者；额度的用途筛选为空时也包括 API 转发。 |
| `option:ProviderRelay.start` | 当前提供方配置和仅发给凭据保管连接器的凭据引用，不含 Orch API 凭据或模型输入。 |
| `option:ProviderUpdateBody.relay` | 替换密钥提供方的公开 API 转发开关和开放模型完整名称，沿用版本校验和配置读取权限。 |
| `option:ModelParameters.request_options.common` | 通过相同端点、分组和模型层合成的通用缓存与结构化输出选项规则。 |
| `option:ModelGroupOptions.request_options.common` | 现有命名模型分组的通用功能规则。 |
| `option:EndpointOptions.request_options.common` | 实际选中的原生端点上的通用功能规则。 |
| `option:common.cache` | 已声明的缓存键与保留时间控制。各接口提供其原生字段；无法表示的成员或时间值会说明原因并拒绝。 |
| `option:common.structured_output` | 已声明的输出结构；仅在原生接口能表示时使用可选名称与严格模式。 |
| `option:LeaseOpen.request_options.common` | 来自同一通用规则的显式线程选项；它们优先于重叠的格式专用选项，但不能重新开放被禁止的值。 |
| `option:FeatureBinding` | 接口将通用功能映射为原生 JSON 路径、辨别字段和创建默认值的数据声明。 |
| `option:FeatureField` | 通用功能成员的原生路径、固有取值规则、可用选项和事实默认值。 |
| `option:common.structured_output.name` | 通用选项中保留的逻辑结构名称；接口有原生结构名称字段时也会发送。 |
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
| `option:Problem.ceiling` | 设置写入被拒绝时，返回判定当时可读取的继承上限及其来源（SettingInheritance）。按操作限定的密钥只收到字段拒绝原因。 |
| `option:ResolvedSetting.inherited` | 在省略此位置自身贡献后生效的值及其来源；读取者具有位置上下文时提供。保存的选择仍然是自身贡献。 |
| `option:SettingInheritance.value` | 继承选项给出的值；null 表示没有任何范围提供该值。它由相同的原生合并和默认规则计算。 |
| `option:SettingInheritance.source` | 提供继承值的位置或声明的默认来源；隐藏名称仍遵守现有隐私边界。 |
| `option:ThreadInterruptBody.turn_id` | 调用方最后看到的轮次。停止操作针对会话中当前正在运行或准备的工作。如果指定的轮次刚刚结束，仍可暂停其队列。 |
| `option:ThreadStop.state` | 停止请求的即时确认：仍有工作等待结束时为 stopping，目标已结束时为 stopped。已接受的请求会在响应后继续执行。 |
| `option:ThreadStop.requested_at` | 发出停止请求的时间；停止耗时从此时起计算，包括等待其他变更完成的时间。 |
| `option:ThreadStop.turn` | 确认响应中所指定轮次的当前记录。停止尚未创建轮次的准备过程时不提供此字段。 |
| `option:Thread.stopping` | 独立编号的已接受停止状态。普通线程行仍在等待写入时，各处停止控件与活动行也读取这一状态。尚未接受过原生停止请求时省略。 |
| `option:ThreadStop.stopping` | 停止答复中的同一状态。只读程序若已返回正式的已结束轮次，无需单独的观察值。 |
| `option:ThreadStopping.version` | 按提交顺序递增的停止状态版本，与会话历史序号分开。旧观察值不能覆盖新值。 |
| `option:ThreadStopping.requested_at` | 共享停止计时的开始时间，在正式结束状态到达前保留。 |
| `option:ThreadStopping.pending` | 接受后的应用、取消或被中断轮次仍在等待时为真。完成观察值不会使旧活动重新出现。 |
| `option:ThreadStopping.thread_seq` | 与此观察值对应的正式会话进度。控件在该进度到达前保持停止状态。 |
| `option:ThreadStopping.state_seq` | 此观察值所对应的正式会话状态行。仅收到更新的排队输入或轮次答复，并不表示活动状态行已到达。 |
| `option:Turn.interrupted_at` | 有人要求停止此轮次的时间。无人停止时省略；它与最终完成时间分别记录。 |
| `option:MarkerSessionReopened.reason` | 轮次开始前重新打开会话的已记录原因，例如指令变化或程序重启。可观察的指令文件没有变化时沿用原会话。 |
| `option:PortOpening.refused_until` | 同一已拒绝效果可再次请求的已保存时间。开放项仍保持拒绝状态；此元数据不会重新开放或创建另一张审批卡。 |

| `option:ModelOptionReason.shared_for_ask_only` | 仅共享给 Ask，无法选择用于会话。 |
| `option:ModelOptionReason.shared_for_other_work_only` | 仅共享给其他工作，无法选择用于个人 Ask。 |
| `option:AskModelOption.unavailable_reason.shared_for_ask_only` | 仅共享给 Ask。 |
| `option:AskModelOption.unavailable_reason.shared_for_other_work_only` | 仅共享给其他工作。 |

| `option:OrgTreeGetQuery.path_to` | 只读取一条祖先路径，不读取同级节点。 |
| `option:OrgTreeGetQuery.q` | 在服务器搜索名称；空值返回有界的第一页。 |
| `option:OrgTreeGetQuery.kind` | 按原生节点类型筛选分页地点选择器。 |
| `option:OrgTreeGetQuery.below_id` | 此节点及其内部的地点。 |
| `option:OrgTreeGetReply.ancestors` | 分页结果之外，经授权的结构祖先。 |
| `option:OrgTreeGetReply.feed` | 仅对应此页的当前有界元数据订阅。 |
| `option:TreeNode.folded` | 可读子节点数量及分支内未读、提及、待决事项和当前工作状态的汇总。 |
| `option:TreeFold.archived` | 已获准查看的归档路径单独提供数量与待关注信息。显示归档时合并此组，隐藏归档时只使用普通字段；两组都不会包含缺少聊天查看权限的信号。 |
| `option:TreeNode.chat` | 节点的已知聊天标识不会授予聊天查看权限。缺少该独立查看权限时，消息位置为 null。 |
| `option:TreeNodeChat.seq` | 节点的已知聊天标识不会授予聊天查看权限。缺少该独立查看权限时，消息位置为 null。 |
| `option:DocumentTreeView.hierarchy` | 当前用户多个根组织的有界层级分页共用一个元数据订阅；每页均通过原生根组织授权，不读取完整树或重放持久历史。 |
| `option:DocumentTreeData.hierarchy` | 仅完整替换正在订阅的单个层级分页。 |

| `option:BindingProvider.setup` | 会话的提供方选择器显示的当前设置问题。订阅令牌过期时，优先提示重新登录。 |
| `option:BindingProviderSetup.reason` | 根据提供方最近报告的凭据及已配置的模型，显示登录已过期、需要登录或未启用模型。 |
| `option:BindingProviderSetup.can_sign_in` | 订阅登录过期时，提供方管理者可打开凭据页面重新登录。 |
| `option:BindingProvider.reason.shared_for_ask_only` | 仅共享给 Ask；仍可请求其他工作用途。 |

| `option:OrgTreeGetReply.page_size` | 本次读取的实际分页默认值；一页之内的小树保留熟悉的初始展开方式。 |

| `option:DocumentTreeData.hierarchy.window` | 客户端为此次替换的单个分页选择的路由编号，不授予权限。 |
| `option:DocumentTreeData.hierarchy.problem` | 此分页现有的拒绝代码，其他分页继续更新；界面使用通常的本地化原因。 |


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
| `option:Problem.database_wait` | 此请求已知原生数据库超时的安全事实；其他错误不含此成员。 |
| `option:DatabaseWait.phase` | 已捕获的获取连接、执行语句或事务闲置超时阶段；不根据一般取消错误推断。 |
| `option:DatabaseWait.applied_limit_ms` | 当时实际期限，单位毫秒；null 表示连接池配置尚未报告，不表示零或无限期等待。 |
| `option:DatabaseWait.service` | 回答请求的服务，目前为 api；不含数据库地址、SQL 或配置来源。 |
| `option:DatabaseWait.occurrence_id` | 失败请求的标识符，与 request_id 一致，不指代数据库对象。 |
| [等待数据库连接已超时](state-reference.md#state:DatabaseWaitPhase.acquire) | 原生连接池等待已超时。 |
| [数据库语句执行超时](state-reference.md#state:DatabaseWaitPhase.statement) | PostgreSQL 明确因语句期限已过而取消了执行。 |
| [数据库事务已超时](state-reference.md#state:DatabaseWaitPhase.idle_transaction) | PostgreSQL 明确因闲置期限已过而结束了事务。 |
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
| [已预留](state-reference.md#state:ProviderTrialState.reserved) | 一个确切输入持有这次唯一试运行的预留。 |
| [正在运行](state-reference.md#state:ProviderTrialState.running) | 预留的原生轮次确实正在运行。 |
| [已使用](state-reference.md#state:ProviderTrialState.consumed) | 已接纳的试运行已结束，或确认恢复后其预留已使用；不表示服务方一定接受。 |
| [未预留](state-reference.md#state:ProviderTrialState.not_reserved) | 此次观察到的检查没有活动的试运行预留。 |
| `option:AnnouncementPayload.delivered_as` | 接收方实际排队、即时引导、中断或延后通知的结果，与请求方式分开。 |
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
| [就绪后在后续轮次返回](state-reference.md#state:CallReturnState.when_ready) | 就绪后在后续符合条件的轮次返回，不承诺启动。 |
| [继续使用故障切换模型](state-reference.md#state:CallReturnState.stay_on_failover) | 继续使用已接纳的故障切换路径，直至普通更改。 |
| [正在返回](state-reference.md#state:CallReturnState.returning) | 引擎确已开始返回原路径。 |
| [尚未报告](state-reference.md#state:CallReturnState.unreported) | 未记录原路径或返回事实。 |
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
| `option:SubFrame.window` | 在已有层级元数据订阅中逐个更新分页；保留其他分页，不占用额外订阅名额。 |
| `option:HierarchyWindow.kind` | query 注册连续的选择器片段；remove 释放该分页。 |
| `option:HierarchyWindow.window` | 仅在保留该分页期间使用的连接内路由编号。 |
| `option:HierarchyWindow.data` | UTF-8 选择器 JSON 的连续 URL 安全无填充 base64 片段；每帧均小于服务器公布的字节边界。 |
| `option:HierarchyWindow.complete` | 最后一个片段到达后校验并跟随有界选择器；无效选择器仅影响该分页。 |
| `option:ReadyFrame.max_client_frame_bytes` | 单个客户端 JSON 帧的原生字节边界，包含封装；元数据选择器按此拆分。 |
| `option:HierarchyWindow.root` | 此分页选择器的实际根组织，不授予查看权限；每次更新均通过与 HTTP 相同的原生根组织授权。 |
| `option:TreeFold.has_children` | 本可见性类别中存在经原生查看权限核验的直接子项；无需统计或保留静默子树即可展开。 |
| `option:TreeFold.small_tree` | 根组织中经权限核验的后代能放入此次初始分页。该可选初始事实保留小树展开形态；实时刷新不重复探测，大树仍可逐层完整浏览。 |
| `option:OrgTreeGetQuery.include_threads` | false 会在填充地点分页前排除线程；省略时包含线程。不与 path_to 合用，其生效值绑定到游标。 |
| `option:OrgTreeGetQuery.sort` | 在分页前排列整个符合条件的集合。navigation 依次采用当前用户保存的选择和组织默认值，也可明确选择 newest、changed、name、custom；省略时按原生标识排列。 |
| `option:OrgTreeGetQuery.locale` | en 或 zh-CN 选择导航的数字及基本字母名称排序；API 进程的区域设置不会改变它。 |
| `option:OrgTreeGetReply.ordering` | 返回生效排序及已返回标识之间的安全局部同级顺序，不暴露原始偏好映射。 |
| `option:HierarchySort` | navigation 解析保存的偏好；newest、changed、name、custom 直接指定一种排序。 |
| `option:HierarchyOrderBy` | 此分页实际采用的 newest、changed、name 或 custom 排序。 |
| `option:HierarchyLocale` | en 和 zh-CN 使用各自的数字名称排序，并忽略大小写及重音差异。 |
| `option:HierarchyOrdering.by` | 实际采用的排序，包括读者无需读取整个设置页即可遵循的组织默认值。 |
| `option:HierarchyOrdering.revision` | 用于撤下旧游标及局部同级排序约束的不透明排序与语言修订标记。 |
| `option:HierarchyOrdering.known_order` | 仅包含本次响应中可查看或可检查标识之间的相对顺序，不包含私密名次或其他已保存标识。 |
| `option:DirListing.revision` | 本次文件夹观察的标识；稳定的小文件夹使用不可变快照，宽大或持续变化的文件夹则继续同一原生读取器。每页在自身窗口内排序，并说明观察时间范围。 |
| `option:FsListPageResult.revision` | 按字节分页时保留同一原生文件夹观察标识，不代表整个文件夹在某一瞬间的完整切面。 |


### 提供方操作事实

| Reference | 含义 |
| --- | --- |
| `option:ProviderRuntimeSupport.retry_after_secs` | 已解析的创建失败重试间隔；手动启动可立即重试。 |
| `option:Provider.facts_refresh` | 按相同持久化条件计算的当前刷新资格；打开页面不会读取厂商。 |
| `option:ProviderRefreshEligibility.eligible` | 显式刷新现在是否可获取读取机会。 |
| `option:ProviderRefreshEligibility.next_at` | 由读取尝试、部署间隔及来源最短间隔计算的下次资格时间；尚未尝试或时间不可表示时不提供。 |
| `option:ProviderRefreshEligibility.source_minimum` | 复用由来源最短间隔决定，而非部署间隔。 |
| `option:HarnessState.home` | 来自本次进程启动的目录事实；当前设置不会改写较早报告。 |
| `option:HarnessHome.path` | 实际传给适配器的目录；原生程序自行选择且未报告路径时不提供。 |
| `option:HarnessHome.source` | 实际启动时记录的运行位置设置、配置档设置、托管绑定或保留的原生默认值。 |
| `option:AgentStartResult.home` | 原生目录事实；复用现有进程时同样返回。 |
| `option:ProviderStart.upstream_policy` | 由同一设置解析器为原生及公开模型调用记录的交换策略。 |
| `option:UpstreamPolicy.head_secs` | 等待响应头或完整响应的秒数。 |
| `option:UpstreamPolicy.idle_secs` | 流式响应连续静默的秒数。 |
| `option:UpstreamPolicy.whole_bytes` | 完整响应及生成结果的字节额度；表示范围仅受可寻址内存约束。 |
| `option:CommandError.details.upstream` | 失败的上游规则及实际秒数或字节数，保留在普通失败原因中。 |
| `state:HarnessHomeSource.runtime` | 明确的运行位置目录设置。 |
| `state:HarnessHomeSource.profile` | 配置档的目录设置。 |
| `state:HarnessHomeSource.managed` | 托管连接器创建或绑定的目录。 |
| `state:HarnessHomeSource.native` | 保留现有原生目录；未知路径保持未报告。 |


### 已记录的放置计算

| Reference | 含义 |
| --- | --- |
| `option:PlaceHost.calculation` | 本放置提议实际记录的调度计算；不提供表示尚未记录。 |
| `option:Job.result.calculation` | 随变更结果保留的实际放置计算；不会补用当前默认值。 |
| `option:RuntimeIsolation.assumptions` | 隔离边界、停止持久性及开销的已记录来源；未记录时不提供。 |
| `option:PlacementCalculation.captured_at` | 调度器决定时记录输入的时间。 |
| `option:PlacementCalculation.declared_totals` | 现有单元声明的处理器、内存和磁盘需求，不含运行时开销，并非实测用量。 |
| `option:PlacementCalculation.applied_totals` | 此调度器容量判断实际采用的合计值。 |
| `option:PlacementCalculation.runtime_overhead` | 单元声明之外的运行时内存；区分未知值和未计入的值。 |
| `option:PlacementCalculation.capacities` | 本次计算采用的容量报告。 |
| `option:PlacementCalculation.capacities_as_of` | 该容量报告的实际接收时间；未知时不提供。 |
| `option:PlacementCalculation.thresholds` | 记录的满载阈值及传入单元的内存需求。 |
| `option:PlacementCalculation.consequence` | 计算当时实际采用的警告或阻止策略。 |
| `option:PlacementCalculation.sources` | 实际使用的声明和设置层，不包含私有机器策略或隐藏资源名称。 |
| `option:PlacementCalculation.isolation` | 放置前由所选驱动、运行时和模板解析的隔离事实。 |
| `option:PlacementRuntimeOverhead.existing_memory_mb` | 现有单元实际计入的额外内存，单位 MiB。 |
| `option:PlacementRuntimeOverhead.incoming_memory_mb` | 传入运行时类别声明的开销；未知时不提供。 |
| `option:PlacementRuntimeOverhead.incoming_included` | 实际空间判断是否计入传入单元的运行时开销。 |
| `option:PlacementRuntimeOverhead.source` | 传入单元开销来自类别声明还是未知时的回退值。 |
| `option:PlacementThresholds.full_percent` | 本次判断实际使用的 hosts.full_percent 值。 |
| `option:PlacementThresholds.incoming_memory_mb` | 剩余空间比较使用的传入单元内存声明。 |
| `option:PlacementCalculationSources.needs` | 提供传入单元每项资源需求的解析分支。 |
| `option:PlacementCalculationSources.capacity_unreported` | 未报告的容量成员；数字回退值并不表示实测为零。 |
| `option:PlacementCalculationSources.template_revision` | 决定时实际读取的模板修订号。 |
| `option:PlacementCalculationSources.full_percent` | 提供满载百分比的系统设置。 |
| `option:PlacementCalculationSources.consequence` | 决定警告或阻止的主机选择或系统设置。 |
| `option:RuntimeAssumptions.boundary_source` | 有效隔离边界的来源。 |
| `option:RuntimeAssumptions.stop_persistence_source` | 停止持久性结论的来源；未报告不表示数据会丢失。 |
| `option:RuntimeAssumptions.overhead_source` | 运行时内存开销的来源；未知值不会成为实测零值。 |
| `state:RuntimeAssumptionSource.runtime_declaration` | 所选运行时类别的声明。 |
| `state:RuntimeAssumptionSource.driver_declaration` | 驱动的隔离声明。 |
| `state:RuntimeAssumptionSource.unknown_runtime_fallback` | 未能解析运行时，实际采用实现中的回退值。 |
| `state:RuntimeAssumptionSource.not_reported` | 没有声明提供此事实。 |
| `state:PlacementNeedSource.resolved_needs` | 明确或继承的需求设置。 |
| `state:PlacementNeedSource.template_needs` | 模板的需求声明。 |
| `state:PlacementNeedSource.resolved_resources` | 有效资源限制，包含模板上限。 |
| `state:PlacementNeedSource.template_resources` | 模板的资源声明。 |
| `state:PlacementNeedSource.not_reported` | 未声明该值；调度器实际采用原有的零回退值。 |
| `state:PlacementSettingSource.system` | 系统设置，包含其声明的默认值。 |
| `state:PlacementSettingSource.host` | 主机明确设置的满载处理选择。 |
| `option:Job.calculation` | 随任务保留的实际放置计算，包含后续失败或取消的任务。旧记录保持未报告，不会使用当前设置补全。 |
| `option:HarnessStatus.home` | 普通状态报告携带的实际进程目录事实，包含受监督恢复及连接器恢复。没有新事实不会选择新目录。 |

| 引用 | 含义 |
| --- | --- |
| `option:SummarySnapshot.result_facts` | 可读段落实际记录的原生输出调整及请求用量；不使用后来的设置重建，未记录时显示尚未报告。这些事实与段落受相同的当前来源权限约束。 |
| `option:SummaryResultFacts.flattened` | 此次实际输出约束是否规范了空白；null 表示未记录。 |
| `option:SummaryResultFacts.first_line_only` | 是否只保留多行回答中首个非空行；null 表示未记录。 |
| `option:SummaryResultFacts.removed_characters` | 格式整理后，按此次实际字符额度删减的字符数。整段拒绝的交接内容不会冒充有效的缩短回答。 |
| `option:SummaryResultFacts.unknown_tags` | 本次记录的未知标签名称省略数。 |
| `option:SummaryResultFacts.duplicate_tags` | 本次记录的、已被保留的重复标签名称省略数。 |
| `option:SummaryResultFacts.over_budget_tags` | 本次记录的、超出所选额度且并非重复项的已知标签数量。 |
| `option:SummaryResultFacts.rejected_whole` | 交接内容是否被整段拒绝；这些事实不保留被拒绝的原始命令或路径前缀。 |
| `option:SummaryResultFacts.captured_usage` | 随结果保留的规范请求用量，区分实际报告的零与未知；不会把缺失计数当成报价或退款。 |
| `option:SummarySnapshot.refresh_proposal` | 刷新前只读查看当前模型、用量归属及渲染后请求估算；不预留资源，也不会开启自动任务。 |
| `option:SummaryRefreshProposal.revision` | 所观察选项及来源输入的版本，不是接纳或付款凭证。 |
| `option:SummaryRefreshProposal.usage_owner` | 实际手动请求者的安全人员引用，不从资源所有者推断。 |
| `option:SummaryRefreshProposal.input_estimate_tokens` | 实际渲染提示词的词元估算，与基础价格及字符目标分开。 |
| `option:SummaryRefreshProposal.output_allowance_tokens` | 实际输出预留额度，包含适用的推理空间；这是上限，不是报价。 |
| `option:GoalReport.helper` | 记录的辅助模型接纳信息及实际结果；旧记录或仅由智能体报告的记录不会凭空获得辅助模型事实。 |
| `option:GoalHelperReceipt.checked` | 仅在已解析实际辅助模型回答时为 true；已记录但尚在等待或未检查的尝试为 false，未记录则为 null。人工接受报告不会凭空产生辅助模型结果。 |
| `option:GoalHelperReceipt.input_allowance_tokens` | 随此次尝试记录的实际接纳输入额度，不使用后来的默认值。 |
| `option:GoalHelperReceipt.output_allowance_tokens` | 实际接纳的输出额度，与说明字符数分开。 |
| `option:GoalHelperReceipt.last_turn` | 所选上一轮材料的实际覆盖范围；null 表示尚未报告。 |
| `option:NodeBase.automatic_fields` | 按当前节点或会话权限读取的独立标题、标签结果及持久调度事实；人工控制权仍然优先。 |
| `option:SummaryAttemptFacts.captured_usage` | 即使结果被丢弃，也保留规范报告的用量；缺失测量值不会变成零。 |
| `option:AutomaticFieldSchedule.due_at` | 已记录的触发到期时间，与最短间隔或重试的可运行时间分开。 |
| `option:AutomaticFieldSchedule.next_at` | 实际记录的间隔或重试可运行时间，不是浏览器根据默认值推算的时间。 |
| `option:AutomaticFieldSchedule.last_attempt_at` | 实际的上次尝试时间，包含失败；不是上次成功时间。 |
| `option:AutomaticFieldSchedule.last_success_at` | 此字段实际的上次成功写入时间，不借用其他字段的写入时间。 |
| `option:AutomaticFieldSchedule.retry_ceiling` | 决定该记录时间时捕获的重试间隔上限；设置来源链接到现有控制。 |
| `option:HistoryReport.handoff` | 实际保留的交接事实，与计划估算或完整转录文件的覆盖范围分开。 |
| `option:BindingEnvironment.room` | 来自已连接机器或当前主机的可用 CPU 和内存，以及采样时间。未提供的值表示尚未报告。 |
| `option:PlaceEnvironment.room` | 此运行位置在记录时间测得的可用容量。 |
| `option:PlaceHost.room` | 所选主机最近测得的可用 CPU 和内存。 |
| `option:PlaceHost.running_threads` | 该主机当前环境中正在运行的主线程数。 |
| `option:MachineFacts.room` | 机器的可用内存和测得的空闲逻辑 CPU 时间。 |
| `option:PlaceRoom.cpu_free_millis` | 以千分之一核表示的空闲 CPU 容量。未测量时未知。 |
| `option:PlaceRoom.memory_free_bytes` | 可用内存字节数。零表示测量值为零。 |
| `option:PlaceRoom.observed_at` | 资源采样时间。 |
| `option:Job.handoff_result` | 已完成变更保留的同一交接记录，按当前任务读取权限返回；仅进度读取不包含此记录。未记录的事实仍为尚未报告。 |
| `option:HandoffResult.input_coverage` | 此次有界交接输入中实际记录的逐轮读取及省略决定。 |
| `option:HandoffResult.result_facts` | 实际的输出约束及规范请求用量；不保留被拒绝的原始文本。 |
| `option:HandoffInputCoverage.read_turn_bytes` | 选择材料时使用的单轮字节额度，不使用后来的设置。 |
| `option:HandoffInputCoverage.included_turn_ranges` | 实际以文本纳入的原生轮次闭区间，与较早的索引行及完整转录文件分开。 |
| `option:HandoffInputCoverage.oversized_turn_ranges` | 因超出当时读取额度而整轮省略的原生轮次闭区间。 |
| `option:HandoffInputCoverage.omitted_prefix` | 较早的所选历史未纳入实际的有界输入。 |
| `state:GoalHelperCoverage.none` | 此次辅助模型请求中没有上一轮材料。 |
| `state:GoalHelperCoverage.included` | 所选上一轮已完整纳入。 |
| `state:GoalHelperCoverage.start_omitted` | 为符合实际接纳的额度，省略了所选轮次的开头。 |
| `state:GoalHelperCoverage.oversized` | 该轮超出当时的字节额度，因此整轮省略。 |
| `state:GoalHelperCoverage.items_exceeded` | 该轮超出所选条目额度，因此整轮省略。 |
| `state:SummaryDiscardReason.access_changed` | 发布所依据的权限版本已更改。 |
| `state:SummaryDiscardReason.history_changed` | 发布前所选历史版本已更改。 |
| `state:SummaryDiscardReason.source_changed` | 发布前来源版本已更改。 |
| `state:SummaryDiscardReason.producer_changed` | 发布前该字段的生成来源已更改。 |
| `state:SummaryDiscardReason.archived` | 发布前对象已归档。 |
| `state:SummaryDiscardReason.changed_by_person` | 人工修改获得该字段的控制权，优先于生成结果。 |
| `state:SummaryAttemptOutcome.published` | 结果通过了发布检查；相同的值无需再次写入。 |
| `state:SummaryAttemptOutcome.discarded` | 当前发布条件丢弃了此次结果，当前字段值仍有效。 |
| `state:SummaryAttemptOutcome.unusable` | 原生输出约束拒绝了回答，已记录用量仍计入统计。 |
| `state:SummaryAttemptOutcome.failed` | 生成来源未能完成此次尝试。 |
| `state:AutomaticSkipReason.no_producer` | 未选择自动生成来源。 |
| `state:AutomaticSkipReason.cap_no_room` | 当时的用量上限没有此次尝试所需的余量。 |
| `state:AutomaticSkipReason.not_allowed` | 当时无法取得来源或模型的使用权限。 |
| `state:AutomaticSkipReason.provider_unavailable` | 此次尝试无法使用提供方。 |
| `state:AutomaticSkipReason.call_rule_paused` | 调用规则未安排自动继续。 |
| `state:AutomaticSkipReason.source_unavailable` | 所选来源不可用。 |
| `state:AutomaticSkipReason.unsupported` | 该生成来源不支持写入此字段。 |
| `state:AutomaticSkipReason.failed` | 生成来源此次尝试失败。 |
| [下次更新](state-reference.md#state:AutomaticScheduleState.queued) | 实际接纳的自动任务已排队或被领取；可运行时间并非承诺的启动时间。 |
| [等待重试](state-reference.md#state:AutomaticScheduleState.retrying) | 实际失败的尝试已有记录的可重试时间。 |
| [已跳过 · 未选择摘要器](state-reference.md#state:AutomaticScheduleState.skipped) | 上次尝试已跳过或暂停，当前没有排队任务。 |
| `state:AutomaticScheduleState.idle` | 当前没有排队的自动任务。 |

### 原生审批检查

| 参考 | 含义 |
| --- | --- |
| `option:AgentDescriptor.approval_auto` | 所选程序构建的自动审批验证事实，由服务器决定，配置覆盖不能生成验证。 |
| `option:ThreadComposer.auto_unavailable` | 为真时，已保存的自动审批当前不可用，折叠控件也会显示该状态。 |
| `option:AutoApprovalCheck.machine_id` | 运行原生检查的所选机器。 |
| `option:AutoApprovalCheck.sha256` | 所选程序构建的确切摘要。 |
| `option:ApprovalCheckTarget.machine_id` | 重试检查的目标机器。 |
| `option:ApprovalCheckTarget.sha256` | 只有具有此校验和的构建可以在所选机器上检查。 |
| `option:HarnessCheckQuery.machine_id` | 与构建校验和一起选择重试的机器。 |
| `option:HarnessCheckQuery.sha256` | 与机器一起指定的单个构建校验和。 |
| `option:MachineFactsHarness.sha256` | 连接器读取的已确认程序的校验和。 |
| `option:RunningHarnessBuild.check_machine_id` | 用于检查所选运行中构建的机器。 |
| `option:AutoApprovalCheck.declared` | 程序最初是否声明自动审批；保留此事实以便重新检查通过后恢复选项。 |
| `option:AutoApprovalCheck.state` | 此程序的自动审批尚未验证、检查失败、检查通过或没有可用检查。 |
| `option:AutoApprovalCheck.check_available` | 保留的程序构建是否有可运行的自动审批一致性检查。 |
| `option:ResolvedSetting.approval_choices` | 审批模式的当前验证、策略、审核器及隔离条件，包含置灰选项及其原因。 |
| `option:ApprovalModeChoice.check` | 自动审批不可用时可运行的现有程序检查，仅当前系统管理员可发起。 |
| `option:ApprovalCheckTarget.kind` | 接受检查的保留构建所属的程序。 |
| `option:ApprovalCheckTarget.version` | 通过常规构建检查操作验证的确切程序版本。 |
| `option:Problem.session_end` | 仅在与保留的浏览器凭据精确匹配后返回本次登录会话的结束事实；未知凭据、已清理记录及自动化密钥不返回原因。 |
| `option:SessionEnd.cause` | 原生 expired、revoked 或 account_unavailable 结果；仅访问令牌到期不代表浏览器登录会话结束。 |
| `option:SessionEnd.at_if_known` | 已保存的到期或撤销时刻；未保留结束时刻时省略，绝不用观察时刻替代。 |
| `state:SessionEndCause.expired` | 已证明的浏览器登录会话超过其记录的空闲截止时间。 |
| `state:SessionEndCause.revoked` | 已证明的浏览器登录会话在记录的时刻被明确撤销。 |
| `state:SessionEndCause.account_unavailable` | 已证明的浏览器登录会话所属账户不可用；不暗示未记录的停用时刻。 |
| `option:MachinePorts.loopback_tcp` | 允许个人转发的本机回环 TCP 端口号。默认列表为空，端口号必须大于零。 |
| `option:PortTicketClaims.machine_reach` | 签名的个人 TCP 使用凭据携带显式的整机范围。连接器会依据当前策略检查所选端口。 |
| `option:PortTarget.named_loopback` | 当前读者可访问机器操作者指定的回环端口。建立连接时连接器会核实监听状态。 |
| `option:FileAccess.reach` | 查看端口的默认范围只包括环境监听端口。显式选择整台机器后，还可个人转发机器策略指定的回环 TCP 端口。文件查看和管理条目默认为 roots。machine 必须明确指定，仅覆盖环境自身系统用户可打开的路径；不补授缺少的权限，也不能超过授予者或机器的上限。 |
| `state:FileReach.roots` | 环境的工作区及其挂载，按原生机器解析后的真实目标核验。 |
| `state:FileReach.machine` | 环境自身系统用户可打开的所有原生路径，仍受机器当前上限及原有操作限制约束。 |
| `option:LocalPolicy.environment.files_reach` | 机器自行规定的人类文件访问范围上限：roots 或 machine。上限默认为 machine，不代表已授予文件权限；单个文件条目仍默认为 roots，代理程序的隔离限制另行生效。 |
| `option:ConnectorConfig.environment.files_reach` | 连接器的 environment.files_reach 策略值，由机器在本地执行，并随当前事实报告。 |
| `option:FsAccess.view` | 本次命令获准的原生读取范围；null 不允许读取。 |
| `option:FsAccess.manage` | 本次命令获准的原生修改范围；更宽的读取范围不会扩大它。 |
| `error:files.reach_denied` | 目标超出当前持有人或机器允许的文件访问范围；被拒绝的操作不会打开该文件。 |
| `option:Environment.file_reach` | 环境完整页面上的当前调用者文件访问范围；受限的线程关联读取不提供此事实。 |
| `option:PermissionPage.file_reach` | 环境权限选项旁的当前调用者文件访问范围与机器上限。 |
| `option:FileReachFacts.view` | 受机器上限约束后的当前读取范围；null 不授予读取权限。 |
| `option:FileReachFacts.manage` | 当前修改范围，与读取范围分开；null 不授予修改权限。 |
| `option:FileReachFacts.view_from_terminal` | 同一凭证可用的终端扩大了已有文件查看权限的范围。 |
| `option:FileReachFacts.manage_from_terminal` | 可用终端扩大已有文件管理权限的范围，不补授缺少的操作权限。 |
| `option:FileReachFacts.machine_bound` | 当前机器自身策略上限，独立于指定的权限条目和代理程序隔离范围。 |
| `state:FileSourceKind.machine` | 有意打开的附加根目录之外的原生路径，以环境自身用户访问，不建立整个机器的工作区索引。 |
| `option:FsFetchResult.minimum_reach` | 原生路径解析为这份精确内容证明的最低访问范围，随缓存内容保留，防止相对符号链接把机器范围数据变成根目录数据。 |
| `option:EnvironmentFileListQuery.prefix` | 可选的子项名称前缀，不区分大小写，在分页之前过滤，用于单层路径补全。 |
| `option:FsList.prefix` | 对应的原生目录名称前缀；补全只读取一层，沿用现有的有界观察游标。 |
| `option:ReferencePreviewQuery.path` | 附加根目录内的相对路径或明确的原生绝对地址；实时及保留的文件数据都核验当前读者的文件访问范围及原始原生来源。 |

<a id="resource-pool-targets"></a>

## 资源池目标

| 选项 | 含义 |
| --- | --- |
| `pool` | 可配置的资源组，拥有独立的查看、使用和管理权限。仅有资源池使用权限不会获得成员访问权限。 |
| `pool_own` | 资源池自身继承的路由默认值来源。 |
| `model`, `provider`, `environment`, `host` | 成员资源类型；选择只接受与其一致的类型。 |
| `single`, `pool` | 一个精确目标或一个已配置的资源池；实际成员与原选择意图分别保存。 |
| `ordered`, `weighted`, `round_robin`, `least_cost`, `least_loaded` | 明确的选择规则：保存顺序、按权重随机、轮转、可比较的成本或已报告的负载。 |
| `cpu`, `memory`, `combined`, `threads`, `processes` | 已报告的负载指标；线程和进程使用实际数量，不虚构百分比分母。 |

| 参考标识 | 含义 |
| --- | --- |
| `option:PoolDefinition.kind` | 模型、提供方、环境或主机；创建后不能更改类型。 |
| `option:PoolDefinition.enabled` | 启用通过资源池进行选择和借出；禁用时仍可查看，已保存的 Where 位置集合仍然有效。 |
| `option:PoolDefinition.policy` | 启用前必须明确选择规则。预览不会预留成员。 |
| `option:PoolDefinition.lend_members` | 允许此资源池接收成员当前的“通过资源池”同意，不授予成员直接使用或读取权限。 |
| `option:PoolMember.priority` | 可选优先级；省略时继承默认值。 |
| `option:PoolMember.weight` | 可选正十进制权重；省略时继承默认值。 |
| `option:PoolMemberRule.grouping` | 精确的实时显示分组，包括缺失的归属、所有者、机器或标签元数据；不会只保存已加载页的快照。 |
| `option:PoolLendingPutBody.matcher` | 指定资源池、归属于指定位置的资源池，或由指定人员管理的资源池。每次原生使用重新核验匹配及同意者。 |
| `option:PoolLendingPutBody.options` | 一条完整限制，包括提供方用途、模型、服务、强度、运行方式及 Where，或主机、模板限制；一次请求不能从不同同意条目拼接权限。 |
| `option:PoolLendingPutBody.show_names` | 允许在已获准的资源池场景中显示成员身份，不授予定义读取或一般成员访问权限。 |
| `option:PoolLendingPutBody.expires_at` | 明确到期时间；对外共享仍遵循当前继承的有效期限制，读取不会续期。 |
| `option:PoolUse.provider` | 提供方限制在每一层嵌套资源池中保持完整。 |
| `option:PoolUse.host` | 主机创建、存储、模板及资源限制。 |
| `option:PoolUse.template` | 模板能力和资源限制。 |
| `option:WhereName.environment_pool` | 已有环境的实时结构集合；成员身份不会授予环境使用权。 |
| `option:WhereName.host_pool` | 当前主机成员上的全部环境；选择规则、冷却和禁用资源池不改变已保存的位置集合。 |
| `state:PoolMember.member_hidden` | 当前读者不能查看成员身份；保留已保存的行并隐藏目标。 |
| `option:PoolMemberMetadataBody.enabled` | 只启用或禁用已保存的成员行；原生写入保留隐藏目标。 |
| `option:PoolMemberMetadataBody.priority` | 明确优先级或继承默认值，不替换规则。 |
| `option:PoolMemberMetadataBody.weight` | 正十进制权重或继承默认值，不替换规则。 |
| `option:PoolMemberMetadataBody.position` | 已保存规则的顺序，受已观察到的资源池修订号保护。 |
| `option:PoolMemberPutBody.position` | 省略时，新规则放在所有已保存行之后，已有规则保留位置；明确指定顺序只更改该行。 |
| `option:PoolDeleteBody.expected_revision` | 移除管理者已审阅的资源池；保留共享成员资源和历史回执。必须先替换仍在使用的资源池或 Where 引用。 |
| `state:PoolMember.member_unavailable` | 当前保存的目标或模型提供已不存在；继续读取预览时会重新核验。 |
| `state:PoolMember.request_required` | 已读取可用事实；实际是否可用仍需按已接纳请求的用途、位置、限制和原生能力核验。查看预览不会产生用量。 |
| `option:Pool.target` | 此资源池的标准类型化选择意图；不授予使用权，并与最终接纳的成员分别记录。 |
| `option:PoolPolicy.least_cost` | 在首个可行优先级内比较已知的请求点数估算；未知价格排在已知价格后，并按已保存权重选择。嵌套资源池先遵循自己的规则提出成员，未被选择的轮转不会前进。 |
| `option:PoolPolicy.least_loaded` | 按原生分母比较所选实测指标；缺失数据不可用。综合负载取规范化 CPU 和内存中的较大者，线程、进程仍按数量比较；接纳时继续核验原生容量。 |
| `option:NodeLoad.pids` | 与原生负载同时报告的主机范围实际进程数；缺失表示未知，按数量解释，不虚构百分比分母。 |
| `option:PoolRecoveryBody.expected_revision` | 立即重试所审核的资源池版本。只解除此资源池的本地暂停，保留共享失败事实及未完成原生尝试，不发送探测请求。 |
| `state:PoolMember.member_cooling` | 当前原生失败证据使此成员按资源池自身窗口和冷却规则暂停。 |
| `state:PoolMember.member_trial_in_progress` | 允许的恢复尝试已被实际原生请求占用；查看页面不会发起另一次尝试。 |
| `option:Candidate.target` | 规范的确切模型或后端、提供方、模型池或提供方池。实际选中成员另行存储，并由原生来源重新检查。 |
| `option:FallbackView.pool_kind` | 规则预览中尚未解析的资源池种类，不披露成员或后端配置。 |
| `state:CallRecovery.pool_exhausted` | 在已审核的资源池预算和调用规则内，没有可用的未尝试成员。只有后续明确配置的步骤可以选择其他目标。 |
| `state:PoolMember.member_already_tried` | 此逻辑恢复链已尝试过同一规范成员，包括最初失败的执行；不同的确切位置或配置仍为不同目标。 |
| `option:ResourceLeaf.model.backend.config` | 确切适配器配置参与身份判断。generation.model 路由将该对象用作原生参数，显式候选参数覆盖同名键，并仍执行原生选项验证。 |
| `option:NewEnvironment.host_id` | 规范的单一宿主机或宿主机池目标。省略时继承默认值；显式 null 选择可用空间最多的宿主机。接受创建时保存唯一的实际宿主机选择，重新检查双方出借权限及各项容量配额，并在原生任务重放时保留该选择。 |
| `option:ForkNewEnvironment.host_id` | 原有分叉或切换计划中新建环境时使用同一类型化宿主机意图。仅修改默认值不会搬迁已有环境。 |
| `option:env.host_id.type` | single 表示指定宿主机，pool 表示托管的宿主机池。 |
| `option:env.host_id.resource` | 单一目标中的实际宿主机叶节点。 |
| `option:env.host_id.resource.kind` | 只能为 host；其他资源类型在放置前拒绝。 |
| `option:env.host_id.resource.id` | 单一目标中实际宿主机的标识。 |
| `option:env.host_id.id` | 宿主机池标识；准入后的实际宿主机单独保存。 |
| `option:env.host_id.kind` | 只能为 host；池成员关系不会授予直接的宿主机使用权限。 |
| `option:runtime.host_id.type` | single 表示指定宿主机，pool 表示托管的宿主机池。 |
| `option:runtime.host_id.resource` | 单一目标中的实际宿主机叶节点。 |
| `option:runtime.host_id.resource.kind` | 只能为 host；其他资源类型在放置前拒绝。 |
| `option:runtime.host_id.resource.id` | 单一目标中实际宿主机的标识。 |
| `option:runtime.host_id.id` | 宿主机池标识；准入后的实际宿主机单独保存。 |
| `option:runtime.host_id.kind` | 只能为 host；池成员关系不会授予直接的宿主机使用权限。 |
| `option:HostUse.max_environments` | 统计通过实际宿主机使用条目创建、尚未删除的环境与提供者注册。池及出借权限的配额分别保留；接受新单元前，原生放置流程再次加锁并计数。 |

| 引用 | 含义 |
| --- | --- |
| `state:Turn.pool_access_changed` | 当前资源池、成员或借出权限已无法允许这项已接纳的工作。仅停止对应的原生轮次；直接使用成员的工作继续运行，不会暗中选择替代目标。 |
| `error:pool_access_changed` | 不含私有资源池、成员或授权者名称的本地化停止原因。恢复允许的选择前，原任务保持停止。 |

| 引用 | 含义 |
| --- | --- |
| `option:PoolDuplicateBody.expected_revision` | 复制时已审阅的源版本；源资源池变化后需重新审阅。 |
| `option:PoolDuplicateBody.home_id` | 按普通资源池创建权限检查的实际目标归属位置。 |
| `option:PoolDuplicateBody.name` | 新资源池名称。复制全部已保存的规则和自身路由设置，不会选择成员。 |
| `option:PoolDuplicateBody.access` | 新资源池使用此项明确的访问选择，不会复制源权限、成员同意、健康状态、轮换或用量。 |

| 引用 | 含义 |
| --- | --- |
| `option:PoolChoicesQuery.before_created_at` | 按最新优先分页时，上一页末行保存的时间；必须与 before_id 一同提交。 |
| `option:PoolChoicesQuery.before_id` | 与 before_created_at 配对的不可变选择记录 ID。游标不授予访问权，也不会预留成员。 |
| `option:PoolChoiceDetailsQuery.after_ordinal` | 从原始已记录遍历的此序号之后继续，不会重新展开或执行当前成员。 |
| `option:PoolChoiceView.resource` | 已保存的实际选中成员；只有当前拥有整条路径上每个资源池和成员的读取权限时才显示，包括实际密钥响应内容来源限制。 |
| `option:PoolChoiceView.policy` | 选中路径所保存的实际根策略。读取不会推进轮换。 |
| `option:PoolChoiceView.details_expired_at` | 详细展开记录的过期时间；原始选择、策略收据和原生执行关联仍保留。 |
| `option:PoolChoiceDetailsReply.expired_at` | 明确说明详情已过期，不会伪装为空展开结果，也不会重新选择成员。 |
| `option:PoolChoiceDetail.cost` | 已记录的请求预估额度，仅在拥有该候选成员读取来源时显示。缺失表示未知而非免费，也不是实际用量账单。 |
| `option:PoolChoiceDetail.cost_basis` | 有权查看预估额度时，详情还会显示当时记录的费率来源和缓存假设。 |
| `option:PoolCostBasis.source` | 作出选择时所用费率的来源。 |
| `option:PoolCostBasis.uncached_input` | 若预估时明确将全部输入按未缓存费率计算，此项为真。 |
| [正在选择](state-reference.md#state:PoolChoiceState.building) | 已接受的选择仍在记录中；查看不会启动任务。 |
| [已选择](state-reference.md#state:PoolChoiceState.selected) | 原生使用方已选中一个确切成员，当前读取权限仍控制其披露。 |
| [没有可用成员](state-reference.md#state:PoolChoiceState.exhausted) | 该次已记录的尝试没有找到可用成员，查看不会重试。 |
| `option:PoolChoiceView.id` | 独立的读取标识，与私有原生使用方或任务 ID 分开；每页详情仍会重新检查当前资源池读取权限。 |
