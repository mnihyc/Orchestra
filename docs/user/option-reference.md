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
| `option:ForkPlanMemory.files` | The measured source note-file count; null means not measured, never zero. |
| `option:ForkPlanMemory.bytes` | The measured source note bytes; null means not measured. |
| `option:Segment.native_history` | Physical cache observations for this exact session, independent of the conversation retained by Orch. |
| `option:NativeHistoryLifetime.state` | Only a positive native inventory/removal produces retained/removed. Missing inventory is unreported, never inferred from an in-memory open hint. |
| `option:NativeHistoryLifetime.idle_eligible_at` | The eligibility captured by the actual retention supplier, or explicit off/unreported. It never promises removal starts then. |
| `option:NativeHistoryLifetime.policy_days` | The actual idle policy. Zero disables automatic native cache removal. |
| `option:NativeHistoryLifetime.policy_source` | The captured policy layer without an unadmitted ancestor name or raw native reference. |
| `option:NativeHistoryLifetime.policy_observed_at` | When the enforcing supplier assessed its policy. A later read does not resolve it again. |
| `option:NativeHistoryLifetime.removed_at` | The time of positively confirmed physical removal; this does not erase readable conversation data. |
| `option:NativeHistoryLifetime.observed_at` | The last successful physical observation. |
| `option:ForkPlanMemory.proposal` | The conditional notes rule and admitted source/target directories, not proof that data was copied. |
| `option:MemoryProposal.source_directory` | The source-relative note directory; null withholds its name. |
| `option:MemoryProposal.target_directory` | The target-relative note directory; null withholds its name. |
| `option:MemoryProposal.proposed_fate` | Shared, copy_if_absent, not_carried or source memory off. |
| `option:MemoryProposal.observed_target_revision` | The metadata revision observed with the selected target. It does not prove physical absence; native publication rechecks that. |
| `option:Job.memory_outcome` | The retained native notes decision; a completed result never borrows a later plan. |
| `option:MemoryOutcome.fate` | The actual shared/copied/target_kept/not_carried/off decision. A kept directory is never inferred from zero written files. |
| `option:MemoryOutcome.at` | When the actual decision was recorded. |
| `option:MemoryOutcome.cleanup_pending` | A changed or unavailable private copy stage was preserved rather than deleting uncertain work. |
| `option:JobActivity.captured_deadline` | The actual native execution timer, captured by the same dispatcher that enforces it and joins cleanup. It is not command admission or a silence estimate. |
| `option:JobWorkDeadline.command` | The exact owning invocation for stale-report checks; never a visible identifier fallback. |
| `option:JobWorkDeadline.phase` | The immutable declared job phase owning this allowance. Internal file substeps do not reset it. |
| `option:JobWorkDeadline.accepted_at` | When the enforcing native dispatcher accepted execution. |
| `option:JobWorkDeadline.at` | The wall-clock instant corresponding to that same native monotonic timer. Later settings and counters cannot move it. |
| `option:JobWorkDeadline.seconds` | The accepted execution allowance in seconds. |
| `option:JobWorkDeadline.policy` | The captured system origin, key and row revision; null means unreported. |
| `option:JobWorkPolicy.key` | The existing public work or measurement setting actually used. |
| `option:JobWorkPolicy.origin` | default is the registry; system is an explicit system row. |
| `option:JobWorkPolicy.revision` | The captured system row revision, or null for a registry default. |
| `option:Measurement.timing` | The native assessment belonging to this exact count reference. |
| `option:MeasurementTiming.cache_valid_until` | The reuse boundary under the policy assessed then, separate from future rechecks. Pending and inapplicable remain explicit. |
| `option:MeasurementTiming.observer_grace_until` | The retained eligibility for cancelling unwatched active work, not a promise of the exact stop time. Completion makes it inapplicable. |
| `option:MeasurementTiming.assessed_at` | When the native supplier made this assessment. |
| `option:MeasurementTiming.cache_policy` | The validity policy origin and row revision actually assessed. |
| `option:MeasurementTiming.observer_policy` | The observer-grace policy origin and row revision actually used. |
| `option:Job.measurement_timing` | Retained native count facts, without resolving a newer setting on read. |
| `option:Thread.environment_preparation` | The current native environment operation, withdrawn when it no longer holds this thread. |
| `option:ThreadEnvironmentPreparation.kind` | The actual accepted create/start job or issued attach command; generic source/file work never supplies this fact. |
| `option:ThreadEnvironmentPreparation.environment` | The same environment reference admitted for this thread, never a later replacement source. |
| `option:ThreadEnvironmentPreparation.actual_job` | The real provisioning job when job-backed; connected attach does not fabricate a job. |
| `option:ThreadEnvironmentPreparation.actual_command` | The issued native attach command when the work is not job-backed. Internal command identity is not a display label. |
| `option:ThreadEnvironmentPreparation.observed_at` | When the producer accepted the job or issued the command, independent of browser timing. |
| `option:ThreadBinding.configuration_application` | The captured comparison with the last acknowledged native session. A metadata read never recomputes it. |
| `option:ConfigurationApplication.accepted_session_revision` | The monotonically increasing successful opening number within this segment, or null when unreported. |
| `option:ConfigurationApplication.accepted_at` | The actual acknowledged setup time, independent of pending requested settings. |
| `option:ConfigurationApplication.pending_kinds` | All categories the native supplier actually compared as changed; null means the accepted setup is unavailable. |
| `option:ConfigurationApplication.deferred_by_background` | Native work held the pending setup even when the ordinary reopen marker was suppressed. |
| `option:ConfigurationApplication.reload_policy` | The observed program declaration: each_turn or on_change. |
| `option:ConfigurationApplication.observed_at` | When the native supplier made the comparison; never a browser prediction. |
| `option:Job.history_coverage` | A completed fork or switch's retained history receipt. Missing means not reported; current policy does not reconstruct it. |
| `option:ForkPlanHistory.selected_range` | Inclusive native ordinal bounds selected in the current proposal, independent of loaded pages. |
| `option:ForkPlanHistory.context_allowance` | The actual computed allowance basis of the proposed history transport. |
| `option:HistoryTurnRange.from` | The first native ordinal. |
| `option:HistoryTurnRange.through` | The last native ordinal, inclusive. |
| `option:HistoryContextAllowance.share` | The resolved percentage actually used. Null means no captured model-window share applies. |
| `option:HistoryContextAllowance.tokens` | The computed context token allowance, not measured usage. The text byte estimate stays distinct. |
| `option:HistoryContextAllowance.source` | The actual calculation basis: declared model window, unknown-window brief, native session, real summary, summary fallback or no history. |
| `option:HistoryContextAllowance.setting_scope` | The captured carry-policy layer, without an unadmitted ancestor name. System sources use the existing setting destination. |
| `option:HistoryCoverage.method` | The history transport that actually succeeded, including a summary fallback. |
| `option:HistoryCoverage.selected_range` | The confirmed native selection bounds. |
| `option:HistoryCoverage.actual_ranges` | Compact exact ordinals represented in context. Summary context has no literal ranges. |
| `option:HistoryCoverage.partial_turn` | A turn cut by the actual byte allowance. Null means no partial turn was recorded. |
| `option:HistoryCoverage.omitted_prefix` | Selected turns before this ordinal were omitted from context, independently of the transcript. |
| `option:HistoryCoverage.context_form` | Native, literal, brief, derived summary or no source context. |
| `option:HistoryCoverage.transcript_coverage` | The separate transcript file's recorded delivery: complete, partial, inapplicable or unreported. The readable conversation is retained independently. |
| `option:HistoryCoverage.context_allowance` | The actual captured allowance, not a later edit to the carry policy. |
| `option:HistoryCoverage.at` | When successful transport coverage was recorded. |
| `option:MachinePolicy.limits` | Applicability and physical authority from the same native role, capability and reader cut that supplied the keys. Missing means not reported. |
| `option:MachineLimits.unit_context` | The addressed machine is an enrolled runtime unit; physical policy commands still name the actual operator program. |
| `option:MachineLimits.applicable_keys` | The native supplier certifies these keys. A unit screen uses this list instead of inventing applicability from its UI kind. |
| `option:MachinePolicyApplicability.key` | The key matched to its existing definition and control. |
| `option:MachinePolicyApplicability.governing_resource` | The resource category bounded by the enforcing catalogue: environment, provider or physical machine. |
| `option:MachinePolicyApplicability.authority_source` | physical_operator keeps the value on the physical program; delegated uses the existing Orch policy authority. This does not grant this reader write access. |
| `option:Job.lifecycle_wait` | Captured coordination facts for this job, read under its current ordinary authority. Opening Details starts no work. |
| `option:JobLifecycleWait.phase` | The actual phase, or cleanup after an accepted abort. |
| `option:JobLifecycleWait.retry_eligible_at` | The actual recorded earliest retry, explicitly unreported or inapplicable where appropriate. It does not promise a start. |
| `option:JobLifecycleWait.lease_recoverable_at` | The current captured runner lease boundary, distinct from retry and from work completion. |
| `option:JobLifecycleWait.last_attempt_at` | The retained claim or retry attempt time. Renewing a lease does not replace its accepted attempt time. |
| `option:JobLifecycleWait.reason` | The same public failure wording as the ordinary job. Private runner diagnostics are not returned. |
| `option:JobLifecycleWait.reason_code` | The public failure catalogue key used to present that reason in the reader's language. |
| `option:JobLifecycleWait.recovering` | Whether this claim recovered an earlier execution; null when that fact was not captured. |
| `option:JobLifecycleWait.observed_at` | When the newest included lease or retry fact was recorded; null means unreported. |
| `option:JobWaitTime.state` | at carries a finite captured instant; unreported and not_applicable are distinct and do not mean zero or an unlimited wait. |
| `option:JobWaitTime.at` | The captured eligible instant when state is at. Eligibility is not a guarantee of worker availability. |
| `option:Environment.idle_stop_deferred` | An enabled unit idle stop is waiting for active turns or retained native work. Its deadline is withheld while the work runs; this is distinct from an idle policy that is off. |
| `option:Environment.removal_keep_days` | The prospective owned-file recovery window after deployment limits and temporary cleanup. Null means no unit policy can currently promise a window. An accepted removal keeps its own frozen window. |
| `option:Environment.attach_retry` | A connected directory's last refused attach and its captured retry eligibility. Missing means no reported attempt; limited through-thread replies omit it. Eligibility does not promise that an attempt starts then. |
| `option:EnvironmentAttachRetry.last_attempt_at` | When the recorded refused attach ended. |
| `option:EnvironmentAttachRetry.next_eligible_at` | The earliest retry in that machine session, captured with the refusal and used by the sweep independently of later metadata edits. |
| `option:EnvironmentAttachRetry.last_problem` | The refused attempt's ordinary environment status detail. |
| `option:EnvironmentAttachRetry.revision` | The environment configuration revision used by the recorded attempt. |
| `option:EnvironmentDeleteQuery.expected_keep_days` | The recovery window reviewed before deletion. A changed policy refuses before bindings or files change; review the new window before trying again. |
| `option:EnvironmentDeleteQuery.expected_revision` | The environment revision reviewed with its roots and name. A changed environment refuses before removal rather than applying the old review to new data. |
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

| `option:NodeArchivePlan.retention` | The enforcing scheduler’s current automatic eligibility; it is separate from review expiry. |
| `option:NodeDeletePlan.retention` | Current automatic eligibility, separate from the accepted recovery cutoff. |
| `option:NodeRetention.archive` | Automatic topic archive policy and its real activity hold. |
| `option:NodeRetention.delete` | Automatic deletion policy for an archived node. |
| `option:NodeRetention.measured_at` | When this native eligibility was assessed. |
| `option:NodeRetentionPolicy.days` | Effective days; zero disables the action, null means the action does not apply. |
| `option:NodeRetentionPolicy.source` | Safe governing layer, without private ancestor names or identifiers. |
| `option:NodeRetentionPolicy.eligible_at` | Eligibility, never a promise that the next sweep starts at that instant. |
| `option:NodeRetentionPolicy.hold_reason` | The same active-work or archive-state condition used by the scheduler. |
| `option:DeletedInspection.cleanup_hold` | For restoration-authorized node inspections, the current physical dependency hold; it does not extend purge_at. |
| `option:NodeCleanupHold.held` | True only when the native purge worker’s actual dependency predicate holds this node. |
| `option:NodeCleanupHold.measured_at` | When the physical dependency predicate was evaluated. |

| `option:Browser.lifecycle` | Native browser eligibility and recorded retirement, separate from admission tickets. |
| `option:Display.lifecycle` | Native display gate and recorded retirement; it does not borrow the browser idle policy. |
| `option:EnvironmentBrowsers.next_cursor` | Continue the same state and literal search in this authorized environment. |
| `option:EnvironmentDisplays.next_cursor` | Continue the same state and literal search in this authorized environment. |
| `option:InteractiveLifecycle.gate` | The actual current public environment or policy gate. |
| `option:InteractiveLifecycle.idle_close_eligible_at` | Native browser idle eligibility; watched, announced and display sessions are not subject to this timer. |
| `option:InteractiveLifecycle.closed_entry_expires_at` | No independent erasure deadline is captured for the row; environment retention owns its purge. |
| `option:InteractiveLifecycle.driving_idle_expires_at` | Not applicable for continuously permission-gated sockets; a ticket deadline is never a driving lease. |
| `option:InteractiveLifecycle.retirement_reason` | The recorded native close reason, never inferred from a hidden row. |
| `option:InteractiveLifecycle.idle_minutes` | The effective browser idle policy; zero switches it off, null means not applicable. |
| `option:InteractiveLifecycle.policy_source` | Safe layer of the governing idle policy. |
| `option:InteractiveLifecycle.evaluated_at` | When native eligibility and current gates were assessed. |
| `option:EnvironmentBrowserListQuery.closed` | Request retained closed rows on intent, without native discovery. |
| `option:EnvironmentBrowserListQuery.query` | Literal case-insensitive search of readable names and notes. |
| `option:EnvironmentBrowserListQuery.cursor` | Continue this environment, state and search by the returned identity cursor. |
| `option:EnvironmentBrowserListQuery.limit` | Preferred positive page size; all matching rows remain reachable. |
| `option:EnvironmentDisplayListQuery.closed` | Request retained closed rows on intent, without native discovery. |
| `option:EnvironmentDisplayListQuery.query` | Literal case-insensitive search of readable names and notes. |
| `option:EnvironmentDisplayListQuery.cursor` | Continue this environment, state and search by the returned identity cursor. |
| `option:EnvironmentDisplayListQuery.limit` | Preferred positive page size; all matching rows remain reachable. |
| `option:DirListing.revision` | The identity of this folder observation; small stable folders use an immutable snapshot, while a wide or changing folder continues its live native reader. Pages are ordered within their window and state their observation interval. |
| `option:FsListPageResult.revision` | The same native folder-observation identity across its byte-bounded pages, rather than a claim of an instantaneous whole-folder cut. |
| `option:MachineDirectoryListQuery.root` | One selected entry of the machine’s environment.roots policy; the machine rechecks its current policy on every page. |
| `option:MachineDirectoryListQuery.path` | Parent directory relative to that root; empty means the root. Absolute paths and parent traversal are refused natively. |
| `option:MachineDirectoryListQuery.prefix` | A child-name prefix for this level only; files and outside-root symlinks are not choices. |
| `option:MachineDirectoryListQuery.recent` | JSON array of this reader’s recent child names; it ranks only directories admitted by the current native read and grants no access. |
| `option:MachineDirectoryListQuery.cursor` | Opaque continuation of the same root, parent, filter and ordering. Changed native membership or policy requires a fresh page. |
| `option:MachineDirectoryListQuery.limit` | Positive requested page count; absent uses System workspace.directory_entries. The captured workspace.directory_page_bytes also bounds a page; continuation keeps every directory reachable. |
| `option:MachineDirectoryPage.entries` | Actual immediate directory names under current machine registration authority. Browsing creates no environment and reads no file contents. |
| `option:MachineDirectoryPage.next_cursor` | Continuation of this admitted level, or null when complete. The original reader must still hold registration authority when the native answer arrives. |
| `option:MachineDirectoryListQuery.read_kind` | Choices (the default) enumerates one bounded directory page; Inspect checks the exact native path and policy without walking its children before registration. |
| `option:MachineDirectoryPage.read_kind` | The actual native read: Inspect is a path check, never a claim of an empty directory; Choices supplies the page and continuation. |
