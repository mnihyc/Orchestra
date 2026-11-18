---
group: providers
screens: [/help, /thread/$threadId, /resources/providers/$id, /settings/interface, /topic/$topicId]
keywords: [reference, settings, options, states, errors, 参考, 设置, 选项, 状态, 错误]
summary: Shipped request, interface and provider options, and what they change.
---

# Option field reference

An option is offered only where its interface or program declares it. Provider limits and permissions still apply. Fields you define in a custom schema keep that schema’s own meaning; this reference covers the shipped declarations. Setting fields are in the [settings reference](settings-reference.md). Read the plan before a change and [the program guide](register-your-program.md#choose-an-arrangement) before choosing an arrangement.

## Entries

| Reference | Meaning |
| --- | --- |
| `option:TurnLost.reason` | The connector’s explanation of the lost turn. |
| `option:TurnLost.turn_id` | Identifies the admitted turn the connector could no longer follow after restart. |
| `option:ThreadSideCreateBody.acting` | Choose an independent session that can act in the parent workspace. Omit for a verified read-only side. |
| `option:ThreadSides.acting` | The parent has a bound agent, provider and environment for an acting side session. The ordinary side-thread policy also applies. |
| `option:AskMoveOutPlan.preflight_id` | A caller-owned review of the full built-in conversation copy and original native Delete. Confirmation rechecks current Copy, Export, destination creation, ownership and every deleted resource. |
| `option:AskMoveOutPlan.delete_plan` | The same native original-deletion footprint used by other thread moves. A changed footprint requires a fresh review and creates no copy. |
| `option:AskMoveOutPlan.open_side_threads` | Independent open sides that must be closed or kept before moving their main conversation. Names follow current View authority. |
| `option:AskMoveOutBody.preflight_id` | Confirms that review atomically, preserving the built-in program, copying all conversation history, applying destination access defaults and choosing destination governance. A creation retry acknowledges the one result even after its original ended. |
| `option:SessionSpec.opening_command` | The controller opening whose native launch supplies a configuration receipt. It is carried internally, never entered by a person or taken from a session credential. |
| `option:SessionOpened.mcp` | Names of Orch servers actually supplied by the adapter. An absent receipt remains unreported until a delayed native launch reports its configuration; an empty list confirms none were supplied. |
| `option:SessionOpenResult.mcp` | The same actual supplied-server receipt on the connector acknowledgement. Requested servers alone do not prove delivery; changing only the model transport preserves existing tool membership. |
| `option:MarkerHarnessChanged.value.opening_command` | Associates native MCP configuration with the exact controller opening. A different opening or a closed process cannot inherit its delivery facts; a native same-name definition is not attributed to the ignored Orch resource. |
| `option:ThreadMoveAudience.people` | Every currently eligible original contributor, excluding the mover. Author reads page internally; the notification choice has no fixed recipient cutoff. |
| `option:ThreadMoveAudience.preview_id` | The same private Move review used by the searchable, paginated people list; absent only for an empty audience. |
| `option:ThreadMoveOutPreviewResponse.notification` | The complete reviewed contributor count for the optional Move notification, absent on a write-only plan receipt. |
| `option:EnvironmentMcp.connections` | The requested thread’s actual program connections. They include reported servers without a static importable configuration, so an incomplete discovery cannot hide a running connection. |
| `option:McpConnection.name` | The server name reported by that program opening. |
| `option:McpConnection.source` | The kind of program reporting this connection. |
| `option:McpConnection.state` | The reported state in this actual opening, or unreported until a state arrives. A resumed or restarted program cannot inherit an older opening’s ready state. |
| `option:McpConnection.using` | The native configuration or Orch definition selected for this opening. |
| `option:MarkerHarnessChanged.value.epoch` | The identity of the actual program opening whose MCP setup and connection reports belong together. It is observation metadata, never a user-entered identifier. |
| `option:MarkerHarnessChanged.value.servers` | A program-opening MCP setup snapshot. It replaces prior evidence even when the new setup has no servers; each entry names the server and supplied definition source. |
| `option:McpServer.spec.imported.origin` | The opaque native discovery origin retained with an imported copy. Private provider/profile routing remains in controller evidence and never becomes a public resource specification. |
| `option:EnvironmentMcpImportCreateBody.origin` | The opaque identity carried by the discovery row selected in the existing list. It distinguishes profiles whose native servers share a name; omission is accepted only when that entry is unambiguous. No person types this identity. |
| `option:McpFound.origin` | The stable native entry identity used by Import and its retry receipt. Provider identities stay private; the row and action carry this value automatically. |
| `option:McpFound.profile` | The native agent profile’s readable name, omitted when this reader may not view that profile. Connection observations and the opaque import identity do not grant profile access. |
| `option:McpProfileRef.id` | The admitted agent profile behind this named native configuration, rendered with the existing resource chip. |
| `option:McpProfileRef.name` | The name of that readable native profile; its home, command, address and credentials are not disclosed here. |
| `option:McpImport.profile` | The actual provider and agent profile selected by discovery for a native definition copy. The connector reads that exact hosted profile instead of choosing another program with the same server name. |
| `option:McpImportAsk.profile` | A local machine import keeps the same selected native profile on its authenticated request channel. |
| `option:EnvironmentFactsMcp.profile` | The hosting connector’s private provider/profile association for a discovered native definition. It separates same-named servers without publishing their launch data. |
| `option:EnvironmentMcpGetQuery.thread_id` | Read the current native MCP connection observations of this thread in its present environment. The caller must also hold View on that thread; omission shows discoveries without borrowing any thread’s runtime state. |
| `option:McpFound.state` | The program’s last reported connection state in the requested thread’s selected native segment. No report remains unreported; an old segment, child conversation or before-Orch history cannot supply its current state. |
| `option:McpFound.using` | The supplied definition behind that program connection: native configuration or an Orch server. Importing alone does not change the program’s own definition. |
| `state:McpDefinitionSource.native` | The local program retains its own global or directory definition. A shared name keeps this native definition; the Orch copy can still be used in other environments. |
| `state:McpDefinitionSource.orch` | The program received the server enabled for this environment in Orch. Its connection state is distinct from the environment host’s server state. |
| `option:MarkerHarnessChanged.value.definition_source` | A native MCP connection observation names the definition supplied at that session’s opening: native or Orch, without its command, address or credentials. An unknown source remains unreported. |
| `option:FilesPlan.source_policy` | The source domain selected by this workspace read/copy operation, captured in the admitted review rather than inferred by the interface from a current path or environment. |
| `option:FilesPlan.home_policy` | Whole-root owned-data policy for the actually measured home copy. Workspace exclusions do not apply; ordinary caller and operation permissions still govern writes. |
| `option:FilesPlan.file_states_policy` | Whole-root owned-data policy for the measured recorded-data copy, independently of workspace exclusions. Older uncaptured plans stay Not reported. |
| `option:UserMessagePayload.native_image_delivery` | Applied image decisions for the original admitted command, including the accepted allowance, resolution time and safe setting source. Missing historical receipts stay Not reported; private nested setting sources stay Not shared. |
| `option:ThreadInput.native_image_delivery` | The same captured image delivery decisions on a queued or delivered input. Editing a setting later does not rewrite them. |
| `state:NativeImageDelivery.native` | Original image bytes accompanied the admitted native command. |
| `state:NativeImageDelivery.path` | The admitted input names the verified materialized image path. Only an actual positive-byte refusal says Image allowance exceeded; zero says Images use paths. |
| `state:NativeImageDelivery.unavailable` | Neither native bytes nor a verified materialized path was available; this does not claim path delivery. |
| `option:FileTransferReply.transfer` | The announced native manifest transfer for the current environment, machine and generation. It is unrelated to browser upload sessions; reading starts no native work. |
| `option:FileTransferFacts.expires_at` | Last accepted piece plus the idle allowance captured at the first piece. No date is invented when the original receipt is absent; a later setting edit changes neither this expiry nor its source. |
| `option:FileTransferFacts.retention_source` | The captured registry-default or System setting source. Cleanup retains this receipt after removing expired pieces. |
| `state:FileTransferState.transferring` | The native publisher announced a transfer that has not completed; no successful publication is implied. |
| `state:FileTransferState.interrupted` | The native publisher reported interruption of this exact transfer. Its recorded expiry still applies. |
| `state:FileTransferState.expired` | Cleanup confirmed removal of the unfinished transfer after its recorded idle expiry. |
| `option:Preview.allowance` | The specific refusal with its captured limit, measured amount when known and setting source; an unknown refusal does not invent an allowance. |
| `option:Preview.viewer_allowance` | The accepted original-read allowance for the interactive viewer, distinct from decoding source bytes. |
| `option:Preview.facts` | Authorized native source facts are added after the derived-content cache; different owners of the same hash cannot share source policy or authority. |
| `state:PreviewAllowanceKind.decoded_source_bytes` | Original bytes exceeded the static decoder allowance. |
| `state:PreviewAllowanceKind.pixels` | Original image pixels exceeded the decoder or renderer allowance. |
| `state:PreviewAllowanceKind.encoded_preview_bytes` | Even the smallest encoded static preview exceeded its byte allowance. |
| `state:PreviewAllowanceKind.viewer_bytes` | The original exceeds the interactive viewer read allowance; Download remains available under its ordinary permission. |
| `option:DirListing.facts` | Native source, applicable exclusion and installed watcher facts captured with the authorized directory observation. Absence is Not reported; opening the facts performs no tree scan. |
| `option:FileStat.facts` | Native source policy and applicable exclusion facts for this read. Retained manifests keep their captured policy; uncaptured child rule provenance stays Not reported. |
| `option:FileSourceFacts.policy` | Classifies workspace, read-only mount or whole owned-data root. A writable source still requires the caller’s ordinary operation permission. |
| `option:FileSourceFacts.inclusion` | Applicable configured and ancestor-local Orch rules, shallow to deep. Deeper rules override, excluded parents prevent traversal, and native agent access remains separate. Hidden rule names are Not shared; patterns and omitted children are not exposed. |
| `option:FileSourceFacts.watch` | The installed native watcher’s scope, fallback interval and observation identity. A later configuration edit does not change an already captured interval; a retained manifest has no live watch. |
| `option:FileSourceFacts.observed_at` | The native capture time of these facts, independent of when the disclosure is opened. |
| `state:FileSourceKind.workspace` | The attached workspace uses Orch exclusions and protected control-file rules. |
| `state:FileSourceKind.mount` | An attached read-only root without workspace exclusions. |
| `state:FileSourceKind.owned_data` | An owned root copied whole without workspace exclusions. |
| `state:FileRuleKind.configured` | Configured Orch patterns contributed to this path’s inclusion. |
| `state:FileRuleKind.local` | An applicable local exclusion file contributed; its name is redacted when excluded. |
| `state:FileWatchSource.machine_configuration` | The fallback interval was supplied by the machine configuration accepted by the installed watcher. |
| `option:ResourceNotificationQuery.notify` | Notify the reviewed affected people once for this deletion. Absent or false leaves notification off; personal routes remain a separate choice. |
| `option:ResourceNotificationQuery.notification_review` | The caller-bound audience from the current impact review, required when Notify is selected. A changed audience requires a fresh review. |
| `option:ResourceStopQuery.note` | An optional one-line note for threads waiting on the disabled provider. |
| `option:ResourceStopQuery.notify` | Notify the reviewed affected people once for this provider stop. Absent or false leaves notification off. |
| `option:ResourceStopQuery.notification_review` | The caller-bound audience from the current impact review, required when Notify is selected. |
| `option:ResourceImpact.notification` | The unique current audience for the operation choice, initially off; its preview supplies the people list. |
| `option:ResourceImpactQuery.skill_publication` | For a skill publication, count current delivered thread contexts whose effective choice follows Latest, including personal choices and inherited locks. Pins are excluded even when pinned to the current Latest. Absent or false counts all current deliveries for a resource-wide change. Valid only for skills. |
| `option:CreateThread.workspace_edits` | File edits saved in the thread's start plan. Opening the editor creates no workspace. Creation applies these after source and environment initialization, before the first agent input; a conflict retains both texts and holds the start. Applied edits are consumed and future turns read the real shared files. |
| `option:WorkspaceFileEdit.path` | A distinct file path relative to the selected thread directory, with no absolute path or parent traversal. The shared instructions editor uses AGENTS.md. |
| `option:WorkspaceFileEdit.text` | The exact saved file text, retained in the start plan until its conditional write succeeds. Conversation metadata does not disclose this file content. |
| `option:WorkspaceFileEdit.expected_hash` | The source version the person read. Absent or null means create only if missing; an existing file brought by an unknown source requires review rather than an unconditional overwrite. |
| `option:WorkspaceFileEdits.revision` | The current start-file plan and bound workspace identity. Replacing edits requires this revision so a newer edit, directory or environment is never silently overwritten. |
| `option:WorkspaceFileEdits.edits` | File edits still waiting to be applied, read under the bound thread's View and the environment's files.view authority. Empty after successful application. |
| `option:EnvironmentSourceEditsQuery.thread_id` | The actual thread bound to the selected environment whose pending start-file edits are read. It grants no additional file or thread access. |
| `option:EnvironmentSourcePrepareBody.workspace_edits` | Explicitly replace the pending file edits after reviewing the source. An empty list accepts the source's files; omission keeps the saved edits. File writes still require the existing native source and workspace permissions. |
| `option:EnvironmentSourcePrepareBody.workspace_edit_revision` | Required when replacing workspace_edits. A mismatch keeps the newer saved plan and refuses the stale replacement. |
| `option:MountSpec.resource_id` | The resource identity carried with the actual native attachment, not an authorization grant. It keeps an applied mount associated with its source when names or targets change; absent identity remains unreported. |
| `option:ResourceImpact.applications` | Current native tool and skill deliveries, and running units that retain an attachment or recipe. Each group counts distinct threads, environments or providers at its own application boundary; empty groups are omitted. Explicit giving and historical recovery references are not live users. |
| `option:ResourceImpact.application_complete` | Whether the native sources account for every current delivery and attachment. False keeps missing evidence explicit rather than turning it into zero affected work. |
| `option:ResourceApplicationGroup.when` | The boundary at which this resource change reaches the group's existing native dependents. |
| `option:ResourceApplicationGroup.object_kind` | The count's unit: thread, environment or provider. Different kinds are never added into one unlabeled figure. |
| `option:ResourceApplicationGroup.count` | Distinct current dependents, including those whose names this reader cannot see. Repeated attachment observations count once. |
| `option:ResourceApplicationGroup.visible` | Names independently readable under this impact operation's native name sources. Reading the resource itself does not grant these names. |
| `option:ResourceApplicationGroup.hidden` | Current dependents whose names and identities remain private; only their count is shown. |
| `option:ResourceApplicationGroup.apply` | The existing Update or Recreate act that applies these pending unit inputs, absent for call or turn boundaries. |
| `option:ResourceApplicationGroup.apply_targets` | Only independently named dependents this current caller may manage. The existing plan rechecks every target and its consequences before confirmation. |
| `state:ResourceApplicationWhen.now` | Calls through a switched-off or deleted MCP resource are refused now in these current thread contexts. |
| `state:ResourceApplicationWhen.before_next_turn` | These current tool or skill deliveries are resolved again before the next turn. |
| `state:ResourceApplicationWhen.updated` | These running units still hold an earlier mount or recipe. Apply the offered Update or Recreate plan; restarting the same unit does not apply a different definition. |
| `state:ResourceApplicationAction.update` | The existing Update plan reviews and applies the pending configuration, including unit replacement when required. This is not the same-unit Restart act. |
| `option:ThreadOutlineGetQuery.selection` | Choose conversation for the ordinary outline, messages for human prompts with sender and turn-change facts, or agents for compact child records including older history. Omitting it selects conversation. |
| `option:ThreadOutlineGetQuery.q` | A literal text or sender search within the selected readable history, applied before paging. A matching prompt may contain the words after its displayed first line. |
| `option:ThreadOutlineGetQuery.include_changes` | Include admitted whole-turn added and removed line counts for a messages list, on by default for that selection. False leaves these optional counts unreported and avoids reading changes for navigation. |
| `option:SearchListQuery.stream` | Limit item matches to one conversation stream, such as main or a reported child stream, inside the authorized search scope before paging. |
| `option:SearchStreamListQuery.stream` | The streamed search uses the same conversation-stream selection as ordinary search; other conversations do not fill its result pages. |
| `option:PersonalState.sidebar.open_links` | The person’s Open links in choice, kept with sidebar state: automatic by default, this or new. Automatic keeps each destination in one view; this replaces the current view and new opens a view. An already open target is focused. Choose it in Interface or a tab menu; explicit clones and browser modifier clicks keep their usual behavior. |
| `option:TopicSearch.q` | The thread-name search on a topic’s Overview and Threads tab. It filters the visible live collection, travels in the topic address and is not retained as a device default. |
| `option:codex.cyber_access_program` | The Codex access program offered by the model and enabled for the provider. The selected name reaches the native turn unchanged, including names outside the Daybreak family. Normal model/account eligibility and provider access still apply. |
| `option:InputScheduleCreateBody.plan` | The authored Plan choice of every future occurrence. Required at creation; true requests a planning turn and false requests work. It is kept when the occurrence is accepted, including when an ordinary mode boundary makes it wait. |
| `option:InputScheduleCreateBody.delivery` | Required queue, steer or interrupt choice for future input. Saved and rechecked through the ordinary input writer when due; creating a schedule does not deliver it immediately. |
| `option:InputScheduleUpdateBody.plan` | An explicit Plan change is an author-only, revision-checked definition edit. Omission keeps the reviewed choice. |
| `option:InputScheduleUpdateBody.delivery` | An explicit queue, steer or interrupt change is an author-only, revision-checked definition edit. Omission keeps the reviewed choice; pausing or resuming alone does not replace it. |
| `option:ThreadGoalUpdateBody.action` | Pause, resume, or accept an unchecked completion report. Accept is the person's explicit Mark met decision; Resume on an unchecked report means Keep working and claims no verification. |
| `option:AgentProfile.config.native_result_wait_ms` | Codex's optional retained-result wait in milliseconds. Omission uses the registered system default, fitted within the engine's interrupt window. A profile's explicit valid wait is kept or refused with its reason. |
| `option:AgentProfile.config.native_result_buffer_bytes` | Codex's optional byte budget for ordered later input during an evidence read. Omission uses the registered system default. Pressure ends optional proof without dropping input or stopping another session. |
| `option:SessionSettings.web_action_wait_secs` | The resolved inherited native web-row wait sent to an agent session. Codex uses receipt time and retains ownership for a late real end; the wait never interrupts the turn. |
| `option:SessionOpen.web_action_wait_secs` | Required resolved web-row wait on the connector's session-open command, carried into the session settings and its reopen fingerprint. |
| `option:x_turn_end_delays.delays.*.default_setting` | The program's declared system registry key supplying an omitted profile wait in milliseconds. The engine resolves it and fits that default under its interrupt window; an explicit profile wait is not shortened. |
| `option:x_turn_end_delays.delays.*.pending_buffer` | A program's declaration of optional ordered-input buffering during its evidence wait. Pressure must end optional evidence and continue the input, rather than drop or refuse frames. |
| `option:x_turn_end_delays.delays.*.pending_buffer.pointer` | JSON pointer to this byte budget in the program profile's configuration. An explicit valid profile value is retained. |
| `option:x_turn_end_delays.delays.*.pending_buffer.default_bytes` | The program's fallback byte default. The engine supplies the declared registered system default when the profile leaves the pointer unset. |
| `option:x_turn_end_delays.delays.*.pending_buffer.default_setting` | The declared system registry key for the pending-input byte default. It must name an available unsigned integer setting. |
| `option:ask.models.*.model` | The enabled API model’s routing name. Together with provider_id it identifies one allowed Ask model; child lists can narrow the choices while retaining the parent’s order. |
| `option:ask.models.*.provider_id` | The API provider’s identity for this allowed Ask model. The person and current payer must still be allowed to use that provider and model in the conversation’s organization. |
| `option:AccessOptions.purposes` | Uses requested from a provider: Ask or other work. Requesting Ask alone does not authorize ordinary turns with the same model. |
| `option:ProviderUse.purposes` | Allowed uses: Ask, other work, or both. Omitted permits both; an empty list permits neither. A single entry must allow the use, model, effort and place together. |
| `option:ProviderModelDefaultsQuery.model_name` | The exact routing name to preview with the provider home’s catalogue policy. This reads stored facts; it neither starts discovery nor adds a model. An explicit create parameters object saves the complete enabled definition and keeps cleared optional choices off. |
| `option:Meta.interface_defaults` | Installation defaults for retaining recently closed views, loading indications and read freshness. Each person may override them in their own interface preferences. |
| `option:InterfaceDefaults.kept_closed_tabs` | Additional recently closed views to retain. Open views are always retained. |
| `option:InterfaceDefaults.progress_delay_ms` | Milliseconds before a read shows its spinner. |
| `option:InterfaceDefaults.catch_up_above_items` | Effective installation threshold for the automatic catch-up card. A missing fact stays Not reported; it is not zero. The reader’s saved Interface choice overrides it. |
| `option:InterfaceDefaults.read_freshness_ms` | Freshness windows for directory, lookup, report and status reads, in milliseconds. |
| `option:PersonalState.interface` | The signed-in person’s reading choices. Omitted fields use the installation defaults; changing a preference preserves the other fields. Showing a tab changes none of them. |
| `option:PersonalState.interface.kept_closed_tabs` | This person’s retained closed views, presentation and Reopen-history count. Zero keeps only the last Reopen locator. |
| `option:Workspace.closedLimit` | Last confirmed retention count in this person’s device workspace. It bounds close history before the shell returns; it never overrides the next confirmed native preference. |
| `option:ReadAttemptFacts.read_attempt` | Observed attempt of this browser’s current safe read, starting at one. Shared observers use the same flight; replacement readers receive no former facts. |
| `option:ReadAttemptFacts.reader` | Browser reader generation that owns the observation. It is a fence, not a displayed identity or permission grant. |
| `option:ReadAttemptFacts.operation` | The observation describes a safe read. Opening it never repeats a write. |
| `option:ReadAttemptFacts.phase` | The actual reading, accepted wait or failed phase of this attempt. Cancellation retires it. |
| `option:ReadAttemptFacts.scheduled_retry_at` | Device-clock instant captured when the actual scheduler accepts a retry. It is eligibility, not a promise of delivery or a deadline reconstructed from current defaults. An uncaptured time is Not reported. |
| `state:ReadAttemptPhase.reading` | This admitted read attempt is running. Opening its facts sends nothing. |
| `state:ReadAttemptPhase.waiting` | The safe read has an accepted retry wait. Cancelling or leaving its owner retires that wait and its facts. |
| `state:ReadAttemptPhase.failed` | The actual read failed without another accepted retry. The affected part retains its existing recovery action. |
| `option:SessionState.renewal` | This browser’s observed renewal phase and times, without credentials or another session’s details. A replacement grant retires the former observation. |
| `option:RenewalObservation.scheduled_at` | Next timer actually scheduled by this browser. A failed manual renewal preserves an already scheduled timer; an unknown next attempt is Not reported. |
| `option:RenewalObservation.renewed_at` | Device-clock instant when this held session’s renewal succeeded. A direct sign-in or replacement grant is not labelled as that earlier renewal. |
| `option:RenewalObservation.phase` | This browser's actual scheduled, running, completed or failed renewal, separate from the native session list. |
| `state:RenewalPhase.scheduled` | This browser has scheduled renewal. Healthy routine scheduling adds no standing explanation. |
| `state:RenewalPhase.in_progress` | The current reader’s actual renewal is running. Concurrent requests share it. |
| `state:RenewalPhase.completed` | This held session was actually renewed. The recorded completion stays separate from its next scheduled timer. |
| `state:RenewalPhase.failed` | This renewal failed. A temporary connection failure retains the held session; it does not invent an ended-session cause or replay a mutation. |
| `option:SessionState.sessionEnd` | Only an attested outcome for this browser's own ended session. A replacement sign-in retires it; missing evidence keeps the generic session-ended notice. |
| `option:SessionEndObservation.at_if_known` | Native end time when known, otherwise Not reported. The browser never calculates it from a token lifetime or the present time. |
| `option:SessionEndObservation.cause` | Only the native attested expired, revoked or account-unavailable cause for the session this browser held. Extra native fields are not retained. |
| `state:SessionEndCause.expired` | The own native session expired. A public invalid token is not proof of this cause. |
| `state:SessionEndCause.revoked` | The own native session was revoked. No other session or person's information is included. |
| `state:SessionEndCause.account_unavailable` | The own native session's account is unavailable. An unrecorded change time remains Not reported. |
| `option:DeviceChoiceObservation.scope` | The choice's existing kind, field and optional item on this person's browser. Scope identifiers are not display names or another person's device inventory. |
| `option:DeviceChoiceObservation.reader` | The signed-in browser reader generation that may see the receipt. Reader replacement makes former writers inert and retires their facts. |
| `option:DeviceChoiceObservation.origin` | Actual address, item, kind, default or current-page source. Each control validates membership against the choices it really offers. |
| `option:DeviceChoiceObservation.read_result` | Result of the same storage read that supplied the value, distinguishing available, refused, invalid and memory-held choices. Details performs no second read. |
| `option:DeviceChoiceObservation.write_result` | Captured saved, failed or not-checked outcome. Successful reads never imply a successful write. Live controls retain their refused value independently of the unobserved fallback cache. |
| `option:DeviceChoiceObservation.chosen` | The actual requested or retained choice before availability is applied. An unavailable saved choice is preserved. |
| `option:DeviceChoiceObservation.effective` | The current value actually used by this control. An explicit address wins; fallback does not rewrite an unavailable saved value. |
| `option:DeviceChoiceObservation.saved` | The saved value known from this choice's observed read. Unknown or unreadable names are Not reported, never an opaque identifier. |
| `option:DeviceChoiceObservation.saved_choice_availability` | Available, unavailable or unreported under the control's actual offered choices. This is separate from whether device storage could be read or written. |
| `state:DeviceChoiceOrigin.address` | The current explicit address supplies the value. |
| `state:DeviceChoiceOrigin.item` | This item's deliberate device choice supplies the value. |
| `state:DeviceChoiceOrigin.kind` | This kind's saved device choice supplies the value. |
| `state:DeviceChoiceOrigin.default` | The control uses its actual default, preserving any unavailable saved intention. |
| `state:DeviceChoiceOrigin.page` | This page's retained choice or current in-memory fallback supplies the value; it is not a promise of persistence. |
| `state:DeviceChoiceRead.available` | The existing device read succeeded. Absence remains a default, rather than a saved zero or Off. |
| `state:DeviceChoiceRead.unavailable` | The actual device read was refused or unavailable. The device inventory must not call this empty. |
| `state:DeviceChoiceRead.invalid` | Saved bytes were readable but not a usable choice. They remain distinct from a storage-read failure. |
| `state:DeviceChoiceRead.memory` | The current choice is held in this browser's fallback memory. An unmounted fallback may later be evicted. |
| `state:DeviceChoiceSave.saved` | The observed device write succeeded. This promises neither permanent browser storage nor another device's state. |
| `state:DeviceChoiceSave.failed` | The actual device write failed. The current control keeps its in-memory choice; a disappeared control retains only a safe kind/save observation. |
| `state:DeviceChoiceSave.not_checked` | No current write outcome was captured. It is not inferred success. |
| `option:DeviceChoiceFailure.kind` | Allowlisted safe category for a disappeared control's refused save, including file viewers, windows and list choices. No private value, identifier or false stored-byte figure is retained here. |
| `option:DeviceChoiceFailure.write_result` | The actual failed save for that current reader, retained in On this device. Clearing it does not claim an earlier save succeeded. |
| `option:Started.deviceChoice` | The existing successful-setup read's receipt. Current provider, model, agent, effort, environment and arrangement still come from the native resolver; a remembered arrangement is never new credential consent. |
| `option:PersonalState.interface.progress_delay_ms` | This person’s loading-indication delay in milliseconds. Zero shows it immediately. |
| `option:PersonalState.interface.catch_up_above_items` | This person’s nonnegative automatic catch-up threshold. Omission follows the installation default; zero includes any unread range. Explicit catch-up is independent. Saving other Interface choices preserves this field without creating an override. |
| `option:PersonalState.interface.read_freshness_ms` | This person’s directory, lookup, report and status freshness overrides, in milliseconds. |
| `state:PersonalStateKey.interface` | The native personal-state key that keeps interface reading choices separately from named layouts, sidebar choices and keyboard bindings. |
| `option:ThreadItemOutputGetQuery.expected_blob` | The exact retained blob selected by this reader. A changed output refuses this page before bytes are read, preventing mixed output pages. |
| `option:OutputRef.inline_limit` | The accepted live inline byte allowance. Small capture budgets keep equally small live and durable tails; the original total still counts every produced byte. |
| `option:OutputRef.retained` | The exact original byte interval stored in this output blob; absent for inline-only capture. A retained blob can contain only part of the produced output. |
| `option:OutputRef.retained.offset` | Original byte offset of the retained interval. Bytes before this boundary may be missing. |
| `option:OutputRef.retained.len` | Actual retained blob bytes, without a synthetic gap marker; offset plus length is the exclusive original end. |
| `option:SessionLost.uncertain` | A session-level report that the native outcome is unknown keeps the input for explicit recovery. When restart reconciliation loses an identified turn, its turn report ends that turn in Orch. |
| `option:EverythingListQuery.resource_rows` | Return compact table rows and the readable sharing summary once per home in the page. Full configuration is read from each resource when opened. |
| `option:EverythingListQuery.resource_details` | Return full resource views when table rows are not requested. This is used by choices that draw the full configuration. |
| `option:SearchListQuery.source_details` | Read only the serving facts captured for this streamed query's receipt cursor. It starts neither another search nor a model request; current search access and safe names are checked again. |
| `option:SearchListQuery.source_offset` | Continue within the same query's captured serving sources. Use the returned next offset; a native sequence gap is not an omitted source. |
| `option:SearchListQuery.source_filter` | Filter captured serving sources by their currently shared place or captured model name before paging. Withheld names and identifiers are not searched. |
| `option:ThreadOpeningGetQuery.limit` | Positive item count for this opening, overriding the resolved threads.opening_items default. Older items remain available through history cursors. |
| `option:ThreadOpeningGetQuery.continuation_id` | Choose the history continuation for this read without changing which continuation executes. |
| `option:ThreadOpeningGetQuery.projection_revision` | Name the known history projection. A stale value returns the native reset with sync.complete=false and the current selector, which can be used to open a fresh window. |
| `option:ThreadModelParameter.options_partial` | True when only first-paint parameter traits and the current or visibly recorded models are supplied. Read full parameters when a control opens for its complete choices, help and forms. |
| `option:ThreadOpening.screen` | Independent activity and plan-answer facts for this actual thread. A failed part is retried through GET /threads/{id}/screen-facts without loading model metadata or the full settings list. |
| `option:ThreadComposer.approval_default` | True when the native approval choice is supplied outside this thread. The closed approval chip can show its default mark without reading the full settings panel. |
| `option:ThreadComposer.sandbox_mode` | Effective native sandbox value for the compact control; null when no value is reported. Full choices, locks and writable revisions are read when the control opens. |
| `option:ThreadComposer.sandbox_default` | True when the sandbox choice is supplied outside this thread. Its compact control retains the default mark before its settings panel is read. |
| `option:ThreadScreenFacts.silent_after_secs` | Effective thread.silent_after_secs value in seconds. It controls the running thread's silence notice and uses the registered default unless a place sets it differently. |
| `option:ThreadScreenFacts.plan_answered_by` | Effective thread.plan_answered_by policy, writers or managers. Proposed plans use it before enabling an answer; it is read independently from model choices. |
| `option:Feed.provider_row` | The provider-row:{id} live feed carries the same authorized compact provider facts as a thread's opening. Provider View is checked again at delivery. It carries no models, source configuration or private login progress; the provider page keeps its full resource feed. |
| `option:ProviderRowEvent.provider` | Current compact thread-provider facts on a changed event. Null withdraws the previous facts when their native View authority is lost. |
| `option:AccessOptions.models` | The models. |
| `option:AccessOptions.services` | Enabled provider services this Use grant allows, by name, family or group. Omitted permits every enabled service; this list is independent of generation models. |
| `option:AccessOptions.efforts` | The efforts. |
| `option:AccessOptions.where` | The environment, or the host or machine of one that is to be made, that a refused use was asked for; it tells where the provider's entry does not reach. |
| `option:BindingOptions.viewer_default` | The current creation default and its permitted provenance; absent when rebinding an existing thread. |
| `option:BindingOptions.environment_readers` | The exact prospective page's View offer on the selected existing environment. A new environment is checked in the creator transaction before the thread is made. |
| `option:BindingOptions.defaults` | What settings choose. |
| `option:BindingOptions.providers` | The providers, each with its models and agents. |
| `option:BindingOptions.environments` | Every existing environment the caller manages. This topic's come first (its default, then those its threads use or that were made for it), and the rest follow. |
| `option:BindingOptions.environment_default` | What a new thread does about its environment unless told. |
| `option:BindingOptions.default_environment` | The environment the nearest setting names as the default, wherever it is set, and why the reader cannot use it when they cannot; absent when no setting names one and when an existing thread is bound again. |
| `option:BindingOptions.source` | Safe resolved repository settings and the place supplying the repository address. |
| `option:BindingOptions.access` | The permission page that a thread created here would begin with. |
| `option:BindingOptions.agent_access` | Agent privilege choices for this creation context. Rebinding an existing holder does not create another contribution and leaves this field absent. |
| `option:BindingOptions.new_environment` | Who would be able to see, and who to manage, an environment made now for this topic. |
| `option:BindingOptions.can_create` | Whether the caller may make a new environment. |
| `option:BindingOptions.hosts` | The hosts a new environment for a thread here could be made on, the one with the most free memory first: what each has, what is left of it, whether it can be chosen and why not, what it warns about, and whether the scheduler would choose it. Empty where the caller may make no environment here or the place names no template. |
| `option:BindingOptions.machines` | The caller's own connected machines: a thread runs on one through an environment that is a directory there. |
| `option:ChangeOptions.active_turn` | How to treat a turn that is under way. |
| `option:ChangeOptions.wait_minutes` | With waiting: how long to wait before giving up with nothing changed. |
| `option:ChangeOptions.verify_timeout_secs` | How long the health checks may take. |
| `option:ChangeOptions.keep_previous` | Keep what is left behind rather than scheduling it for removal. |
| `option:ChangeOptions.stop_processes` | For a switch out of an environment that others stay in: end this thread's processes there. |
| `option:AppliedChangeOptions.active_turn` | How to treat a turn that is under way. |
| `option:AppliedChangeOptions.wait_minutes` | How long to wait for it. |
| `option:AppliedChangeOptions.verify_timeout_secs` | How long the health checks may take. |
| `option:AppliedChangeOptions.keep_previous` | Whether what is left behind is kept. |
| `option:AppliedChangeOptions.stop_processes` | Whether this thread's processes are ended. |
| `option:ImportOptions.parts` | Which kinds of thing; all three unless given. |
| `option:ImportOptions.versions` | The earlier versions of documents, where the package has them; `true` unless given. |
| `option:ImportOptions.comments` | The discussions of documents; `true` unless given. |
| `option:ImportOptions.access` | Whether the sharing the package carries is restored, within what the importer holds and what the place's locks allow; `false` unless given, and refused for one who does not hold `access.manage` there. |
| `option:ImportOptions.authors` | By whom things are written: the accounts for an Admin of the place, and the importer otherwise. |
| `option:ImportOptions.same_source` | What is done with an object that an earlier import of the same thing made: `replace` unless given. |
| `option:ImportOptions.same_name` | What is done with an object whose name another of the place has: `keep_both` unless given. |
| `option:ParameterOptions.from_model` | Which list. |
| `option:protocols.google-interactions.web_search.search_types` | Gemini native search types: web_search for text sources and image_search for image sources. These configure an enabled Live search tool and preserve the caller’s functions. |
| `option:EndpointOptions.auth` | How the endpoint receives its credential. Gemini Interactions uses x-goog-api-key by default; bearer, x-api-key, a named header and no credential remain available for other endpoints. |
| `option:protocols.openai-responses.web_search.search_context_size` | The amount of search context requested: low, medium or high. |
| `option:protocols.openai-responses.web_search.return_token_budget` | The declared return-token budget: default or unlimited. |
| `option:protocols.openai-responses.web_search.filters` | Domain filters for this search; allow and block lists cannot be supplied together. |
| `option:protocols.openai-responses.web_search.filters.allowed_domains` | Only search the listed domains. |
| `option:protocols.openai-responses.web_search.filters.blocked_domains` | Do not search the listed domains. |
| `option:protocols.openai-responses.web_search.user_location` | Approximate location supplied for this search; it is a provider option, never inferred from the reader. |
| `option:protocols.openai-responses.web_search.user_location.type` | The approximate-location kind. |
| `option:protocols.openai-responses.web_search.user_location.city` | City used for the approximate search location. |
| `option:protocols.openai-responses.web_search.user_location.region` | Region used for the approximate search location. |
| `option:protocols.openai-responses.web_search.user_location.country` | Country code used for the approximate search location. |
| `option:protocols.openai-responses.web_search.user_location.timezone` | Time zone used for the approximate search location. |
| `option:protocols.openai-responses.native_tools.image_generation.request.model` | The image model used by the hosted tool, separate from the conversation model. Choose a model the endpoint supports within its allowed options. |
| `option:protocols.openai-responses.native_tools.image_generation.request.action` | Choose auto, generate or edit. Auto lets the tool choose; editing still needs the image references supplied with the request. |
| `option:protocols.openai-responses.native_tools.image_generation.request.background` | Choose auto, opaque or transparent. The selected image model and output format must support the requested background. |
| `option:protocols.openai-responses.native_tools.image_generation.request.output_compression` | The provider’s compression setting from 0 to 100 for JPEG or WebP output, where supported; it does not change the requested image count. |
| `option:protocols.openai-responses.native_tools.image_generation.request.output_format` | The requested image encoding: PNG, JPEG or WebP. This does not prove the format of returned bytes; the result keeps its reported format. |
| `option:protocols.openai-responses.native_tools.image_generation.request.partial_images` | Request 0 to 3 streaming previews. A preview is not another completed image; the provider may include preview work in its token usage. |
| `option:protocols.openai-responses.native_tools.image_generation.request.quality` | The requested rendering quality. Use only levels supported by the selected image model and its option constraints. |
| `option:protocols.openai-responses.native_tools.image_generation.request.size` | The requested dimensions or auto, within the selected image model’s supported sizes. It does not override provider limits. |
| `option:protocols.anthropic-messages.web_search.max_uses` | Maximum search-tool uses in this request, when set. |
| `option:protocols.anthropic-messages.web_search.allowed_domains` | Only search the listed domains. |
| `option:protocols.anthropic-messages.web_search.blocked_domains` | Do not search the listed domains. |
| `option:protocols.anthropic-messages.web_search.user_location` | Approximate location supplied for this search; it is a provider option, never inferred from the reader. |
| `option:protocols.anthropic-messages.web_search.user_location.type` | The approximate-location kind. |
| `option:protocols.anthropic-messages.web_search.user_location.city` | City used for the approximate search location. |
| `option:protocols.anthropic-messages.web_search.user_location.region` | Region used for the approximate search location. |
| `option:protocols.anthropic-messages.web_search.user_location.country` | Country code used for the approximate search location. |
| `option:protocols.anthropic-messages.web_search.user_location.timezone` | Time zone used for the approximate search location. |
| `option:SummaryTargetOptions.automatic` | Allow automatic refresh for this target. Off as shipped; explicit Refresh is separate. |
| `option:SummaryTargetOptions.on` | Events that may trigger automatic refresh. Omission uses this target’s ordinary events; an empty list disables event triggers. |
| `option:SummaryTargetOptions.min_interval_secs` | Minimum automatic-refresh interval in seconds; zero adds no minimum gap. |
| `option:SummaryTargetOptions.context_tokens` | Positive input token budget; otherwise the summarizer default is 32,000, within the available model window less output. |
| `option:SummaryTargetOptions.output_chars` | Positive output character budget. This never overrides provider limits or usage caps. |
| `option:SummaryOptionsBody.options` | The target’s own options; null removes the override and restores inheritance. |
| `option:SummaryOptionsBody.expected_revision` | The options revision read by the caller, so a changed choice must be reviewed again. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.max_uses` | Maximum page-fetch tool uses in this request, when set. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.allowed_domains` | Only fetch pages from the listed domains; it cannot be set together with blocked domains. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.blocked_domains` | Never fetch pages from the listed domains; it cannot be set together with allowed domains. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.max_content_tokens` | The largest page content, in tokens, the provider passes to the model for one fetch, when set. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.citations` | Whether fetched pages are returned with citations the answer can point to. |
| `option:protocols.anthropic-messages.native_tools.page_fetch.request.citations.enabled` | Turns citations for fetched pages on or off. |
| `option:ModelGroupOptions.request_options` | Request option forms and constraints keyed by interface; `native` names program parameters. The endpoint, group and offering constraints still apply. |
| `option:providers.presets.*.credential_hints` | Public key-format hints that propose this preset and a credential role. They never verify a key or enable an offering; the normal first read after saving confirms the vendor facts. |
| `option:providers.presets.*.credential_hints.*.prefix` | The plain public beginning of a key format. Matching only proposes a choice; the complete key stays in the form until Save. |
| `option:providers.presets.*.credential_hints.*.role` | The proposed role: model access or an admin credential for the facts source. A role the client does not recognize requires an explicit supported choice. |
| `option:providers.presets.*.credential_hints.*.field` | Optional declared secret-field name for the proposed admin credential in the selected facts source. It is kept separate from model access and must be offered by that source. |
| `option:AgentDescriptor.features` | The program’s declared feature IDs and shipped on/off states. Explicit agent.features settings override these values, including false. |
| `option:ProviderModel.parameter_forms` | Read-only request-option forms keyed by interface, projected from this model’s effective constraints. The same forms serve thread and place-default editors. |
| `option:AgentModel.parameter_forms` | The same effective request-option forms exposed with the agent’s model entry; the selected program and model still determine applicability. |
| `option:SettingParameterFormBody.definitions` | Native parameter declarations from the selected program descriptor. Resolves form metadata without opening a session or making a model call. |
| `option:SettingParameterFormBody.model` | The selected model’s enabled parameters and effective native constraints; discovery offers alone do not enable a choice. |
| `option:SettingParameterFormBody.feature_defaults` | Shipped feature states from the selected program descriptor, before explicit place choices are applied. |
| `option:SettingParameterFormBody.features` | Explicit resolved feature choices at the place. Each entry overrides the program’s shipped value; false keeps that feature off. |
| `option:SettingParameterFormBody.provider_kind` | The selected provider kind, api or subscription, used to evaluate declared parameter conditions. |
| `option:SettingParameterFormBody.scope` | Optional org, project, topic or thread scope; the form includes only parameters writable at that scope. |
| `option:SettingParameterFormBody.harness_version` | The selected program’s reported version when known, used for version-dependent parameter conditions. |
| `option:ResolvedSetting.inherited` | The effective value and its source with this place’s contribution omitted; supplied when the reader has the place context. The own contribution remains the stored choice. |
| `option:SettingInheritance.value` | The value the inherited choice gives; null means no scope supplies one. It is computed by the same native merge and default rules. |
| `option:SettingInheritance.source` | The place or declared default supplying the inherited value; concealed names retain their ordinary privacy boundary. |
| `option:ThreadInterruptBody.turn_id` | The exact turn to stop. Omit it to stop preparation of the next message before a turn exists. Stop needs no queue or history revision. |
| `option:ThreadStop.state` | The immediate Stop acknowledgement: stopping while work remains, or stopped when the target has ended. The accepted request continues after this response. |
| `option:ThreadStop.requested_at` | When Stop was requested; the elapsed stopping time begins here, including a wait for another change to finish. |
| `option:ThreadStop.turn` | The named turn as it stands in the acknowledgement. Absent when stopping preparation before a turn exists. |
| `option:Thread.stopping_since` | Reported start of the oldest outstanding Stop. Absent when no Stop remains; the thread can still show its current running or preparation phase while stopping. |
| `option:Turn.interrupted_at` | When somebody requested this turn's Stop. Absent when nobody stopped it; this is separate from its eventual completion time. |
| `option:MarkerSessionReopened.reason` | The recorded reason the session opened again before a turn, such as changed instructions or a program restart. Unchanged observable instructions keep the existing session. |


### Live file reads

| Reference | Meaning |
| --- | --- |
| `option:Thread.starting_since` | When the current preparation began; absent when no start is under way. |
| `option:Thread.starting_work` | The current preparation, its command and what is ahead of it. |
| `option:StartingWork.command` | The accepted native command for this preparation, when one is running or in line. |
| `option:StartingWork.behind` | The work that actually holds the serial line ahead; finished capture uploads do not hold later starts. |
| `option:Turn.files_changed` | Known changed paths at the turn end, from complete before/end coverage or the native change journal; absent means unknown. |
| `option:Turn.files_changed_paths` | The relative changed paths known for this turn, including changes made without a file-tool patch. |
| `option:Turn.files_changed_complete` | True means complete change coverage; false means known scoped changes, which keep independently reported tool edits. |
| `option:WorkspaceObserveResult.changed_paths_complete` | Whether native watcher coverage is complete for the observed interval; degraded coverage still returns paths it actually saw. |
| `option:WorkspaceObserve.sealed` | Omit to seal prepared end metadata and observed paths; supply that exact result to publish it without reading newer live files. |
| `option:WorkspaceObserveResult.end_patch_blob` | Immutable changed metadata over a prepared base; keeps a complete end state while the full manifest is encoded and published in the background. |
| `state:RecordKind.source` | Job-owned Current files capture, independent of automatic recording days/bytes and held by its native worktree recovery journal. |
| `option:Turn.recording_notice` | The first affected turn keeps its recording-coverage reason; a later refresh does not move the notice. |
| `option:Turn.recording_indexing` | Indexing progress when this turn started before whole recording was ready; absent progress is not guessed. |
| `option:TurnRecordingProgress.done` | Entries prepared when the turn started. |
| `option:TurnRecordingProgress.total` | Entries in that preparation pass; percentage is shown only when this is known. |
| `option:EnvironmentFileSearchQuery.cursor` | Continue the same names or content query, source and filters; omit it to start fresh. |
| `option:FileSearchNames.entries` | Matching path observations in this page; their source is stated beside the results. |
| `option:FileSearchNames.next_cursor` | The next part of the same traversal; null means it reached the end. |
| `option:FileSearchNames.source` | Live traversal, or the last complete cached file index when that is the source. |
| `option:FileSearchNames.searched` | Entries checked so far; this is not a claim that the whole directory has this many entries. |
| `option:FileSearchNames.as_of` | The end of this page observation or the cached manifest time. |
| `option:FileSearchNames.observed_from` | When the live observation began; cached results retain their own observation time. |
| `option:FileSearchContent.next_cursor` | Continue text search after this window, including a time slice with no matches. |
| `option:FileSearchContent.observed_from` | When this live text traversal began. |
| `option:EnvironmentArchiveListQuery.cursor` | Continue the same open archive and folder without rereading the compressed prefix. |
| `option:EnvironmentArchiveListQuery.q` | Match child names of the selected archive folder, without regard to case. |
| `option:ArchiveListing.next_cursor` | Continue the same archive reader; a time slice can return no rows and still continue. |
| `option:EnvironmentGitDiffGetQuery.cursor` | Continue the output of the same native Git process; omitted starts a fresh difference. |
| `option:EnvironmentGitDiffGetQuery.max_bytes` | Preferred bytes in this difference page; omitted uses the workspace default and the wire page still applies. |
| `option:DocumentFileArchiveListQuery.cursor` | Continue this currently authorized immutable document archive. |
| `option:DocumentFileArchiveListQuery.q` | Filter the selected archive folder by child name. |
| `option:UploadBlobGetQuery.cursor` | Continue an archive page only after the caller independently holds the source blob again. |
| `option:UploadBlobGetQuery.q` | Filter child names while listing this draft archive. |
| `option:UploadBlobGetQuery.limit` | Rows in this archive window; omitted uses the current archive-row default. |
| `option:EnvAttach.workspace` | Resolved file-reader defaults for this attachment; they do not make large directories unavailable. |
| `option:FsRecord.wait_for_indexing` | A chosen wait also waits for indexing; the shipped turn default starts without waiting for initial indexing. |
| `option:FsRecordResult.indexing` | Current preparation progress when whole recording is not ready. |
| `option:FsRecordResult.reason` | Why whole recording is unavailable; cancelled records a Stop and a resumed input asks again. |
| `option:FsSearch.scope` | Names or text content, both with live continuation. |
| `option:FsSearch.cursor` | Continue exactly this live query and attachment. |
| `option:FsArchiveList.cursor` | Continue the same open archive traversal. |
| `option:FsArchiveList.query` | A case-insensitive substring of the displayed child name. |
| `option:GitDiff.cursor` | Continue the same process output instead of running its prefix again. |
| `option:GitDiff.max_bytes` | Preferred bytes in the next page, within the existing wire representation. |
| `option:CmdAck.key_phase` | Actual queued, running or released ownership; acceptance alone does not pause a recording deadline. |
| `option:EnvironmentFacts.recording` | Current whole-recording coverage; incomplete coverage is never reported as a complete state. |
| `option:EnvironmentFacts.exclusion_problem` | The unapplied exclusion source, line when known and safe reason; no rule contents are disclosed. |
| `option:ExclusionProblem.path` | The relative exclusion source or configured-rule name that needs correction. |
| `option:ExclusionProblem.line` | The one-based invalid line when known. |
| `option:ExclusionProblem.reason` | A safe reason such as unreadable, linked, oversized, invalid encoding or invalid pattern. |
| `option:WorkspaceLimits.directory_entries` | Resolved workspace.directory_entries default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.search_results` | Resolved workspace.search_results default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.archive_results` | Resolved workspace.archive_results default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.index_entries` | Resolved workspace.index_entries default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.index_scan_secs` | Resolved workspace.index_scan_secs default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.ignore_max_bytes` | Resolved workspace.ignore_max_bytes default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.ignore_cache_bytes` | Resolved workspace.ignore_cache_bytes default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.directory_idle_secs` | Resolved workspace.directory_idle_secs default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.directory_slots` | Resolved workspace.directory_slots default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.directory_cache_bytes` | Resolved workspace.directory_cache_bytes default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.directory_builds` | Resolved workspace.directory_builds default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.directory_page_bytes` | Resolved workspace.directory_page_bytes default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.search_slice_secs` | Resolved workspace.search_slice_secs default for this attachment; explicit requests and continuation keep the act available. |
| `option:WorkspaceLimits.git_diff_bytes` | Resolved workspace.git_diff_bytes default for this attachment; explicit requests and continuation keep the act available. |
| `option:DirListing.revision` | The identity of this folder observation; small stable folders use an immutable snapshot, while a wide or changing folder continues its live native reader. Pages are ordered within their window and state their observation interval. |
| `option:FsListPageResult.revision` | The same native folder-observation identity across its byte-bounded pages, rather than a claim of an instantaneous whole-folder cut. |

| Reference | Meaning |
| --- | --- |
| `option:SummarySnapshot.result_facts` | Captured native output transformations and request counts of the readable paragraph. Later settings do not reconstruct them; absence means Not reported. The paragraph’s current source governors also guard these facts. |
| `option:SummaryResultFacts.flattened` | Whether the captured conformance normalized whitespace; null means the fact was not captured. |
| `option:SummaryResultFacts.first_line_only` | Whether the first nonempty line of a multiline answer was kept; null means not captured. |
| `option:SummaryResultFacts.removed_characters` | Characters cut by the actual character allowance after formatting. A whole rejected handoff is not counted as a shortened valid answer. |
| `option:SummaryResultFacts.unknown_tags` | Captured count of supplied tag names absent from the offered choices. |
| `option:SummaryResultFacts.duplicate_tags` | Captured count of supplied tag names already retained in the answer. |
| `option:SummaryResultFacts.over_budget_tags` | Captured count of known, nonduplicate tags beyond the chosen allowance. |
| `option:SummaryResultFacts.rejected_whole` | Whether the handoff was rejected whole; no raw rejected command or path prefix is retained in these facts. |
| `option:SummaryResultFacts.captured_usage` | Canonical reported request token counts retained with the result, including measured zero; missing counts stay unknown and never become a quote or refund. |
| `option:SummarySnapshot.refresh_proposal` | Read-only current model, usage owner and rendered request estimate before Refresh; it reserves nothing and never enables automation. |
| `option:SummaryRefreshProposal.revision` | Revision of the observed choice and source inputs; never an admission or payment token. |
| `option:SummaryRefreshProposal.usage_owner` | The actual manual requester’s safe person reference; never inferred from the resource owner. |
| `option:SummaryRefreshProposal.input_estimate_tokens` | Estimated tokens of the actual rendered prompt, separate from base prices and character targets. |
| `option:SummaryRefreshProposal.output_allowance_tokens` | Actual output reservation, including applicable reasoning room; a maximum rather than a quote. |
| `option:GoalReport.helper` | Captured helper admission and actual outcome; old or agent-only records have no invented helper facts. |
| `option:GoalHelperReceipt.checked` | True only for an interpreted helper answer; false for a captured pending or unchecked attempt, null when unreported. A person accepting a report does not manufacture a helper result. |
| `option:GoalHelperReceipt.input_allowance_tokens` | The admitted input allowance captured with this attempt, never a later default. |
| `option:GoalHelperReceipt.output_allowance_tokens` | The actual admitted output allowance, separate from explanation characters. |
| `option:GoalHelperReceipt.last_turn` | Actual material coverage of the selected last turn; null remains Not reported. |
| `option:NodeBase.automatic_fields` | Independent title/tag outcomes and durable scheduling facts, read under current node/thread authority. Manual ownership remains authoritative. |
| `option:SummaryAttemptFacts.captured_usage` | Canonical reported usage retained even when the result is discarded. Missing measurements never become zero. |
| `option:AutomaticFieldSchedule.due_at` | The recorded trigger due time, separate from interval/retry eligibility. |
| `option:AutomaticFieldSchedule.next_at` | Actual recorded interval/retry eligibility, not a browser prediction from a default. |
| `option:AutomaticFieldSchedule.last_attempt_at` | The actual last attempt, including failures; it is not last success. |
| `option:AutomaticFieldSchedule.last_success_at` | This field’s actual last successful write, never another field’s write time. |
| `option:AutomaticFieldSchedule.retry_ceiling` | The retry ceiling captured when the recorded timing was decided; the setting source links to its existing control. |
| `option:HistoryReport.handoff` | Actual retained handoff facts, separate from a plan estimate or the full transcript’s coverage. |
| `option:HandoffResult.input_coverage` | Captured per-turn reading and omission decisions of this bounded handoff input. |
| `option:HandoffResult.result_facts` | Actual output conformance and canonical request usage; rejected raw text is not retained. |
| `option:HandoffInputCoverage.read_turn_bytes` | The per-turn byte allowance used by the material decision, not a later setting. |
| `option:HandoffInputCoverage.included_turn_ranges` | Inclusive native turn ranges actually included as text, distinct from older index lines and the full transcript. |
| `option:HandoffInputCoverage.oversized_turn_ranges` | Inclusive native ranges omitted whole for exceeding the captured read allowance. |
| `option:HandoffInputCoverage.omitted_prefix` | Earlier selected history remained outside the actual bounded input. |
| `state:GoalHelperCoverage.none` | No last-turn material was present in this helper request. |
| `state:GoalHelperCoverage.included` | The selected last turn was included whole. |
| `state:GoalHelperCoverage.start_omitted` | The beginning of the selected turn was omitted to fit the admitted allowance. |
| `state:GoalHelperCoverage.oversized` | The turn was omitted whole because it exceeded the captured byte allowance. |
| `state:GoalHelperCoverage.items_exceeded` | The turn was omitted whole because it exceeded the chosen item allowance. |
| `state:SummaryDiscardReason.access_changed` | The publication authority generation changed. |
| `state:SummaryDiscardReason.history_changed` | The selected continuation changed before publication. |
| `state:SummaryDiscardReason.source_changed` | The source revision changed before publication. |
| `state:SummaryDiscardReason.producer_changed` | The selected field producer changed before publication. |
| `state:SummaryDiscardReason.archived` | The subject was archived before publication. |
| `state:SummaryDiscardReason.changed_by_person` | A person’s field ownership wins over this generated result. |
| `state:SummaryAttemptOutcome.published` | The result passed publication; an identical value need not be rewritten. |
| `state:SummaryAttemptOutcome.discarded` | A current publication predicate discarded this result; the current field stays authoritative. |
| `state:SummaryAttemptOutcome.unusable` | Native conformance rejected the answer while recorded usage remains counted. |
| `state:SummaryAttemptOutcome.failed` | The producer did not complete this attempt. |
| `state:AutomaticSkipReason.no_producer` | No automatic producer was selected. |
| `state:AutomaticSkipReason.cap_no_room` | The actual usage cap had no room for the attempt. |
| `state:AutomaticSkipReason.not_allowed` | Current source or model access was unavailable. |
| `state:AutomaticSkipReason.provider_unavailable` | The provider could not be used for the attempt. |
| `state:AutomaticSkipReason.call_rule_paused` | The call rules reserved no automatic continuation. |
| `state:AutomaticSkipReason.source_unavailable` | The selected source was unavailable. |
| `state:AutomaticSkipReason.unsupported` | The producer does not write this field. |
| `state:AutomaticSkipReason.failed` | The producer failed this attempt. |
| `state:AutomaticScheduleState.queued` | Actual accepted automatic work is queued or claimed; eligibility is not a promised start. |
| `state:AutomaticScheduleState.retrying` | An actual failed attempt has a recorded retry eligibility. |
| `state:AutomaticScheduleState.skipped` | The last attempt was skipped or paused with no current queued work. |
| `state:AutomaticScheduleState.idle` | No automatic work is currently queued. |
