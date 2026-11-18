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
| `option:EverythingListQuery.resource_kind` | 按模板的单元类型、环境的 machine/template 来源，或用 system 选择没有归属位置的代理配置；在名称搜索和分页前筛选。 |
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
| `option:EnvironmentMcpListQuery.collection` | 原生目录：默认是 servers 连接，或 found 发现的定义。 |
| `option:EnvironmentMcpListQuery.q` | 在选定目录的可见名称、程序或目录路径中搜索词语。 |
| `option:EnvironmentMcpListQuery.limit` | 每页条目数；默认 50，可用任意正 u32。 |
| `option:EnvironmentMcpListQuery.cursor` | 继续同一环境、目录和搜索。 |
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
| `state:SessionEndCause.expired` | 自身原生会话已到期。公开的无效令牌不能证明此原因。 |
| `state:SessionEndCause.revoked` | 自身原生会话已被撤销。不含其他会话或用户的信息。 |
| `state:SessionEndCause.account_unavailable` | 自身原生会话的账号已不可用。未记录的变更时间仍显示“尚未报告”。 |
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
| `option:Started.deviceChoice` | 原有成功设置读取的记录。当前提供方、模型、代理程序、思考强度、环境和布置仍由原生解析器确定；记住的布置不会被当作新的凭据授权。 |
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
| `option:DirListing.revision` | 本次文件夹观察的标识；稳定的小文件夹使用不可变快照，宽大或持续变化的文件夹则继续同一原生读取器。每页在自身窗口内排序，并说明观察时间范围。 |
| `option:FsListPageResult.revision` | 按字节分页时保留同一原生文件夹观察标识，不代表整个文件夹在某一瞬间的完整切面。 |
