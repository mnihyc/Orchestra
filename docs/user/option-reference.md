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
| `option:UploadSessionPutReply.expires_at` | The idle expiry retained by the receiving carrier after the acknowledged prefix. Progress renews the captured interval; a later default edit does not change an existing session. |
| `option:UploadSessionCreateBody.to` | The workspace destination selected before any bytes are sent. Omission retains a private upload on the API. File relay chooses the native carrier; the actual machine and incarnation stay bound until this session ends. |
| `option:UploadDestination.environment_id` | The logical destination environment. Its current Files management and remote-write policy still govern staging and publication. |
| `option:UploadDestination.path` | The initial workspace-relative path, including the file name. Replace and Keep both choose the final checked path after upload. |
| `option:UploadDestination.thread` | An optional thread beside which the person is working. It supplies no permission to write the environment. |
| `option:UploadedBlob.relay` | The verified native upload session when content is staged only at its destination. It grants no retained private attachment source; absence means ordinary stored upload custody. |
| `option:UploadSessionCompletion.file` | The actual verified file receipt in the ready alternative. Pending verification has no file receipt. |
| `option:UploadSessionCompletion.retry_after_ms` | The resolved file-relay retry pause before checking native verification again. Asking again does not resend accepted bytes. |
| `option:EnvironmentUploadWriteBody.relay` | The verified native session for this workspace write. The job prepares without the environment key, then publishes after checking the target version; omission uses an authorized retained blob. |
| `option:FetchState.reason` | Why the selected file cannot be read: offline for a live relay, offline_not_cached for cached mode without a retained version, ignored, too_large or not_found. Pending source hashing stays fetching until its native completion or failure. |
| `option:CredentialForm.expires` | An access-token copy expires at the time its issuer reports for that copy, even when no fixed duration is declared. It carries no renewal credential. |
| `option:ArrangementConsentForm.expires` | The shared credential has an expiry reported by its sign-in; an absent fixed lifetime is not a promise that it lasts indefinitely. |
| `option:TokenCreated.token` | The API key secret, shown once: opat_, the key’s own 32 lowercase UUID hex digits, an underscore, 43 unpadded base64url characters holding 256 random secret bits, and a six-character zero-padded Base62 CRC-32/ISO-HDLC checksum over all preceding text. Older opaque forms are refused. |
| `option:TokenCreateReceipt.token` | The same canonical identity-bound API key as TokenCreated.token, shown once in an acknowledgement; null on a repeated creation. |
| `option:CreateProvider.runtime_location` | The connection location: a new container using its host/template choices, a person’s machine, or an existing environment the caller may use. Environment connections follow its availability and persistent home. |
| `option:ProviderConnectionLocation.kind` | container, machine or environment selects one location through the same control. |
| `option:ProviderConnectionLocation.machine_id` | The selected machine for a machine location; its declared connection services and current Use authority apply. |
| `option:ProviderConnectionLocation.environment_id` | The selected logical environment. Its current own connector supplies the connection after a move or rebuild; credentials placed there are readable to its agents and terminal users. |
| `option:ProviderSetup.environments` | Existing environments the caller may use, with actual connector services; unavailable choices retain their reason. |
| `option:ProviderSetupEnvironment.environment` | The environment name and current state for the connection location choice. |
| `option:ProviderSetupEnvironment.connector` | The current own connector and declared services; absent when no own connector exists. |
| `option:Provider.connection_environment` | The environment keeping this connection, including stopped or removed state. A separately selected credential keeper remains independent. |
| `option:ProviderListQuery.connection_environment_id` | List readable providers whose connection belongs to this environment. |
| `option:AgentRuntime.environment_home` | Keep an environment-hosted connection’s own harness home under that environment’s persistent home, so ordinary home transfer carries it. An explicitly chosen existing directory remains explicit. |
| `option:FileRenameResult.kept_entries` | Entries observed left at the source after a cross-filesystem move, including their containing directories. Only copied entries whose identity and version still match are removed, children first; ignored, new or changed entries stay. An unreadable subtree counts once. Filesystems without stable file identities keep the source copy. |
| `option:ProviderStartStopBody.force` | Stop a provider even while its own work is stuck. Other work and running turns can still require a separate decision. The record stops at once; the host is told again until it confirms. |
| `option:HostRuntimeStopBody.force` | Apply the same forced-stop behavior to a provider runtime from its host page. Environment runtimes do not support this option and return an explicit refusal. |
| `option:HostRuntimeDeleteQuery.force` | End the provider’s own work that prevents deletion, then remove its runtime with the chosen volume treatment. Work that has committed and a replaced runtime keep their explicit safeguards. |
| `option:ProviderRuntime.detail` | The reason for the runtime’s current state. A forced stop says that host confirmation is pending, then becomes the ordinary person-requested stop once the host confirms. Host-supplied diagnostics remain the host’s own words. |
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
| `option:InterfaceDefaults.read_freshness_ms` | Freshness windows for directory, lookup, report and status reads, in milliseconds. |
| `option:PersonalState.interface` | The signed-in person’s reading choices. Omitted fields use the installation defaults; changing a preference preserves the other fields. Showing a tab changes none of them. |
| `option:PersonalState.interface.kept_closed_tabs` | This person’s retained closed views, presentation and Reopen-history count. Zero keeps only the last Reopen locator. |
| `option:Workspace.closedLimit` | Last confirmed retention count in this person’s device workspace. It bounds close history before the shell returns; it never overrides the next confirmed native preference. |
| `option:PersonalState.interface.progress_delay_ms` | This person’s loading-indication delay in milliseconds. Zero shows it immediately. |
| `option:PersonalState.interface.read_freshness_ms` | This person’s directory, lookup, report and status freshness overrides, in milliseconds. |
| `state:PersonalStateKey.interface` | The native personal-state key that keeps interface reading choices separately from named layouts, sidebar choices and keyboard bindings. |
| `option:ThreadItemOutputGetQuery.expected_blob` | The exact retained blob selected by this reader. A changed output refuses this page before bytes are read, preventing mixed output pages. |
| `option:OutputRef.inline_limit` | The accepted live inline byte allowance. Small capture budgets keep equally small live and durable tails; the original total still counts every produced byte. |
| `option:OutputRef.retained` | The exact original byte interval stored in this output blob; absent for inline-only capture. A retained blob can contain only part of the produced output. |
| `option:OutputRef.retained.offset` | Original byte offset of the retained interval. Bytes before this boundary may be missing. |
| `option:OutputRef.retained.len` | Actual retained blob bytes, without a synthetic gap marker; offset plus length is the exclusive original end. |
| `option:SessionLost.uncertain` | The native outcome is unavailable rather than confirmed lost. The admitted turn and its input remain retained; no implicit retry is made. |
| `option:EverythingListQuery.resource_rows` | Return compact table rows and the readable sharing summary once per home in the page. Full configuration is read from each resource when opened. |
| `option:EverythingListQuery.resource_details` | Return full resource views when table rows are not requested. This is used by choices that draw the full configuration. |
| `option:ThreadOpeningGetQuery.limit` | Positive item count for this opening, overriding the resolved threads.opening_items default. Older items remain available through history cursors. |
| `option:ThreadOpeningGetQuery.continuation_id` | Choose the history continuation for this read without changing which continuation executes. |
| `option:ThreadOpeningGetQuery.projection_revision` | Name the known history projection. A stale value returns the native reset with sync.complete=false and the current selector, which can be used to open a fresh window. |
| `option:ThreadModelParameter.options_partial` | True when only first-paint parameter traits and the current or visibly recorded models are supplied. Read full parameters when a control opens for its complete choices, help and forms. |
| `option:ThreadOpening.screen` | Independent activity and plan-answer facts for this actual thread. A failed part is retried through GET /threads/{id}/screen-facts without loading model metadata or the full settings list. |
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
| `option:TranslationOptions.allowed` | Whether a selected pairing may use format conversion under its current place policy. |
| `option:TranslationOptions.output_tokens` | The captured fallback output allowance when neither the request nor the offering supplies one. |
| `option:ProtocolTranslation.from` | The caller or harness interface of the selected conversion. |
| `option:ProtocolTranslation.to` | The actual outgoing upstream interface. |
| `option:ProtocolTranslation.title` | The converter name shown with the selected pairing. |
| `option:ProtocolTranslation.losses` | Declared preserved, approximated and omitted behavior for this conversion. |
| `option:ConversionLosses.preserved` | Behavior whose meaning is preserved across the interfaces. |
| `option:ConversionLosses.approximated` | Behavior carried in an equivalent or reduced form, including custom tool grammars and caching. |
| `option:ConversionLosses.dropped` | Features with no translated equivalent; these are disclosed before choosing the pairing. |
| `option:Isolation.translation` | The selected conversion and its fidelity, when reported; absence is direct or unreported. |
| `option:Resolution.translation` | The actual generation conversion captured with the capability resolution. |
| `option:Provider.translated_protocols` | Additional converted interfaces offered by the provider, separate from its native endpoints. |
| `option:UpstreamSide.model` | Captured model declarations and reported output maximum for this exact translated request. |
| `option:UpstreamSide.output_tokens` | Captured fallback output allowance used only if the request and offering have none. |
| `option:UpstreamSide.session` | Opaque stable session cache key; it contains no original session identity or person. |
| `option:ProviderStart.translation_output_tokens` | The resolved translated-output default captured when the provider is started. |
| `option:Provider.relay` | The key provider’s explicit public relay choice, shown only to its current configuration reader. |
| `option:ProviderRelayOptions.enabled` | Permit direct model API calls; off by default. |
| `option:ProviderRelayOptions.models` | Exact enabled model names opened to public calls; empty opens none. |
| `option:ProviderRelay.lease` | The one admitted model lease for this public call; no model credential. |
| `option:ProviderRelay.relay_id` | Command-bound memory exchange, first for the request and then its response. |
| `option:ProviderRelay.path` | The interface’s allowed request path below the public SDK base. |
| `option:ProviderRelay.headers` | Admitted interface headers without the Orch credential. |
| `option:ProviderRelay.request_bytes` | Exact normalized request length, used to bound the input phase. |
| `option:ProviderRelay.request_hash` | Request digest verified before any upstream call. |
| `option:ProviderRelay.idle_secs` | Captured silence allowance of the memory exchange. |
| `option:ProviderRelay.retry_ms` | Captured transient retry interval. |
| `option:ProviderRelayHead.status` | Actual upstream HTTP status. |
| `option:ProviderRelayHead.headers` | Repeated response header octets, with connection, credential and cookie headers removed. |
| `option:ProviderRelay.max_request_bytes` | The captured public request setting also limits the outgoing native request, so raising it needs no separate connector setting. |
| `state:UsagePurpose.relay` | Direct model API calls through a key provider, counted for their actual caller. Empty cap purpose filters include Relay. |
| `option:ProviderRelay.start` | Current provider configuration and credential references delivered only to the keeping connector; no Orch API credential or model input. |
| `option:ProviderUpdateBody.relay` | Replace a key provider’s public relay switch and exact opened models; the normal revision and configuration reader apply. |
| `option:ModelParameters.request_options.common` | One vendor-neutral schema for declared cache and schema-output options, composed through the same endpoint, group and model layers. |
| `option:ModelGroupOptions.request_options.common` | The common feature rules of an existing named model group. |
| `option:EndpointOptions.request_options.common` | Common feature rules on the actually selected native endpoint. |
| `option:common.cache` | Declared key/retention controls. Each interface supplies its native fields; unsupported members or retention values are refused with their reason. |
| `option:common.structured_output` | Declared output schema, with optional name and strictness where the native interface represents them. |
| `option:LeaseOpen.request_options.common` | Explicit thread choices from the same common schema; they override overlapping format-specific choices and do not reopen a blocked value. |
| `option:FeatureBinding` | A wire format’s data mapping from a common feature to native JSON paths, discriminators and creation defaults. |
| `option:FeatureField` | A canonical feature member’s native pointer, intrinsic value schema, available choices and factual default. |
| `option:common.structured_output.name` | Logical schema name retained with the common choice; interfaces with a schema-name field also send it. |
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


### Captured provider operation facts

| Reference | Meaning |
| --- | --- |
| `option:ProviderRuntimeSupport.retry_after_secs` | Resolved creation retry interval; manual Start retries immediately. |
| `option:Provider.facts_refresh` | Read-only current Refresh eligibility under the same durable claim; opening the page does not observe the vendor. |
| `option:ProviderRefreshEligibility.eligible` | Whether ordinary explicit Refresh may claim a read now. |
| `option:ProviderRefreshEligibility.next_at` | Next known eligibility from the durable attempt, deployment interval and source minimum; absent before an attempt or outside representable time. |
| `option:ProviderRefreshEligibility.source_minimum` | The source minimum, rather than the deployment interval, controls reuse. |
| `option:HarnessState.home` | Evidence from this process start; current settings never relabel an older report. |
| `option:HarnessHome.path` | The home passed to the actual adapter; absent when the native program chooses its own unreported path. |
| `option:HarnessHome.source` | The runtime choice, profile choice, managed binding or preserved native default captured for the actual start. |
| `option:AgentStartResult.home` | Native home evidence also returned when reusing the existing process. |
| `option:ProviderStart.upstream_policy` | Operational model exchange policy captured by the same settings resolver for native and public calls. |
| `option:UpstreamPolicy.head_secs` | Seconds to wait for response headers or a whole reply. |
| `option:UpstreamPolicy.idle_secs` | Seconds without a streamed response fragment. |
| `option:UpstreamPolicy.whole_bytes` | Whole reply and owned generation byte allowance; only addressable memory bounds the representation. |
| `option:CommandError.details.upstream` | The failed upstream rule with its captured seconds or bytes, retained in the normal failure line. |
| `state:HarnessHomeSource.runtime` | An explicit runtime home choice. |
| `state:HarnessHomeSource.profile` | The profile home choice. |
| `state:HarnessHomeSource.managed` | A home made or bound by the managed connector. |
| `state:HarnessHomeSource.native` | The existing native home is preserved; an unknown path stays unreported. |


### Recorded placement calculation

| Reference | Meaning |
| --- | --- |
| `option:PlaceHost.calculation` | Actual scheduler calculation captured for this proposal; absent means no recorded calculation. |
| `option:Job.result.calculation` | The actual placement calculation retained with the change result; it never borrows current defaults. |
| `option:RuntimeIsolation.assumptions` | Captured sources of boundary, stop persistence and overhead; absent on an unreported record. |
| `option:PlacementCalculation.captured_at` | When the deciding scheduler captured its inputs. |
| `option:PlacementCalculation.declared_totals` | Existing units’ declared CPU, memory and disk before overhead, never measured utilization. |
| `option:PlacementCalculation.applied_totals` | The exact totals used by this scheduler’s capacity tests. |
| `option:PlacementCalculation.runtime_overhead` | Runtime memory additional to unit declarations, with unknown and excluded amounts kept distinct. |
| `option:PlacementCalculation.capacities` | The capacity report used for this calculation. |
| `option:PlacementCalculation.capacities_as_of` | Receipt time of that actual capacity report; unknown stays absent. |
| `option:PlacementCalculation.thresholds` | Captured fullness threshold and incoming memory need. |
| `option:PlacementCalculation.consequence` | The actual warn or block policy at calculation time. |
| `option:PlacementCalculation.sources` | Declaration and setting layers actually used, without private machine policy or hidden resource names. |
| `option:PlacementCalculation.isolation` | Isolation resolved from the selected driver, runtime and template before placement. |
| `option:PlacementRuntimeOverhead.existing_memory_mb` | Additional memory actually counted for existing units, in MiB. |
| `option:PlacementRuntimeOverhead.incoming_memory_mb` | The incoming runtime class’s declared overhead, or absent when unknown. |
| `option:PlacementRuntimeOverhead.incoming_included` | Whether incoming overhead was included in the actual room test. |
| `option:PlacementRuntimeOverhead.source` | Declared class or unknown fallback behind the incoming overhead. |
| `option:PlacementThresholds.full_percent` | The effective hosts.full_percent used by this decision. |
| `option:PlacementThresholds.incoming_memory_mb` | The incoming declared memory used for the free-room comparison. |
| `option:PlacementCalculationSources.needs` | The resolver branch supplying each incoming resource need. |
| `option:PlacementCalculationSources.capacity_unreported` | Missing capacity members; a numeric fallback does not make them measured zero. |
| `option:PlacementCalculationSources.template_revision` | The actual template revision read for the decision. |
| `option:PlacementCalculationSources.full_percent` | System setting supplying the fullness percentage. |
| `option:PlacementCalculationSources.consequence` | Host choice or system setting supplying warn versus block. |
| `option:RuntimeAssumptions.boundary_source` | Source of the effective isolation boundary. |
| `option:RuntimeAssumptions.stop_persistence_source` | Source of the stop-persistence claim; unreported never means data is lost. |
| `option:RuntimeAssumptions.overhead_source` | Source of runtime memory overhead; unknown never becomes measured zero. |
| `state:RuntimeAssumptionSource.runtime_declaration` | Selected runtime class declaration. |
| `state:RuntimeAssumptionSource.driver_declaration` | Driver confinement declaration. |
| `state:RuntimeAssumptionSource.unknown_runtime_fallback` | The runtime was unresolved and the implementation used its fallback. |
| `state:RuntimeAssumptionSource.not_reported` | No declaration supplied this fact. |
| `state:PlacementNeedSource.resolved_needs` | Explicit or inherited needs setting. |
| `state:PlacementNeedSource.template_needs` | Template needs declaration. |
| `state:PlacementNeedSource.resolved_resources` | Effective resource limits, including template ceilings. |
| `state:PlacementNeedSource.template_resources` | Template resource declaration. |
| `state:PlacementNeedSource.not_reported` | No declared value; the scheduler used its existing zero fallback. |
| `state:PlacementSettingSource.system` | The system setting, including its declared default. |
| `state:PlacementSettingSource.host` | The host’s explicit when-full choice. |
| `option:Job.calculation` | The actual placement calculation retained with the job, including later failure or cancellation. Old records remain unreported; current settings never fill them. |
| `option:HarnessStatus.home` | Fresh actual process home carried by the ordinary status report, including supervised recovery and connector reconciliation. Missing evidence never selects a new home. |
