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
| `option:ThreadOutlineGetQuery.q` | Words in the same projected outline title, goal condition or turn number, matched before the output page. Native history membership and frozen text stay unchanged. Message and subagent lists search their selected source text and sender before paging. |
| `option:ThreadOutlineGetQuery.kinds` | Comma-separated outline kinds, applied before the output page; omitted means all native kinds. |
| `option:ThreadHistoryListQuery.q` | Words in the projected segment agent/provider names and ordinal, or a turn’s model, state and ordinal, before the page fills. |
| `option:ThreadRawListQuery.q` | Words in protocol frame payloads after current masking; hidden original secrets never supply a match. |
| `option:ThreadRawListQuery.direction` | Select the direction before the matching frame page fills. |
| `option:ThreadRawListReply.next_before` | Continue below this original event number, including an empty search-progress page; absent when no older page remains. |
| `option:ModelInputListQuery.q` | Words in block keys, source labels and stored previews before paging, under the original source permissions. This finder does not search unread full blobs; Read the full text and Copy retain every byte. |
| `option:NotificationListQuery.q` | Literal stored title/body and currently disclosed place/actor words, after the native readable projection and before output paging. Place grouping retains full-group read/Undo counts; its continuation binds the search. |
| `option:AdminProgramBuildListQuery.q` | Literal version, operating system, architecture and effective check-state words, matched in every native build source before its page limit. |
| `option:AdminSearch.buildQ` | The build list’s current search in an administration address. A changed query clears that list’s continuation; other program filters remain independent. |
| `option:StorageQuery.q` | Literal words in disclosed record/native handle, place, host, owner and dependency labels, after the original storage authority/name projection and before totals or paging. Hidden host names never match. |
| `option:StorageQuery.limit` | Records per page; fifty by default, any positive u32. The work buffer is not a total inventory cutoff. |
| `option:StorageQuery.cursor` | Continue the same filtered native retained-record selection. |
| `option:PkiInfo.machines_next_cursor` | Continue the same administrator, machine-name words and captured certificate-expiry window. The report keeps the complete thirty-day due count while showing one page. |
| `option:HistoryListQuery.q` | Literal words in the same currently permitted History text, action, actor and disclosed thread/path fields. Filtering follows native projection before the output page fills; hidden raw details never match. With upload, match filenames/paths before the file window, preserving original immutable positions. |
| `option:HistoryListQuery.kinds` | Environment history uses its existing HistoryKind values. Nodes and other resource histories use the exact AuditKind catalogue, including kinds absent from the loaded page. Selection precedes the result limit and Summary grouping. |
| `option:DocumentRelatedListQuery.q` | Current readable item-name words, matched before the ranked page fills. Native related scores, reasons and eligibility remain unchanged. |
| `option:DocumentRelatedListQuery.limit` | Entries per page; any positive u32, the place's existing related-count default when omitted. |
| `option:DocumentRelatedListQuery.cursor` | Continue the same source, actor, words and ranked visible observation. A changed source or search starts fresh. |
| `option:DocumentReferencesListQuery.q` | Literal words in a readable source/target name or context, or the label/path already written in the document. With of, search rendered mention lines throughout that source. Hidden target names never match. |
| `option:DocumentReferencesListQuery.limit` | Sources/targets, or matching mentions with of, per page; any positive u32. Defaults come from the place's existing Links page/occurrence settings. Native access-work budgets yield continuation rather than capping the list. |
| `option:NodeTagListQuery.q` | Visible tag-name words, matched before paging; exact names precede partial matches. |
| `option:NodeTagListQuery.picker` | On an empty opening, put this person's recent currently available tag definitions first. Other reads keep their ordinary defining-place order. |
| `option:NodeTagListQuery.limit` | Tag definitions per page; fifty by default, any positive u32. |
| `option:NodeTagListQuery.cursor` | Continue the same actor, place, words and personal order. A changed order starts a fresh read. |
| `option:HolderSearchQuery.picker` | Open a holder chooser with this person's recent people and groups; search starts on the first character while retaining the native placed-locator grammar. |
| `option:HolderSearchQuery.kinds` | Comma-separated holder kinds; native selection applies them before the page fills. Omitted means all kinds. |
| `option:HolderSearchQuery.limit` | Holder candidates per page; fifty by default, any positive u32. This does not cap discoverable people or groups. |
| `option:HolderSearchQuery.cursor` | Continue the same actor, source place, tree, requested kinds and exact locator search. A changed visible source starts fresh. |
| `option:HolderSearchReply.next_cursor` | Native continuation of the same visible source; absent at its end. |
| `option:SemanticSearchOverview.orgs_next_cursor` | Continue the same administrator and root-org name query. Eligibility and name matching precede the page; detailed live index state is read only for its rows. |
| `option:CapabilityListQuery.q` | Literal visible definition-name, key or description words, matched before paging. |
| `option:CapabilityListQuery.limit` | Entries in one capability page; fifty by default, any positive u32. |
| `option:CapabilityListQuery.cursor` | Continue the same administrator, words and public alias-key selection. |
| `option:CapabilityListQuery.aliases` | Comma-separated public capability keys whose shipped localized name or description also matches. This supplements ordinary stored text search before paging and never bypasses system administrator authority. |
| `option:MePickerRecentReceipt.count` | Current system group length, returned with the write acknowledgement. No stored private history is returned by a write-only operation. |
| `option:MePickerRecentReceipt.remembered` | True when this selection was recorded; false when remembering is off. Off neither records this choice nor erases earlier preferences. |
| `option:UserSearchQuery.include_external` | With an explicit org, also offer the existing shared-org and exact public people search outside it. False by default; the same native org view and account scopes apply before paging. |
| `option:UserSearchReply.org_member_ids` | When include_external is true, IDs among this page's people who actually belong to the selected org. Otherwise absent; a picker does not infer guest status from loaded pages. |
| `option:UserSearchQuery.picker` | A choice picker accepts one character; empty input offers this person's recent active people and shared-org people. An org selection still restricts membership; current account scopes are checked before paging. |
| `option:UserSearchQuery.limit` | People in one page; twenty by default, any positive u32. This never caps the people that can be found. |
| `option:UserSearchQuery.cursor` | Continue the same actor, org, picker mode and literal words. A changed opening order starts a fresh page. |
| `option:MePickerRecentChoice.kind` | Native resource kind or model. All screens share one private history per kind. |
| `option:MePickerRecentChoice.key` | Stable opaque choice key; model choices include their provider. Records a deliberate selection, never a permission or a name. |
| `option:MePickerRecents.count` | Current system default for entries remembered and shown per kind; zero disables remembering. |
| `option:MePickerRecents.choices` | This person's newest-first distinct keys by kind, read in one batch. The current native picker still decides which entries are available. |
| `option:PersonalStateKey.picker_recents` | Private server preference used by all pickers. POST /me/picker-recents atomically promotes one kind/key; PUT /me/state/picker_recents with an empty object clears all of it. |
| `option:NameSearchListQuery.picker` | Picker mode: empty words include all currently available names, with this person's recent selections first. A single typed character filters natively. |
| `option:NameSearchListQuery.cursor` | Continue the same actor, scope, kinds, participation, starred selection and words. A changed recent order starts a fresh read. |
| `option:NameSearchPage.next_cursor` | Continue beyond this currently authorized name page; absent at the end. |
| `option:ProviderModelSuggestionListQuery.refresh` | When true, explicitly reread the source. Otherwise pages and searches use its last successful catalogue; an unread source is loaded once. Refreshing remains available through the provider’s Refresh act. |
| `option:ProviderModelSuggestionListQuery.q` | Words in a visible source proposal’s model name; matched before paging. |
| `option:ProviderModelSuggestionListQuery.limit` | Entries in this proposal page: 50 by default; every continuation remains available. |
| `option:ProviderModelSuggestionListQuery.cursor` | The previous next_cursor from the same provider, search and unlisted choice. |
| `option:ProviderModelSuggestionListQuery.unlisted` | When true, omit model names already saved on this provider before paging. The complete discovered catalogue remains available with false or omission. |
| `option:AdminRuntimeListQuery.q` | Words in the visible unit, resource or template name, matched before paging. |
| `option:AdminRuntimeListQuery.limit` | Entries in one page; 50 by default, any positive u32. |
| `option:AdminRuntimeListQuery.cursor` | Continue the same host, kind and name search. |
| `option:EverythingListQuery.resource_kind` | Select a template unit kind, a machine/template environment origin, or system for agent profiles without a home; applied before name search and paging. |
| `option:NativeSessionListQuery.q` | Words in a native session title or folder, matched within the captured source before paging. |
| `option:NativeSessionListQuery.limit` | Entries in this page; 50 by default, any positive u32. |
| `option:NativeSessionListQuery.cursor` | Continue the same source and search; it cannot select another catalogue. |
| `option:NativeSessionListQuery.source` | The opaque source returned by the first page. Omit it to discover afresh. It is bound to the caller, provider, profile and credential/configuration generation; expiry or a changed source returns a field fault and the UI restarts that read. |
| `option:InvitationListQuery.active` | When true, exclude expired, revoked and used-up links before paging; false or omission retains the full governed history. |
| `option:InvitationListQuery.q` | Words of the visible destination, email address, note or inviter. |
| `option:InvitationListQuery.limit` | Entries in one page; 50 by default, any positive u32. |
| `option:InvitationListQuery.cursor` | Continue the same org, active choice and search. |
| `option:NotificationChannelListQuery.q` | Words in the visible channel name or notifier kind, before paging. |
| `option:NotificationChannelListQuery.limit` | Entries in one page; 50 by default, any positive u32. |
| `option:NotificationChannelListQuery.cursor` | Continue the same owner selection and search. |
| `option:EnvironmentMcpListQuery.collection` | The native catalogue: servers by default, or found definitions. |
| `option:EnvironmentMcpListQuery.q` | Words in the selected catalogue’s visible names, program or directory. |
| `option:EnvironmentMcpListQuery.limit` | Entries in a page; 50 by default, any positive u32. |
| `option:EnvironmentMcpListQuery.cursor` | Continue the same environment, catalogue and search. |
| `option:HostRuntimeListQuery.q` | Words in the readable environment/provider name, driver or state. |
| `option:HostRuntimeListQuery.limit` | Entries in a page; 50 by default, any positive u32. |
| `option:HostRuntimeListQuery.cursor` | Continue the same host, kind and search. |
| `option:MachineMountListQuery.collection` | Locations by default, available additions, or environments attached to mount_id. |
| `option:MachineMountListQuery.mount_id` | The located mount whose readable attachments are selected with collection=environments. |
| `option:MachineMountListQuery.q` | Words in visible mount names/paths or attached environment names. |
| `option:MachineMountListQuery.limit` | Entries in a page; 50 by default, any positive u32. |
| `option:MachineMountListQuery.cursor` | Continue the same machine, collection, mount and search. |
| `option:NotificationRouteListQuery.retained` | Keep only removed-channel routes when true, current routes when false, or all stored routes when omitted. |
| `option:NotificationRouteListQuery.q` | Words in the safe channel name, kind or connected label, before paging. |
| `option:NotificationRouteListQuery.limit` | Entries in a page; 50 by default, any positive u32. |
| `option:NotificationRouteListQuery.cursor` | Continue the same person, retained choice and search. |
| `option:NodeChildListQuery.q` | Visible child-name words, matched with the indexed native name predicate before paging. |
| `option:NodeDocumentListQuery.q` | Visible title words in both readable and administrative-name phases, before paging. |
| `option:InputScheduleListQuery.q` | Visible schedule-name words before paging. |
| `option:InputScheduleListQuery.thread_id` | Keep only schedules whose readable destination and agent origin address this thread. |
| `option:SharedListQuery.q` | Visible name, description and disclosed path words, matched after native projection and before paging. |
| `option:SharedListQuery.kind` | Only org, project, topic, thread, document or document_container; omission includes all. |
| `option:SharedListQuery.limit` | Entries requested in this page; 50 by default, any positive u32. |
| `option:SharedListQuery.cursor` | Continue the same actor, kind and search with the returned cursor. |
| `option:PermissionPeopleListQuery.q` | Visible person-name or username words before paging; actual directories include everyone eligible beyond bounded refusal previews. |
| `option:OrgChatListQuery.include_empty` | With the nodes scope, include current readable place chats before their first message, so they remain forwarding destinations. False by default; ordinary activity lists keep their existing meaning. |
| `option:OrgChatListQuery.ids` | Optional comma-separated chat identities, resolved in one batch through the same native org, scope, participation and current account admission. Unavailable chats are omitted; supplied IDs grant no access. |
| `option:OrgChatListQuery.limit` | Chats per page; fifty by default, any positive u32. |
| `option:OrgChatListQuery.cursor` | Continue the same actor, root, scope, participation, identity filter and words. A changed read starts a fresh page. |
| `option:OrgChatListQuery.q` | Visible conversation title, participant names or disclosed node name; matched through the native name projection before the result page fills. |
| `option:ChatGetQuery.members_q` | Visible group-member names and usernames, projected by ChatGet before paging. Omission preserves the complete ordinary metadata view. |
| `option:ChatGetQuery.members_limit` | Entries in a requested member page; any positive u32, with 50 supplied by the roster screen. |
| `option:ChatGetQuery.members_cursor` | The members_next_cursor from the same chat and search. |
| `option:CollectionQuery.q` | Words or quoted phrases in disclosed collection text, matched on the server without a minimum length. Spaces inside a quoted phrase remain literal. A changed search starts a fresh continuation. Sent-message lists match their headings and recorded sender names. Active document notice work matches state, phase, progress text and date. |
| `option:CollectionQuery.limit` | Entries requested in this page: 50 by default, any positive u32 when supplied. This changes a page, never the collection’s total. |
| `option:CollectionQuery.cursor` | The opaque next_cursor returned by the same collection and search. Follow it until null; it cannot be reused for another parent or search. |
| `option:ChildCollectionQuery.collection` | The child directory: MachineGet agents/providers/environments; HostGet kept_layers/kept_data; MountGet locations/kept/lost_locations. Only that child directory is returned; unselected child arrays are empty. Omit all child-query fields for the complete native resource read. |
| `option:ChildCollectionQuery.q` | Words in the chosen child directory’s disclosed names and location text. Hidden names and paths never participate. |
| `option:ChildCollectionQuery.limit` | Positive entries in one child page; fifty by default, without a total-result cutoff. |
| `option:ChildCollectionQuery.cursor` | The same reader/resource/child/search continuation. A changed child or search starts at its first page. |
| `option:HostKeptLayers.latest` | Whether this is the newest visible kept generation for its workspace. The native complete observation decides it before paging. |
| `option:EnvironmentGetQuery.inspect_deleted` | Use the workspace’s current restoration authority when inspecting kept data. The normal live read is unchanged. |
| `option:EnvironmentGetQuery.threads_q` | Words in disclosed bound-thread, topic, owner or directory text. The server searches the complete native visible projection before selecting its page. |
| `option:EnvironmentGetQuery.threads_limit` | Positive size of one explicit bound-thread page (50 by default). Omitting all threads_* fields preserves the complete environment read used for functional inputs. |
| `option:EnvironmentGetQuery.threads_cursor` | The next bound-thread continuation from the same reader, workspace and search. Clear it when the search changes. |
| `option:AdminTokenListQuery.q` | Words in the administrative token inventory’s visible name. Search runs before paging; continuations belong to the same administrator, owner and search. |
| `option:NodeDeletedEnvironmentListQuery.q` | Words in a kept workspace’s current name, matched on the server before its page is selected. Clear the cursor when the search changes. |
| `option:OrgGroupListQuery.q` | Words of the group name, searched before paging. |
| `option:OrgGroupListQuery.limit` | Entries in one page; 50 by default, any positive value. |
| `option:OrgGroupListQuery.cursor` | Continue the same org, descendant choice and search. |
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
| `option:DirListing.revision` | The identity of this folder observation; small stable folders use an immutable snapshot, while a wide or changing folder continues its live native reader. Pages are ordered within their window and state their observation interval. |
| `option:FsListPageResult.revision` | The same native folder-observation identity across its byte-bounded pages, rather than a claim of an instantaneous whole-folder cut. |
