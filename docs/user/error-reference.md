---
group: operations
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [reference, settings, options, states, errors, 参考, 设置, 选项, 状态, 错误]
summary: What each error Orch shows means and what to do next.
---

# Error reference

A failed action keeps its human explanation and available recovery. These stable identifiers are for matching Details or a support report; they are not instructions to retry every failure. A missing permission uses the existing access request, and a changed plan must be reviewed again. See [model call recovery](model-call-recovery.md) for rules that may retry a call.

## Entries

| Reference | Meaning |
| --- | --- |
| `error:request.invalid` | Something in this request is not valid. Check what you entered and try again. |
| `error:request.value_refused` | A value in this request is not one that can be kept. Check what you entered and try again. |
| `error:admin.risk_not_acknowledged` | This change carries a risk that has to be acknowledged first. Tick the acknowledgement and try again. |
| `error:ask.not_in_list` | This model is not on this organization’s Ask list. Choose one of the listed models. |
| `error:ask.model_disabled` | This model is switched off. Choose another model. |
| `error:ask.provider_unavailable` | This model’s provider is not available. Choose another model or ask whoever manages the provider. |
| `error:ask.no_interface` | This model’s provider offers no way to hold a conversation. Choose another model. |
| `error:ask.options_refused` | The options chosen for this model are not allowed. Change them or use the model’s defaults. |
| `error:ask.model_not_allowed` | This organization’s policy does not allow this model. Choose another model. |
| `error:ask.provider_not_allowed` | This organization’s policy does not allow this model’s provider. Choose another model. |
| `error:ask.payer_unset` | Ask is set to be paid for by whoever set it, and that person is no longer known. Ask an administrator to set who pays. |
| `error:ask.no_use` | You may not use this model’s provider in this organization. Ask whoever manages it to let you. |
| `error:ask.identity_not_usable` | Your access to this provider does not cover this model’s default options. Choose another model. |
| `error:ask.use_excludes` | Your access to this provider leaves out this model or effort. Choose another. |
| `error:ask.payer_no_use` | The person who pays for Ask here may not use this provider in this organization. Ask an administrator. |
| `error:ask.payer_excludes` | The person who pays for Ask here may not use this model or effort. Choose another. |
| `error:ask.no_keeper` | This provider has no machine that keeps its key. Ask whoever manages it to set one. |
| `error:ask.keeper_offline` | The machine that keeps this provider’s key is offline. Try again shortly or choose another model. |
| `error:ask.cap_reached` | A usage limit for Ask has been reached. Try again after it resets, or ask an administrator. |
| `error:ask.other_organization` | This page belongs to another organization. Ask in that organization. |
| `error:ask.root_refused` | You cannot open this organization as yourself. Ask answers as you are here, not as an administrator, so As admin does not open it. Use an organization you belong to. |
| `error:ask_context_too_long` | The conversation cannot fit even after folding whole older turns. Choose a larger allowed model, start a new conversation or keep it as a thread; the originals remain in history. |
| `error:ask_turn_too_long` | The current question and what it read cannot fit by themselves. Ask a narrower question or choose a model with more room; summarizing older messages cannot make this turn smaller. |
| `error:ask_unavailable` | The question could not finish. Read its current explanation and use the offered retry when available; a later question can continue the conversation. |
| `error:environment.none_usable` | There is no environment you can use for this thread, and you may not create one. Ask somebody who manages one to let you in. |
| `error:auth.required` | You are not signed in. Sign in and try again. |
| `error:auth.invalid_token` | Your session is no longer valid. Sign in again. |
| `error:auth.suspended` | This sign-in is suspended. Ask an administrator of this system. |
| `error:auth.invalid_credentials` | That did not work. Check the name and the password or code. |
| `error:auth.second_step_required` | One more step is needed to finish signing in. Enter your verification or recovery code. |
| `error:auth.second_step_invalid` | That code did not work, or was used already. Check it, or ask for another. |
| `error:org.suspended` | Your access to this org is suspended. Ask one of its admins. |
| `error:auth.email_unverified` | Your address is not confirmed yet. Open the link that was mailed to you. |
| `error:auth.email_code_not_allowed` | Your orgs do not allow a mailed code as a second step. Use an authenticator or recovery code. |
| `error:auth.local_login_disabled` | Signing in with a password or a mailed code is switched off here. Use single sign-on. |
| `error:auth.session_required` | This needs a browser session. Open the web application and sign in. |
| `error:auth.csrf` | The request was refused because it did not come from this page. Reload and try again. |
| `error:auth.password_common` | This password is commonly used. Choose a different password or a longer phrase. |
| `error:auth.second_step_setup` | An org you belong to requires a second step. Set one up under Security to go on. |
| `error:auth.reauthentication_required` | Prove who you are again for this: give your password, or sign in again with your identity provider. |
| `error:token.lifetime_exceeded` | A token may live only so long here. Choose an earlier end. |
| `error:token.refused_here` | This token is refused in this org. Use another token or ask its maker for one that reaches this org. |
| `error:sharing.external_lifetime_exceeded` | Access for somebody from outside this org must end sooner. Choose an earlier end. |
| `error:authz.denied` | You do not have access to this. Ask somebody who manages it. |
| `error:registration.closed` | Registration is closed here. Ask the team you want to join for an invitation. |
| `error:registration.invite_required` | This system takes new people by invitation. Ask somebody in the team you are joining for a link. |
| `error:sharing.external_forbidden` | You may not name somebody from outside this org here. Ask one of its admins. |
| `error:provider.subscription_not_allowed` | The policy here does not allow this agent's subscription. Choose another provider or ask the place's manager. |
| `error:provider.not_allowed` | This provider is not allowed here. Choose an allowed provider or ask the place's manager to review the limit. |
| `error:provider.subscription_not_shared` | This subscription can only be used by its owner. Choose your own provider or ask for a shared provider. |
| `error:workspace.remote_write_disabled` | Files of this environment cannot be changed from here. Ask its manager to allow file changes. |
| `error:policy.remote_write_disabled` | This machine does not allow changing files from here. Ask its owner to allow file changes. |
| `error:terminal.not_allowed` | You may not open a shell here. Ask the environment's manager for shell access. |
| `error:browser.not_allowed` | Browsers are not available here. Choose another environment or ask its manager to allow them. |
| `error:vnc.not_allowed` | Displays are not available here. Choose another environment or ask its manager to allow them. |
| `error:vnc.too_many` | This environment's display limit is full. Close a display first. |
| `error:vnc.start_failed` | The display could not start or be reached. Look again or try again. |
| `error:browser.too_many` | This environment has as many browsers open as it allows. Close one first. |
| `error:browser.profile_in_use` | A browser is using this profile. Close it before changing the profile. |
| `error:browser.start_failed` | The browser could not start. Check the browser setup on this environment and try again. |
| `error:ports.not_allowed` | Current access or machine limits do not allow this port use. Ask the environment's manager to review access and limits. |
| `error:ports.too_many` | The limit of distinct ports open to others is reached. Close an opening first. |
| `error:ports.no_range` | The host has no approved port range with a free number. Ask its manager to review the port ranges. |
| `error:ports.port_in_use` | The reviewed host number cannot bind every selected transport. Review a new number. |
| `error:ports.program_conflict` | The port program changed or its TCP prefix conflicts. Review its current options. |
| `error:ports.not_listening` | No service in this environment is listening on this port. Start the service before trying again. |
| `error:ports.domain_missing` | Browser entry needs a ports domain. Ask the system operator to configure it, or use the helper. |
| `error:ports.unavailable` | The port service is not available. Check the environment's connection and try again. |
| `error:tunnel.gateway_unreachable` | The machines cannot reach a shared gateway. Check their network connections and try again. |
| `error:chat.not_allowed` | A chat is not possible here. Choose a place you may chat in or ask its manager. |
| `error:chat.manager_required` | Only a current chat manager may do that. |
| `error:chat.last_manager` | A chat must keep a manager. |
| `error:chat.move_crosses_org` | A chat stays in its organization. Make a new chat there and share a link. |
| `error:policy.mcp_import_disabled` | This machine does not allow importing MCP servers. Ask its owner to review its policy. |
| `error:policy.mcp_add_disabled` | This machine does not accept MCP servers added from Orch. Ask its owner to review that policy. |
| `error:docs.disabled` | Documents are switched off here. Ask the place's manager to allow them. |
| `error:invitation.not_allowed` | This org does not allow that kind of invitation. Choose an allowed invitation method or ask its manager. |
| `error:invitation.terms_not_accepted` | Review and accept the current organization terms before joining. |
| `error:org.creation_disabled` | Making a team org is restricted here. Ask a system administrator to create one or invite you to make one. |
| `error:not_found` | This does not exist, was deleted, or is not open to you. Return to a place you can open or ask its manager. |
| `error:access.locked` | This setting is locked above and cannot be changed here. Open the place that sets it or ask its manager. |
| `error:policy.not_delegated` | This machine keeps that setting to itself. Ask its owner to change it or hand it over to Orch. |
| `error:conflict.stale_revision` | Somebody changed this in the meantime. Look at how it stands now and try again. |
| `error:conflict.exists` | That name or value is taken already. Choose another. |
| `error:conflict.idempotency_mismatch` | This request was repeated in a different form. Reload and try again. |
| `error:conflict.confirmation_required` | This needs your confirmation first. Review what it will share or change, then confirm. |
| `error:node.archived` | This lies under something that is archived. Bring that back first. |
| `error:node.restore_parent_first` | Bring back the parent first, under its own restoration permission. |
| `error:node.wrong_parent` | This can only be moved within its own tree of orgs. Choose a destination in that tree. |
| `error:org.too_deep` | Orgs cannot be nested any deeper here. Choose a place nearer the top. |
| `error:project.too_deep` | Folders cannot be nested any deeper here. Choose a folder nearer the top. |
| `error:org.closed` | This org takes no further members. Ask a system administrator to open it. |
| `error:access.too_many_requests` | You already have as many requests for access waiting as are allowed here. Wait for one to be decided, or cancel one. |
| `error:org.flat` | This org cannot hold orgs inside it. Choose another parent or ask a system administrator to allow it. |
| `error:owner.last` | The last owner cannot be removed. Name somebody else first. |
| `error:member.exists` | This person is a member already. Move them instead of inviting them. |
| `error:member.same_domain` | That org is this person's home already. Choose another org to move them. |
| `error:member.domain_role` | A member's home-org role cannot be removed. Change their role or move them to another org. |
| `error:policy.exec_not_offered` | This machine does not offer the requested capability. Choose another machine or ask its owner. |
| `error:thread.not_idle` | The thread is busy. Wait for it to finish, or stop it. |
| `error:thread.not_running` | The thread is no longer running. Review its latest result before sending another message. |
| `error:thread.use_switch` | This thread already has history. Use Switch to change its provider, agent or environment. |
| `error:thread.side` | A side thread does not support this action. Open its main thread to continue. |
| `error:thread.side_limit` | This thread has as many side threads open as it may. Close one first. |
| `error:goal.exists` | A goal is under way here already. Edit or clear it first. |
| `error:goal.limit_reached` | A limit of the goal is reached. Raise the limit first, then resume. |
| `error:input.not_pending` | This message is no longer waiting. Review the current queue before changing it. |
| `error:input.not_held` | This message is not waiting for its files. Review its current delivery state. |
| `error:workspace.not_indexed` | This workspace is too large to index. Open a directory in Files or use the environment's shell. |
| `error:workspace.source_conflict` | A directory with that name exists already. Choose another one, or use the one that is there. |
| `error:workspace.restore_conflict` | The directory changed since the restore was planned. Plan the restore again. |
| `error:workspace.restore_unavailable` | This state of the files cannot be restored completely. Review and undo individual files instead. |
| `error:git.conflict` | The repository changed since you looked at it. Look again before you go on. |
| `error:share.unavailable` | This cannot be shared with that thread's agent. Choose another thread or agent. |
| `error:capability.unsupported` | The agent does not support what this needs. Choose an agent that supports this action. |
| `error:binding.incompatible` | This agent, provider and environment cannot work together as arranged. Choose another combination. |
| `error:auto.disabled` | The automation for this is switched off here. Use the manual action or ask the place's manager. |
| `error:plan.blocked` | This plan cannot be carried out as it stands. Look at what blocks it. |
| `error:native_action.unavailable` | This command is not available in the current session. Choose one from its current command list. |
| `error:native_action.stale` | The session’s command list changed. Read the current list and choose the command again. |
| `error:provider.handed_over` | This provider's sign-in is with another environment. Bring it back from that environment before trying again. |
| `error:credential.return_failed` | The provider's sign-in could not be brought back. Try again, or sign the provider in anew. |
| `error:model.not_allowed` | This model is not allowed here. Choose another, or ask an admin to allow it. |
| `error:approval.already_decided` | Somebody decided this already. Review the decision in the thread or Approvals. |
| `error:preflight.expired` | This plan is out of date. Make it again. |
| `error:resource.in_use` | This is still in use and cannot be deleted. Free it first. |
| `error:admin.last_admin` | The last system administrator cannot be removed. Name another one first. |
| `error:upload.offset` | The upload stopped at a different position. Retry it from the server's last saved position. |
| `error:route.not_ready` | This channel cannot reach you yet. Finish setting it up first. |
| `error:export.too_large` | This archive would be larger than the place allows. Choose less. |
| `error:export.too_many` | This archive would hold more things than the place allows. Choose less. |
| `error:export.nothing` | There is nothing here that you may take away as a file. Choose content you may export or ask its manager. |
| `error:import.too_large` | This import would be larger than the place allows. Choose less. |
| `error:import.too_many` | This import would hold more things than the place allows. Choose less. |
| `error:import.nothing` | There is nothing here that you may bring into this place. Choose another source or place. |
| `error:import.invalid` | This is not an Orch package or files that can be imported. Check the file and try again. |
| `error:import.undone` | This import was taken back already, or is being taken back. |
| `error:gone` | This link was used already or has expired. Ask for a new link. |
| `error:docs.too_large` | This would take the document past a limit. Remove some content or split it into separate documents. |
| `error:share.too_large` | This copy is larger than the limit here. Choose less content to share. |
| `error:request.too_large` | This is larger than the server accepts. Choose less content and try again. |
| `error:settings.instructions_too_large` | These standing instructions would make what a thread here is told longer than it may be. Shorten them, or those of the places around it. |
| `error:workspace.source_failed` | The files could not be prepared. Check the environment's connection, and the repository's address and access if one is used, then retry. |
| `error:git.failed` | Git could not finish the operation. Review the repository state before trying again. |
| `error:auth.locked` | Too many wrong tries for this name. Try again later, or sign in another way. |
| `error:rate_limited` | Too many requests in a short time. Wait a little and try again. |
| `error:internal` | The server could not complete this request. Try again. |
| `error:source.offline` | The machine this runs on is offline. Reconnect it or ask its owner before trying again. |
| `error:agent.offline` | The agent cannot be reached at the moment. Try again shortly. |
| `error:provider.unavailable` | The provider is not ready at the moment. Try again shortly. |
| `error:harness.sessions_no_runtime` | This agent has no runtime for this provider yet. Start a thread with this agent, then check its sessions again. |
| `error:harness.sessions_unavailable` | This agent could not list its sessions. Check its machine and try again. |
| `error:harness.channels_unavailable` | The release channels could not be read. Check this server's internet access or name a mirror. |
| `error:provider.reset_stale` | The reset or the number available changed. Refresh and confirm the current reset again. |
| `error:provider.reset_unavailable` | This provider cannot use a reset now. Refresh its facts or ask its manager. |
| `error:environment.unavailable` | The environment is not ready at the moment. Try again shortly. |
| `error:network.restricted_unavailable` | The requested network restriction is not verified. Retry the change or choose open in a new plan. |
| `error:network.host_denied` | The network policy refused this address. Choose an allowed address or ask the machine's owner. |
