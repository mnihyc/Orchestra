---
group: operations
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [reference, settings, options, states, errors, 参考, 设置, 选项, 状态, 错误]
heading_id.条目: entries
summary: Orch 显示的每个错误的含义，以及下一步该做什么。
---

# 错误参考

操作失败时会保留可读的原因和可用的处理方式。下列稳定标识用于对照详情或问题报告，并不表示所有失败都应重试。缺少权限时使用现有的访问申请；计划发生变化时应重新审阅。可自动重试调用的规则见[模型调用恢复](model-call-recovery.md)。

## 条目

| 参考标识 | 含义 |
| --- | --- |
| `error:request.invalid` | 请求中有内容不符合要求。请检查填写的内容后重试。 |
| `error:request.value_refused` | 请求中有一个值无法保存。请检查填写的内容后重试。 |
| `error:admin.risk_not_acknowledged` | 这项更改有风险，需要先确认。请勾选确认后重试。 |
| `error:environment.none_usable` | 没有可供这个会话使用的环境，你也无权创建。请让管理环境的人为你开放权限。 |
| `error:auth.required` | 你尚未登录。请登录后重试。 |
| `error:auth.invalid_token` | 登录状态已失效，请重新登录。 |
| `error:auth.suspended` | 这个账号已被停用。请联系系统管理员。 |
| `error:auth.invalid_credentials` | 验证失败。请检查用户名以及密码或验证码。 |
| `error:auth.second_step_required` | 还需要一步验证才能完成登录。请输入验证码或恢复码。 |
| `error:auth.second_step_invalid` | 这个验证码不正确，或已经使用过。请检查后重试，或者重新获取。 |
| `error:org.suspended` | 你在这个组织的访问已被暂停。请联系它的管理员。 |
| `error:auth.email_unverified` | 邮箱地址尚未确认。请打开发送到你邮箱的链接。 |
| `error:auth.email_code_not_allowed` | 你所在的组织不允许把邮件验证码作为第二步验证。请使用验证器或恢复码。 |
| `error:auth.local_login_disabled` | 这里已停用密码和邮件验证码登录，请使用单点登录。 |
| `error:auth.session_required` | 这项操作需要浏览器登录状态。请打开网页应用并登录。 |
| `error:auth.csrf` | 请求被拒绝，因为它不是从本页面发出的。请刷新后重试。 |
| `error:auth.password_common` | 这个密码太常见了。请换一个密码，或者用更长的短语。 |
| `error:auth.second_step_setup` | 你所在的一个组织要求第二步验证。请先在“安全”里设置好，再继续。 |
| `error:auth.reauthentication_required` | 这项操作需要再次确认身份。请输入密码，或通过身份提供方重新登录。 |
| `error:token.lifetime_exceeded` | 这里的令牌有效期有上限。请选择更早的结束时间。 |
| `error:token.refused_here` | 这个令牌在本组织中已被拒绝。请使用其他令牌，或让创建者提供能访问本组织的令牌。 |
| `error:sharing.external_lifetime_exceeded` | 组织外人员的访问期限必须更短。请选择更早的结束时间。 |
| `error:authz.denied` | 你没有这项权限。请联系管理它的人。 |
| `error:registration.closed` | 这里没有开放注册。请向你要加入的团队索取邀请。 |
| `error:registration.invite_required` | 这个系统通过邀请加入。请向你要加入的团队成员索取邀请链接。 |
| `error:sharing.external_forbidden` | 你不能在这里添加组织之外的人。请联系组织管理员。 |
| `error:provider.subscription_not_allowed` | 这里的策略不允许使用这种智能体的订阅。请选择其他提供方，或联系当前位置的管理者。 |
| `error:provider.not_allowed` | 这里不允许使用这个提供方。请选择允许的提供方，或让当前位置的管理者检查限制。 |
| `error:provider.subscription_not_shared` | 这个订阅只能由所有者使用。请选择自己的提供方，或申请使用共享提供方。 |
| `error:workspace.remote_write_disabled` | 不能从这里修改这个环境中的文件。请让环境管理者允许修改文件。 |
| `error:policy.remote_write_disabled` | 这台机器不允许从这里修改文件。请让所有者允许修改文件。 |
| `error:terminal.not_allowed` | 你不能在这里打开终端。请向环境管理者申请终端权限。 |
| `error:browser.not_allowed` | 这里无法使用浏览器。请选择其他环境，或让管理者允许使用浏览器。 |
| `error:vnc.not_allowed` | 这里无法使用显示屏。请选择其他环境，或让管理者允许使用显示屏。 |
| `error:vnc.too_many` | 这个环境的显示屏已达上限。请先关闭一个。 |
| `error:vnc.start_failed` | 显示屏无法启动或连接。请重新查看或重试。 |
| `error:browser.too_many` | 这个环境打开的浏览器已达上限。请先关闭一个。 |
| `error:browser.profile_in_use` | 浏览器正在使用这个配置目录。请先关闭它，再更改配置目录。 |
| `error:browser.start_failed` | 浏览器启动失败。请检查此环境的浏览器设置后重试。 |
| `error:ports.not_allowed` | 当前权限或机器限制不允许使用此端口。请让环境管理者检查权限和限制。 |
| `error:ports.too_many` | 向其他人开放的不同端口已达上限。请先关闭一个开放项。 |
| `error:ports.no_range` | 主机没有已批准且包含空闲号码的端口范围。请让管理者检查端口范围。 |
| `error:ports.port_in_use` | 审核的主机端口无法绑定全部所选传输协议。请重新审核号码。 |
| `error:ports.program_conflict` | 端口程序已改变，或 TCP 前缀冲突。请审核当前选项。 |
| `error:ports.not_listening` | 此环境中没有服务监听这个端口。请先启动服务，再重试。 |
| `error:ports.domain_missing` | 浏览器入口需要端口域名。请让系统运营者配置域名，或使用辅助程序。 |
| `error:ports.unavailable` | 端口服务暂不可用。请检查环境的连接后重试。 |
| `error:tunnel.gateway_unreachable` | 这些机器无法连接到同一个网关。请检查机器的网络连接后重试。 |
| `error:chat.not_allowed` | 这里无法进行聊天。请选择有聊天权限的位置，或联系管理者。 |
| `error:chat.manager_required` | 只有当前聊天管理者可以这样操作。 |
| `error:chat.last_manager` | 聊天必须保留一位管理者。 |
| `error:chat.move_crosses_org` | 聊天只能留在所属组织内。请在目标组织新建聊天，并分享链接。 |
| `error:policy.mcp_import_disabled` | 这台机器不允许导入 MCP 服务器。请让所有者检查其策略。 |
| `error:policy.mcp_add_disabled` | 这台机器不接受从 Orch 添加的 MCP 服务器。请让所有者检查此策略。 |
| `error:docs.disabled` | 这里已停用文档。请让当前位置的管理者允许使用文档。 |
| `error:invitation.not_allowed` | 这个组织不允许这种形式的邀请。请选择允许的邀请方式，或联系管理者。 |
| `error:invitation.terms_not_accepted` | 请先查看并同意此组织的最新条款，再加入。 |
| `error:org.creation_disabled` | 这个系统限制了团队组织的创建。请让系统管理员为你创建，或邀请你创建。 |
| `error:not_found` | 它不存在、已被删除，或者没有对你开放。请返回有权打开的位置，或联系管理者。 |
| `error:access.locked` | 这项设置已被上级锁定，不能在这里更改。请打开决定它的位置，或联系管理者。 |
| `error:policy.not_delegated` | 这项设置由机器自己掌握。请让所有者更改设置，或将其交给 Orch 管理。 |
| `error:conflict.stale_revision` | 刚才有人改动了它。请查看当前状态后重试。 |
| `error:conflict.exists` | 这个名称或取值已被占用，请换一个。 |
| `error:conflict.idempotency_mismatch` | 同一请求被以不同的内容重复发送。请刷新后重试。 |
| `error:conflict.confirmation_required` | 这一步需要你先确认。请查看它将共享或更改的内容，然后确认。 |
| `error:node.archived` | 它位于已归档的内容之下。请先恢复上级。 |
| `error:node.restore_parent_first` | 请先使用上级内容自身的恢复权限恢复它。 |
| `error:node.wrong_parent` | 它只能在自己所属的组织树内移动。请选择这棵组织树中的目标位置。 |
| `error:org.too_deep` | 组织的嵌套层数已达上限。请选择更接近顶层的位置。 |
| `error:project.too_deep` | 文件夹的嵌套层数已达上限。请选择更接近顶层的文件夹。 |
| `error:org.closed` | 这个组织不再接收新成员。请让系统管理员解除限制。 |
| `error:access.too_many_requests` | 你等待处理的访问申请已达到这里允许的数量。请等其中一个被处理，或撤回一个。 |
| `error:org.flat` | 这个组织内不能再建组织。请选择其他上级，或让系统管理员允许此操作。 |
| `error:owner.last` | 不能移除最后一位所有者。请先指定另一位。 |
| `error:member.exists` | 这个人已经是成员了。请移动他们，而不是再次邀请。 |
| `error:member.same_domain` | 这个组织已经是此人的归属组织。请选择其他组织以移动此人。 |
| `error:member.domain_role` | 不能移除成员在归属组织中的角色。请更改其角色，或将此人移动到其他组织。 |
| `error:policy.exec_not_offered` | 这台机器没有提供所需的能力。请选择其他机器，或联系其所有者。 |
| `error:thread.not_idle` | 会话正忙。请等它结束，或者先停止它。 |
| `error:thread.not_running` | 会话已不再运行。请查看最新结果后再发送消息。 |
| `error:thread.use_switch` | 这个会话已有历史。请使用“切换”更换提供方、智能体或环境。 |
| `error:thread.side` | 旁支会话不支持这项操作。请打开其主会话继续。 |
| `error:thread.side_limit` | 这个会话打开的旁支会话已达上限。请先关闭一个。 |
| `error:goal.exists` | 这里已有进行中的目标。请先编辑或清除它。 |
| `error:goal.limit_reached` | 目标已达限额。请先提高限额，再继续。 |
| `error:input.not_pending` | 这条消息已不在等待中。请查看当前队列后再更改它。 |
| `error:input.not_held` | 这条消息没有在等待文件。请查看它当前的发送状态。 |
| `error:workspace.not_indexed` | 这个工作区太大，无法建立索引。请在“文件”中打开目录，或使用环境中的终端。 |
| `error:workspace.source_conflict` | 已经有同名的目录。请换一个，或使用已有的那个。 |
| `error:workspace.restore_conflict` | 计划恢复之后，这个目录发生了变化。请重新计划恢复。 |
| `error:workspace.restore_unavailable` | 无法完整恢复文件的这个状态。请改为逐个检查并撤销文件更改。 |
| `error:git.conflict` | 你查看之后，仓库发生了变化。请先重新查看再继续。 |
| `error:share.unavailable` | 无法与那个会话的智能体共享这项内容。请选择其他会话或智能体。 |
| `error:capability.unsupported` | 智能体不支持这项操作所需的能力。请选择支持此操作的智能体。 |
| `error:binding.incompatible` | 这个智能体、提供方和环境无法按当前安排协同工作。请换一种组合。 |
| `error:auto.disabled` | 这里已停用相应的自动化。请手动执行操作，或联系当前位置的管理者。 |
| `error:plan.blocked` | 这个方案目前无法执行。请查看是什么阻碍了它。 |
| `error:native_action.unavailable` | 当前会话无法使用此命令。请从会话当前的命令列表中选择。 |
| `error:native_action.stale` | 会话的命令列表已改变。请读取当前列表，并重新选择命令。 |
| `error:provider.handed_over` | 这个提供方的登录凭据目前在另一个环境中。请先从那个环境取回凭据再重试。 |
| `error:credential.return_failed` | 提供方的登录凭据没能取回。请重试，或者重新登录提供方。 |
| `error:model.not_allowed` | 这里不允许使用这个模型。请换一个，或请管理员放行。 |
| `error:approval.already_decided` | 已经有人做出了决定。请在会话或“审批”中查看结果。 |
| `error:preflight.expired` | 这个方案已经过期，请重新生成。 |
| `error:resource.in_use` | 它仍在使用中，无法删除。请先解除占用。 |
| `error:admin.last_admin` | 不能移除最后一位系统管理员。请先指定另一位。 |
| `error:upload.offset` | 上传位置与服务器保存的位置不符。请从服务器已保存的位置重试上传。 |
| `error:route.not_ready` | 这个渠道还无法送达给你。请先完成设置。 |
| `error:export.too_large` | 这份归档会超过这里允许的大小。请少选一些。 |
| `error:export.too_many` | 这份归档包含的内容会超过这里允许的数量。请少选一些。 |
| `error:export.nothing` | 这里没有你可以导出为文件的内容。请选择有权导出的内容，或联系管理者。 |
| `error:import.too_large` | 这次导入会超过这里允许的大小。请少选一些。 |
| `error:import.too_many` | 这次导入包含的内容会超过这里允许的数量。请少选一些。 |
| `error:import.nothing` | 这里没有你可以带入这个位置的内容。请换一个来源或位置。 |
| `error:import.invalid` | 这不是 Orch 包，也不是可以导入的文件。请检查文件后重试。 |
| `error:import.undone` | 这次导入已被撤销，或正在撤销。 |
| `error:gone` | 这个链接已被使用或已过期。请索取新链接。 |
| `error:docs.too_large` | 这样会使文档超出限制。请删除部分内容，或拆分为多份文档。 |
| `error:share.too_large` | 这份副本超过了这里的大小限制。请减少要共享的内容。 |
| `error:request.too_large` | 内容超过了服务器接受的大小。请减少内容后重试。 |
| `error:settings.instructions_too_large` | 这些常设指令会让这里的会话收到的内容超过上限。请缩短它们，或缩短周围位置的指令。 |
| `error:workspace.source_failed` | 无法准备文件。请检查环境的连接；如使用仓库，还请检查其地址和访问权限，然后重试。 |
| `error:git.failed` | Git 未能完成操作。请先检查仓库的当前状态，再重试。 |
| `error:auth.locked` | 这个账号的尝试次数过多。请稍后再试，或者换一种方式登录。 |
| `error:rate_limited` | 短时间内请求过多。请稍等片刻后重试。 |
| `error:internal` | 服务器未能完成这个请求，请重试。 |
| `error:source.offline` | 运行它的机器处于离线状态。请重新连接，或联系所有者后重试。 |
| `error:agent.offline` | 暂时无法连接到智能体，请稍后重试。 |
| `error:provider.unavailable` | 提供方现在无法执行这个操作。 |
| `error:harness.sessions_no_runtime` | 此智能体尚未在此提供方上运行。请先使用该智能体开始一个会话，再查看其已有会话。 |
| `error:harness.sessions_unavailable` | 此智能体未能列出已有会话。请检查其所在机器后重试。 |
| `error:harness.channels_unavailable` | 无法读取发布渠道。请检查此服务器的网络访问，或设置镜像。 |
| `error:provider.reset_stale` | 重置项或可用数量已变更。请刷新并重新确认当前重置项。 |
| `error:provider.reset_unavailable` | 此提供方目前无法使用重置。请刷新其信息或联系管理者。 |
| `error:environment.unavailable` | 环境暂时不可用，请稍后重试。 |
| `error:network.restricted_unavailable` | 请求的网络限制尚未验证。请重试更改，或在新的计划中明确选择开放网络。 |
| `error:network.host_denied` | 网络策略拒绝了这个地址。请选择允许的地址，或联系机器所有者。 |
