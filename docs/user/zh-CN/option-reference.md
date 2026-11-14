---
group: providers
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [选项, 字段, 搜索, 参考, options, fields]
heading_id.条目: entries
summary: 提供方接口接受的每个请求选项，以及它改变什么。
heading_id.实时文件读取: live-file-reads
---

# 选项字段参考

只有接口或程序声明支持的选项才会提供，提供方限制和权限仍然适用。自定义结构中的字段沿用该结构的含义；本参考涵盖随产品提供的声明。设置中的字段见[设置参考](settings-reference.md)。作出改变前先审阅计划，选择运行方式前阅读[程序指南](register-your-program.md#choose-an-arrangement)。

## 条目

| 参考标识 | 含义 |
| --- | --- |
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


### 实时文件读取

| 参考标识 | 含义 |
| --- | --- |
| `option:Thread.starting_since` | 本次准备开始的时间；未在启动时省略。 |
| `option:Thread.starting_work` | 当前准备工作、对应命令和正在前方执行的工作。 |
| `option:StartingWork.command` | 本次准备已接受的原生命令，包括正在执行或排队的命令。 |
| `option:StartingWork.behind` | 实际占据前方执行通道的工作；已完成捕获后的上传不会占住后续启动。 |
| `option:Turn.files_changed` | 从完整前后状态或原生变更日志得到的本轮更改路径数量；省略表示未知。 |
| `option:Turn.files_changed_paths` | 本轮已知的相对更改路径，也包括没有文件工具补丁的更改。 |
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
