---
group: providers
screens: [/help, /thread/$threadId, /resources/providers/$id, /settings/interface, /topic/$topicId]
keywords: [reference, settings, options, states, errors, 参考, 设置, 选项, 状态, 错误]
summary: Shipped request, interface and provider options, and what they change.
---

# Option field reference

An option is offered only where its interface or program declares it. Provider limits and permissions still apply. Fields you define in a custom schema keep that schema’s own meaning; this reference covers the shipped declarations. Setting fields are in the [settings reference](settings-reference.md). Read the plan before a change and [the program guide](register-your-program.md#choose-an-arrangement) before choosing an arrangement.

## Entries

Provider revisions describe editable configuration. Factual refreshes, runtime status and learned source metadata keep that revision current; a concurrent configuration edit still makes an explicitly supplied revision stale.

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
| `option:EverythingListQuery.resource_kind` | Select a template’s unit kind, an environment’s machine or template origin, or a pool’s model, provider, environment or host family. For agent profiles without a home, select system. The filter applies before name search and paging. |
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
| `option:EnvironmentMcpGetQuery.thread_id` | Read the current native MCP connection observations of this thread in its present environment. The caller must also hold View on that thread; omission shows discoveries without borrowing any thread’s runtime state. |
| `option:EnvironmentMcpGetQuery.collection` | The native catalogue: servers by default, or found definitions. |
| `option:EnvironmentMcpGetQuery.q` | Words in the selected catalogue’s visible names, program or directory. |
| `option:EnvironmentMcpGetQuery.limit` | Entries in a page; 50 by default, any positive u32. |
| `option:EnvironmentMcpGetQuery.cursor` | Continue the same environment, thread selection, catalogue and search. |
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
| `option:JobRef.clone` | Received bytes and objects for the current source clone and its idle allowance. |
| `option:CloneProgress.bytes` | Repository transfer bytes received by this attempt. |
| `option:CloneProgress.objects` | Objects Git has received during this attempt. |
| `option:CloneProgress.idle_seconds` | Seconds without progress that stop this attempt. |
| `option:Problem.allowed_upstreams` | Allowed upstream addresses disclosed by the machine-policy refusal only to a current machine manager; an empty list means none, while omission withholds the list. |
| `option:Problem.field` | The exact request field associated with structured refusal facts; a machine-upstream refusal points to its upstream URL field. |
| `option:Problem.reason` | A machine_upstream refusal reports the provider.upstreams policy and, for a machine manager, its allowed addresses; a harness_error retains its native startup cause. |
| `option:Problem.policy` | The exact policy key reported by the deciding gate for a harness or upstream refusal. It stays structured through public error sanitization; the interface names the policy and its recovery act. |
| `option:AgentRuntime.build` | Exact system-listed artifact for a missing harness. The machine uses the same verified cache as a host, subject to agent.programs and tools.fetch; explicit local programs take precedence. |
| `option:AgentRuntime.build.name` | The registered harness kind; it must match the selected agent. |
| `option:AgentRuntime.build.version` | The selected catalogue version, including any profile pin. |
| `option:AgentRuntime.build.url` | The catalogue artifact address, never a path supplied by the thread. |
| `option:AgentRuntime.build.sha256` | The exact artifact SHA-256 checked before publication or execution. |
| `option:AgentRuntime.build.job_id` | Absent for a harness start; only a provisioning job owns tool-download progress. |
| `option:ProviderDiscoverModels.max_report_bytes` | Captured whole-response allowance for this discovery, including the complete normalized catalogue; resolved from providers.upstream_whole_bytes, whose default is 16 MiB. |
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
| `option:TokenCreated.token` | The API key secret, shown once: opat_, the key’s own lowercase hyphenated UUID, an underscore, 43 unpadded base64url characters holding 256 random secret bits, and a six-character zero-padded Base62 CRC-32/ISO-HDLC checksum over all preceding text. Older opaque forms are refused. |
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
| `option:native_model.program_output.max_output_tokens` | The admitted program model step’s output allowance in tokens. It stays with that request across process restarts. |
| `option:native_model.program_output.schema` | The response schema supplied by the model consumer when its adapter enforces structured output. It grants no source access or tool authority. |
| `option:BackendRef.config.environment_id` | For program-session backends without a source thread environment, the exact environment to admit. Ordinary thread work always keeps its real environment and directory; no first-online environment is selected. |
| `option:BackendRef.config.helper_topic_id` | An existing topic in the source governance root for a program helper when its real source has no topic. A source topic takes precedence. Placement grants no creator, environment or provider permission. |
| `option:CapabilityBackendQuery.place` | The actual node for a capability backend preview. Omit it only when choosing a global summary or Ask model policy; that reads provider metadata and current Use, without claiming source policy, placement or caps. Actual work always checks its own source. |
| `option:SummaryModelChoice.backend` | The complete backend for one explicitly requested summary, including its route, sign-in, model and placement. A retry retains that choice and its options; no API-only identity is reconstructed. |
| `option:SummarySnapshot.problem.reason.source_changed` | The source changed after model work may have begun. Keep the prior paragraph; Summarize again is a new explicit paid act. |
| `option:SummarySnapshot.problem.reason.settings_changed` | The accepted summary policy changed. Keep the prior paragraph and review the current choice before asking again. |
| `option:SummarySnapshot.problem.reason.content_changing` | The source exhausted its configured unpaid rereads before model admission. Keep the prior paragraph and wait for stable content or ask again deliberately. |
| `option:SummarySnapshot.program` | Retained subscription execution facts. The ordinary helper and its owner appear only with current helper Read; the actual sign-in and its plan appear only with current provider Read. Opening these facts performs no model work. |
| `option:SessionOpen.program_work.input_files` | Content hashes of frozen images that the selected model receives by file. The connector verifies their bytes and stages them under hash-derived names in the disposable unit. |
| `option:DriverDescriptor.program_units` | This placement's actual support and unavailable reason for isolated capability units. A restricted proxy and an enforced network boundary are required; another placement's support grants nothing here. |
| `option:TicketWanted.program.expires_at` | The original work's absolute deadline for its private program channel. Re-signing a stored command cannot extend it or grant source Read. |
| `option:ProgramPlacement.environment_id` | The explicit environment reference of a threadless program consumer. Null leaves placement unset; a missing, malformed or empty reference cannot start work. |
| `option:ProgramPlacement.helper_topic_id` | The explicit helper topic when the producer has no actual topic. It must belong to the producer governance root. Provider credentials and arbitrary program input are not placement fields. |
| `option:CapabilityFailure.reason.program_sign_in_changed` | The opened private program context belongs to a previous native sign-in. A fresh request can use the current sign-in. |
| `option:Resolution.reason.program_unit_unavailable` | The selected sign-in has no available node for an isolated capability unit. |
| `option:Resolution.reason.program_unit_network_unavailable` | The node driver needs a restricted provider proxy and an isolated network to run this capability. |
| `option:SummaryWait.reason.usage_measurement` | Automatic subscription work waits for a current plan measurement before it can apply the chosen reserve. |
| `option:SummaryWait.reason.plan_reserve` | Automatic subscription work waits while an applicable window is at or below its configured reserve. The previous paragraph stays readable. |
| `option:Resolution.reason.program_placement_invalid` | The selected program backend contains malformed placement references or fields that do not belong in placement. Correct the choice; the backend is retained and no alternative sign-in or environment is selected automatically. |
| `option:Support.execution` | The actual program adapter reports implemented input/result codecs and verified streaming, read-tool bridge and confinement support. A delegated mode or provider name alone proves none of them. |
| `option:Route.execution` | Required input/result codecs and execution facilities for this route. Preview and execution both require matching installed code; choosing a route starts no work. |
| `option:ResolvedRoute.execution` | The exact declared consumer requirements retained with the selected route until execution. They are technical requirements, not credentials or access grants. |
| `option:ExecutionSupport.input` | The exact input codec identifier understood by the installed adapter. Different request shapes need their actual codec. |
| `option:ExecutionSupport.result` | The exact result codec identifier. A text answer, viewed file or attached image does not prove another result family. |
| `option:ExecutionSupport.streaming` | True only when native incremental events and durable replay are implemented; required by consumers that stream answers. |
| `option:ExecutionSupport.read_tools` | True only when a source-bound read-tool bridge is implemented. This does not grant source Read. |
| `option:ExecutionSupport.confined_tools` | True only when native execution is verified to confine tools to the admitted consumer catalogue. An instruction asking the program to behave is insufficient. |
| `option:Resolution.reason` | A safe reason why the chosen capability route is unavailable. execution_codec_unavailable means its installed executor lacks a required codec or facility; program_execution_unavailable means the native adapter has not verified this consumer. No other provider is selected automatically. |
| `state:TerminalNotAllowedReason.environment_stopping` | An accepted Stop prevents new shells and ends access to existing shell sessions. |
| `state:BrowsersOff.environment_stopping` | An accepted Stop ends browser and display access while the environment finishes stopping. |
| `state:BundledNodeEnrollment.waiting_for_administrator` | Waiting for the first administrator to register the bundled host. |
| `state:BundledNodeEnrollment.waiting_for_node` | Bundled host registered · waiting for the node to enroll. |
| `state:BundledNodeEnrollment.token_file_unreadable` | The API cannot read the bundled host token file. |
| `state:BundledNodeEnrollment.invalid_token_file` | The bundled host token file is invalid. |
| `state:BundledNodeEnrollment.different_authority` | The bundled host token was issued by another installation. |
| `state:BundledNodeEnrollment.registration_unavailable` | The bundled host could not register with the database. |
| `option:MachineFacts.data_volume_bytes` | Capacity in bytes of the filesystem holding the connector data directory. |
| `option:MachineFacts.capacity_refresh_secs` | Maximum ordinary reporting interval while this machine is connected. A probe takes a fresh measurement. |
| `option:BindingEnvironment.capacity` | Latest readable capacity of the connected machine providing this thread environment. |
| `option:MoveMachine.capacity` | Latest readable machine capacity for this directory destination. |
| `option:PlaceEnvironment.capacity` | Latest readable capacity of the connected machine providing this directory. |
| `option:MachineCapacity.cpus` | Processor count available to the connector. |
| `option:MachineCapacity.memory_bytes` | Total machine RAM in bytes. |
| `option:MachineCapacity.data_volume_bytes` | Capacity of the connector data filesystem in bytes. |
| `option:MachineCapacity.capacity_refresh_secs` | Ordinary capacity reporting interval in seconds. |
| `option:EnvironmentDisplayCreateBody.desktop` | Share the local desktop where the machine offers a sharing program. |
| `option:EnvironmentSupportVnc.desktop` | The connected machine can share its local desktop. |
| `option:EnvironmentFactsVnc.desktop` | The machine observed a local X11 desktop and an installed sharing program. |
| `option:DisplayAdopt.target` | Select the local endpoint or the machine desktop. |
| `option:DisplayAdoptResult.endpoint` | The local port, socket or desktop name selected on the machine. |
| `state:DisplayTarget.desktop` | Share the local desktop with admitted display viewers. |
| `state:DisplayTarget.endpoint` | Use the specified machine-local RFB server. |
| `option:EnvironmentDisplayCreateBody.attach` | Attach an existing VNC server on the machine. |
| `option:DisplayServer.port` | Use this nonzero TCP port on the machine’s loopback interface. |
| `option:DisplayServer.password` | The machine uses the optional sealed password for each RFB connection. Viewers receive a display ticket. |
| `option:Machine.execution_approvals` | Current execution decisions verified by the server for this Machine view; native facts cannot supply their people. The Programs screen reuses this projection without another request. |
| `option:MachinePrograms.approvals` | Current execution decisions whose manager and time the server can verify; a machine report cannot supply people for this list. |
| `option:MachineExecutionApproval.subject` | The exact currently selected program kind and path. |
| `option:MachineExecutionApproval.by` | The manager recorded by the server, shown as a person reference. |
| `option:MachineExecutionApproval.at` | When that manager recorded the decision. |
| `option:ExecutionApproval.by` | The actual manager; a missing historical name does not manufacture a new author. |
| `option:ExecutionApproval.at` | The recorded decision time, retained when the decision is unchanged. |
| `state:MachineExecutionSubject.system` | A manager reviewed the exact connector binary on this operating system and architecture. |
| `option:MachineExecutionSubject.build` | Native build covered by this system decision. |
| `option:MachineFacts.runtime_platform` | Current native build offered for review on an unverified system. |
| `option:CleanupPlan.mount_switch_off_blocked` | Visible places where the mount cannot be switched off and the count of other restricted places. |
| `option:Problem.mount_switch_off_blocked` | When a mount deletion is refused, an account can see the reviewed places it can still name and the count of other restricted places. Read the deletion plan again for the current list. |
| `option:MachinePolicy.platform_review` | Current reviewable build and the decision in force on this machine. |
| `option:MachinePlatformReview.build` | System, architecture and executable being offered for review. |
| `option:MachinePlatformReview.approved` | Whether the actual connector build matches the current approval. |
| `option:MachinePlatformReview.approval` | Verified manager and time of a delegated decision. |
| `option:EnvironmentContext.reviewed_native_system` | The local connector has a matching review for this unverified native build. |
| `option:PlatformBuild.os` | Operating system of the running connector. |
| `option:PlatformBuild.arch` | Processor architecture of the running connector. |
| `option:PlatformBuild.executable` | BLAKE3 digest of the connector executable. |
| `option:PlatformConsent.state` | Whether this machine has an active build review. |
| `state:PlatformConsent.off` | No unverified native build has been approved. |
| `state:PlatformConsent.approved` | The machine owner or manager reviewed the specified native build. |
| `option:PlatformConsent.build` | The specific OS, architecture and connector digest reviewed. |
| `option:PlatformConsent.confirmation` | Manager and time verified by the server for a delegated decision. |
| `option:LocalPolicy.exec.platform` | Native system approval under the machine’s own policy. |
| `state:PolicyKey.exec.platform` | Machine policy for confirming the running connector on an unverified system. |
| `option:MachineExecutionSubject.kind` | The kind of confirmed machine execution subject. |
| `option:MachineExecutionSubject.harness_kind` | The adapter whose selected program this decision covers. |
| `option:MachineExecutionSubject.path` | The current selected program path; another path cannot inherit its attribution. |
| `state:MachineExecutionSubject.program` | A program selected on this machine under its actual local policy. |
| `option:PolicyHarness.confirmation` | The server-recorded manager and moment; absence means a local selection without an invented person. Attribution grants no permission. |
| `option:Confirmation.by` | The actual manager recorded by the server, never inferred from local configuration. |
| `option:Confirmation.at` | When the manager made the unchanged factual decision. |
| `option:MachineProgramsRequest.request` | The original native program act; the HTTP request does not supply attribution. |
| `option:MachineProgramsRequest.confirmation` | Server-recorded manager and time for confirming a program; null for other program acts. |
| `option:McpBinding.server_id` | The same registered MCP UUID carried by the machine-link binding; the adapter-facing mirror preserves it through a read/write round trip. |
| `option:Job.kind` | A mount_copy job performs a reviewed standalone data copy through the normal progress and outcome rows; server_update runs the release updater on the bundled node. Notify on mount_copy keeps one renewable Inbox notice per affected person through progress and outcome; leaving it off sends none. Copy progress names a waiting machine only when the current reader may read that machine. |
| `option:JobProgress.message` | `mount_copy_custody_pending` means cleanup keeps the job running until remaining copy data has a durable kept-data record. Its progress line and tooltip use the reader’s language. |
| `option:MountCopyPreflightBody.source_machine_id` | The existing machine holding this mount location; the server retains its exact membership, path or owned volume. |
| `option:MountCopyPreflightBody.target_machine_id` | The allowed destination machine; the server chooses native storage from its current data-making declaration. |
| `option:MountCopyPreflightBody.into` | Replace or New location when the destination already holds data; either choice keeps the displaced data. |
| `option:MountCopyPlan.preflight_id` | The saved copy plan used to start the transfer. |
| `option:MountCopyPlan.expires_at` | When this review expires; a changed native source requires a fresh review sooner. |
| `option:MountCopyPlan.mount` | The current readable mount being copied. |
| `option:MountCopyPlan.source` | The original readable machine from which data is copied. |
| `option:MountCopyPlan.target` | The readable destination machine. |
| `option:MountCopyPlan.copy` | The shared measured copy plan, including destination data and its preservation; unfinished counts remain unknown. |
| `option:MountCopyPlan.blockers` | Current reasons that stop acceptance; reviewing a blocked copy mutates no data. |
| `option:MountCopyPlanReceipt.projection` | A restricted acknowledgement of the saved review, without existing resource content. |
| `option:MountCopyPlanReceipt.mount_id` | The exact mount named by this request. |
| `option:MountCopyPlanReceipt.preflight_id` | The saved review identity, without its measured or private location details. |
| `option:MountCopyPlanReceipt.expires_at` | The deadline to accept this review. |
| `option:MountCopyCreateBody.preflight_id` | The exact current review to accept; repeating its acceptance joins the durable job. |
| `option:AdapterEvent.native_opening_id` | The producer's physical opening, captured from its admitted SessionSpec before output waits; null is reserved for unscoped history and cannot control a live session. |
| `option:ModelRequestReport.native_opening_id` | The physical opening that observed a request; delivery and replay preserve it, and measured usage remains attached to its original logical turn. |
| `option:Ev.native_opening_id` | The original producer carried beside a durable session event; earlier reports retain their facts without controlling a replacement. |
| `option:EvbEntry.native_opening_id` | Each batched observation keeps its own producer; batching, coalescing and restart cannot relabel it from a current session. |
| `option:SessionOpenMcp.server_id` | Every offered session MCP server carries its registered UUID alongside its current name; a name alone is refused. |
| `option:Cmd.native_members` | The member names and nonces frozen with an accepted session configuration. MCP and model changes compare their own members at the native effect boundary; another independent member can change without blocking them. |
| `option:Cmd.native_revision` | The factual revision captured with the original session members. A retry retains it; delivery does not stamp the command from a newer configuration. |
| `option:ToolSessionControl.refreshing` | A primary is synchronizing current scope facts. Local access waits; independent retained remote carriers keep their own live relay admission. |
| `option:Frame.tool_session_update` | Committed session and member withdrawals reach only the original source machine’s authenticated primary connection; the notice installs no native route. |
| `option:Frame.tool_session_control` | Current API control availability suspends local tool exchanges while retaining their native conversation. |
| `option:ToolSessionUpdate.machine_id` | The original source machine of this physical opening; another machine’s primary or a tunnel-only attachment cannot receive its native control. |
| `option:ToolSessionUpdate.thread_id` | The exact thread whose committed physical scope changed. |
| `option:ToolSessionUpdate.segment_id` | The original logical segment; an update never selects a newer segment as its target. |
| `option:ToolSessionUpdate.opening_id` | The physical opening whose access changed, including when its segment is removed. |
| `option:ToolSessionUpdate.revision` | Monotone membership facts; an older notice cannot restore access or retire a successor. |
| `option:ToolSessionUpdate.live` | False withdraws the whole opening; true preserves only the existing members whose nonces still match. |
| `option:ToolSessionUpdate.members` | The remaining factual member names and nonces; this list grants no new route or permission. |
| `option:ToolSessionMember.member` | The exec, llm or stable MCP member identity within one physical opening. |
| `option:ToolSessionMember.nonce` | An unchanged member’s identity; removing and adding it again produces a different nonce. |
| `option:ToolSessionControl.available` | Whether the live API can currently validate this installation’s session tools. |
| `option:ToolSessionControl.admission_timeout_ms` | The positive configurable allowance for an API admission question, from tunnels.control_timeout_ms. |
| `option:ToolSessionQuery.n` | One pending admission question; a later answer cannot be reused for another channel. |
| `option:ToolSessionQuery.claims` | The exact original verified ticket claims presented for Open or Resume. |
| `option:ToolSessionQuery.ticket` | The original signed ticket, verified again by the API and never included in diagnostic output. |
| `option:ToolSessionReply.n` | The one pending question this answer settles, once. |
| `option:ToolSessionReply.allowed` | Whether the current native opening and member admit this exact request. |
| `option:ToolSessionReply.revision` | The committed native revision actually read for that admission. |
| `option:Cmd.native_target` | The original physical operand accepted for a native act. A known absent session remains absent for that act; later execution never looks up a replacement. It grants no permission. |
| `option:LifecycleTarget.observed` | The earlier observed physical source in this original act. Explicit null is required where this act has no earlier observed source; an untracked runtime is never converted to known absence. |
| `option:LifecycleTarget.opening` | The distinct proposed opening of a fresh Create or a new Start, recorded before its first effect. Fresh Create never adopts an existing observed or pending unit; a genuine running Start reuse keeps its actual earlier source. |
| `option:LifecycleTarget.pending` | The exact earlier admitted births or starts that may already have performed their effect before their replies. Required explicit array, empty when none; it grants no permission and never includes a later unrelated source. |
| `state:LifecycleAction.create` | An original fresh allocation with its own opening and no observed or inherited pending unit. It must prove a fresh slot before effect; null alone is no absence proof. |
| `option:RuntimeSpec.labels.orch.birth` | Native fresh-allocation marker set from the admitted birth opening. Drivers refuse to reuse or rewrite an existing unit for it; exact canonical retries retain their recorded physical reference. |
| `option:NodeRuntimeStopResult.outcome` | A Stop reports its actual stopped source or proves its exact original targets absent. An empty or unknown result cannot confirm physical absence. |
| `option:NodeRuntimeStopResult.source` | The actual tracked native source confirmed stopped under the native effect lock, retained for the next Start and original-only reconciliation. |
| `option:NodeRuntimeStopResult.originals` | The exact original or admitted opening identities proven absent; this does not select or clear an unrelated current unit. |
| `state:NodeRuntimeStopResult.stopped` | The named actual native source is stopped; data is retained according to the existing reviewed choice. |
| `state:NodeRuntimeStopResult.absent` | The exact original targets are proven absent or closed; an untracked source is not such proof. |
| `option:NativeTarget.session` | A captured session operand carried by the current machine-link protocol; an older receiver cannot silently ignore it. |
| `option:SessionTarget.thread_id` | The exact thread of the accepted native operation, matched against its ordinary command body. |
| `option:SessionTarget.segment_id` | Its captured logical segment, or null when the act was accepted without one. |
| `option:SessionTarget.opening_id` | The immutable physical opening; null records an already absent target, rather than a request to close a later session. |
| `option:SessionTarget.opening_epoch` | The accepted opening order across a thread’s segments, independent from timestamps and process IDs. Native retirement keeps that order across an empty slot and restart. |
| `option:ToolSessionScope.opening_epoch` | The signed accepted opening order; an older segment cannot regain access after a provisional successor is removed. |
| `option:ToolSessionUpdate.opening_epoch` | Orders physical opening changes across logical segments; a delayed older update cannot close its successor. |
| `option:McpServerCreateBody.name` | The added server's name; exec and orch belong to builtin tool servers and cannot name an added definition. |
| `option:EnvironmentMcpServerCreateBody.name` | The environment-owned server's name, distinct from the builtin exec and orch services. |
| `option:SessionSpec.opening_id` | The accepted physical opening command passed to the adapter; it is separate from logical settings and from the private loopback key. |
| `option:SessionOpened.opening_id` | The physical opening that produced this report. A delayed earlier report cannot fill or reopen its replacement. |
| `option:SessionLost.expected` | A planned idle retirement closes the old physical opening quietly while retaining its conversation. A turn that was running still receives its actual loss and recovery outcome. |
| `option:SessionLost.opening_id` | The physical opening that actually ended, also retained across connector restart; only its matching live admission closes and recovers. |
| `option:TicketClaims.tool_session` | The immutable physical opening, segment, thread, member nonce and admitted revision of a session tool ticket; every Open and Resume also asks the API for current admission. |
| `option:ToolSessionScope.opening_id` | The accepted physical opening command; a reopen takes a new identity and transport refresh preserves the existing one. |
| `option:ToolSessionScope.member` | The native exec member (including the builtin MCP executor transport), the llm member, or mcp followed by the stable server UUID; a server name cannot stand for its identity. |
| `option:ToolSessionScope.nonce` | The unchanged member keeps its nonce; withdrawal and re-add issue a new one, leaving other members usable. |
| `option:ToolSessionScope.revision` | The durable ordering of session and membership facts; older replies cannot restore a closed or withdrawn channel. |
| `option:ToolSessionScope.thread_id` | The exact thread of this physical session, checked against both the signed ticket and the current segment. |
| `option:ToolSessionScope.segment_id` | The existing logical segment containing the physical opening; it alone does not identify a reopen. |
| `option:CleanupPlan.effects` | Current consequences from the shared usage-facts service: mount references switch off now; existing environment attachments end at their next start. Names follow current visibility; hidden consumers remain counts. |
| `option:CleanupStart.plan_id` | The exact current preview read by this person; replacing a mount location invalidates its old preview even at the same timestamp. |
| `option:CleanupStart.choices` | One displayed fate per selected item. Mount Delete can proceed with every item kept, including an empty mount. |
| `option:CleanupStart.confirmation_name` | The exact mount or plan name when selected owned data is destroyed; keeping data never asks for a typed name. |
| `option:CleanupStart.notify` | An explicit notification choice for the whole operation, narrowed by each recipient's preferences and grouped once per person. |
| `option:CleanupItem.record.blocker` | The actual reason a physical fate is unavailable, including not_permitted when data-management authority is absent; deleting the definition and keeping data remain possible. |
| `option:NotificationOperation.kind` | mount_delete identifies one accepted mount deletion and updates its existing grouped notice as cleanup ends. mount_enable and mount_disable identify a committed availability change, with one notice per affected person and no notice to the actor. |
| `option:ServerUpdateStart.release` | The exact currently offered semantic release selected by a system administrator; no path, command, URL or argument list is accepted. |
| `option:ServerUpdateView.installed_version` | The release of the API answering this read, including after an update or rollback. |
| `option:ServerUpdateView.offered_version` | The last validated release offer, or null before one is available. |
| `option:ServerUpdateView.update_available` | Whether that offer is newer than the API answering now; a cached earlier check never claims the new server is still behind. |
| `option:ServerUpdateView.availability` | The actual bundled-node, connection and local Docker condition for the Management update action. |
| `option:ServerUpdateView.command` | The same update.sh operator command remains available when the native bundled-node path cannot run. |
| `option:ServerUpdateView.job` | The most recent durable server update job, retained across API restart and page reload; another press joins an open job. |
| `option:ServerUpdateView.observation` | The latest safe phase and outcome recorded by the host updater, without native logs or credentials. |
| `option:ServerUpdateObservation.stage` | The phase recorded by the release updater; updated and rolled_back are confirmed outcomes. |
| `option:ServerUpdateObservation.reason` | A fixed safe failure reason, separate from the phase and from unfiltered native output. |
| `option:ServerUpdateObservation.from_version` | The release being left, once the existing updater journal records it. |
| `option:ServerUpdateObservation.to_version` | The exact release admitted for this job, unchanged by reconnection or another release check. |
| `option:NodeServerUpdate.timeout_secs` | The positive native preparation allowance captured from server.update_prepare_timeout_secs; it overrides the command kind’s default and does not time-limit the detached release transaction. |
| `option:NodeServerUpdate.command_id` | The exact successful native command behind this job’s prepared helper; observations without the same stored job, machine, release and private source are ignored. |
| `option:NodeServerUpdate.job_id` | The same durable update job on its authenticated bundled node; another node cannot supply its outcome. |
| `option:NodeServerUpdate.release` | The exact validated release for the fixed installation updater; native paths and host commands are never request fields. |
| `option:NodeServerUpdate.observation` | The bundled node reports only its own operation’s safe updater record through its durable node stream. |
| `option:MachineFacts.server_update` | A locally reported fixed updater capability, present only on the physical bundled installation with local Docker access. |
| `option:AuditAction.server.update_requested` | Records who requested the server release, its durable job and the exact bundled host and node. |
| `option:update.sh.ORCH_UPDATE_JOB` | The native helper attaches its durable job UUID for phase recording and recovery. Ordinary shell updates omit it; it contains no credential and chooses no host command. |
| `error:server_update_rolled_back` | The selected release failed readiness and the existing updater confirmed that the earlier release is running again. |
| `error:server_update_failed` | The existing updater could not finish; the retained observation says whether the original recovery command is needed. |
| `state:ServerUpdateAvailability.ready` | The connected bundled node offers the fixed local updater. |
| `state:ServerUpdateAvailability.no_bundled_node` | No active node is marked as bundled with the server; use the shown shell command. |
| `state:ServerUpdateAvailability.ambiguous_bundled_nodes` | More than one active node claims the server bundle, so no host is guessed. |
| `state:ServerUpdateAvailability.node_offline` | The bundled node must reconnect before it can receive an update job. |
| `state:ServerUpdateAvailability.docker_unavailable` | The bundled node lacks the installation’s local Docker updater path; the operator command remains. |
| `state:ServerUpdateStage.queued` | The admitted job has not started fetching. |
| `state:ServerUpdateStage.fetching` | The existing updater is fetching its manifest and assets. |
| `state:ServerUpdateStage.verifying` | The fetched release has reached its native verification step. |
| `state:ServerUpdateStage.applying` | The verified payload generations are being prepared. |
| `state:ServerUpdateStage.stopping` | The application services are stopping for the transaction. |
| `state:ServerUpdateStage.dumping` | The existing updater is taking the rollback database snapshot. |
| `state:ServerUpdateStage.activating` | The existing updater is selecting the new payload generations. |
| `state:ServerUpdateStage.starting` | The application services are restarting on the selected release. |
| `state:ServerUpdateStage.checking` | The existing updater is probing service and browser-route readiness. |
| `state:ServerUpdateStage.rolling_back` | The existing updater is restoring the previous payloads and database. |
| `state:ServerUpdateStage.recovering` | The same operation is recovering its existing journal after interruption. |
| `state:ServerUpdateStage.updated` | The selected release is running and passed readiness. |
| `state:ServerUpdateStage.rolled_back` | The previous release is confirmed running again after the attempted update. |
| `state:ServerUpdateStage.failed` | The updater did not confirm an updated or restored release; retain its recovery path. |
| `state:ServerUpdateFailure.health_check_failed` | The selected release did not pass service or browser readiness. |
| `state:ServerUpdateFailure.update_failed` | The updater ended before confirming its requested release or rollback. |
| `state:ServerUpdateFailure.recovery_required` | The original journal remains and the same update.sh recover path applies. |
| `option:PublicAddressUpdateBody.expected_revision` | The applied address revision used to prepare this draft. A concurrent publication or local reset refuses the stale draft without returning another system projection. |
| `option:PublicAddressUpdateBody.public_url` | The complete proposed primary origin, published only after certificate and installation checks succeed. |
| `option:PublicAddressUpdateBody.alternate_urls` | The complete proposed alternate origin set; omission is invalid, and an empty list clears it atomically with the primary. |
| `option:PublicAddressSnapshot.using_example` | The effective primary is still the shipped example address, rather than an installation-specific browser origin. |
| `option:PublicAddressUpdated.projection` | An intrinsic receipt of a prepared address publication. It carries no prior file seed or general system configuration. |
| `option:PublicAddressUpdated.revision` | The revision committed by this publication. |
| `option:PublicAddressUpdated.public_url` | The accepted canonical primary origin, derived from this caller’s draft. |
| `option:PublicAddressUpdated.alternate_urls` | The accepted alternate origins, normalized together with the primary. |
| `option:PublicAddressUpdated.using_example` | Whether the accepted primary is the shipped example. |
| `option:Meta.public_address_revision` | The committed revision read atomically with public_url and alternate_urls. A changed revision refreshes readers and a one-line browser notice, without reloading other browsers. |
| `option:orchctl.public-address.reset` | Restore the validated file primary and alternate seed through the local database, clear the stored override and advance its revision. Restoring an already admitted seed or current address set needs no HTTP hostname. A novel file origin uses the same certificate/origin preparation before commit. No credential is printed. |
| `option:orchctl.public-address.get` | Print only the committed effective primary address for local scripts. The file is used only if the system store has not been seeded; later file edits do not replace it. |
| `option:orchctl.doctor.only` | Includes public-address for the stored primary, alternates, revision and seed/override state. Without an explicit --url, selected browser checks use the committed address; default local checks probe the page and API. |
| `option:headers.X-Orch-Frontend-Generation` | The shipped frontend’s loaded worker certificate generation. Publication verifies this marker, the served TLS leaf and the installation proof before changing the effective addresses. |
| `option:PublicAddressProofQuery.challenge` | A fresh noncredential UUID challenge for one browser-origin check. |
| `option:PublicAddressProof.challenge` | The exact challenge given to this origin; a proof for another attempt is refused. |
| `option:PublicAddressProof.proof` | A purpose-bound keyed proof from this installation’s existing private authority. It signs nobody in and reveals no key or credential. |
| `option:PublicAddresses.public_url` | The primary HTTP(S) browser origin, validated with its alternates and read from one committed system-store revision for new links, commands and deliveries. |
| `option:PublicAddresses.alternate_urls` | Other explicit browser origins of this installation. Publication replaces the whole set atomically; a file edit does not replace an already seeded value. |
| `option:PublicAddressSnapshot.public_url` | The primary address in force at this revision. Already admitted logins, issued claims and exported packages keep their admitted address. |
| `option:PublicAddressSnapshot.alternate_urls` | The alternate origins committed together with the primary address. |
| `option:PublicAddressSnapshot.revision` | The monotone revision checked before publishing a prepared address candidate; a stale draft changes nothing. |
| `option:PublicAddressSnapshot.from_file_seed` | The stored initial seed is in force. The configuration file is read only to create the first snapshot or for an explicit local reset. |
| `option:MachineGatewaySetBody.selection` | Choose Automatic or a selected advertised gateway without reenrolling or replacing the machine identity. |
| `option:MachineGatewaySetBody.gateway_id` | Required only for Selected; Automatic omits or clears this value. Addresses outside the installation catalogue are refused. |
| `option:MachineGatewaySetReceipt.projection` | A receipt for the accepted native choice, without existing machine content. |
| `option:MachineGatewaySetReceipt.machine_id` | The same enrolled machine named by the request. |
| `option:MachineGatewaySetReceipt.selection` | The accepted primary selection; actual connection state remains in gateway status. |
| `option:MachineGatewaySetReceipt.gateway_id` | The selected advertised gateway identity, or null for Automatic. |
| `option:MachineGatewaySetReceipt.changed` | Whether this native write changed the saved choice; its reply is journaled before reconnecting. |
| `option:MachineGatewaySet.choice` | Native primary selection, validated against the machine’s deployment-bound advertisement. |
| `option:MachineGatewaySetResult.choice` | The exact native choice that was saved. |
| `option:MachineGatewaySetResult.changed` | Whether the saved configuration changed; this does not claim the new connection is already online. |
| `option:AuditAction.machine.gateway_changed` | Records who changed the primary gateway, its machine, saved choice and native command. |
| `option:Host.bundled_with_server` | Immutable origin recorded only by the protected server bootstrap token; never inferred from a host name or chosen home. |
| `option:HostRuntimeStopReceipt.projection` | The exact unit's Stop request was accepted without disclosing existing runtime content. This does not confirm physical absence. |
| `option:HostRuntimeStopReceipt.host_id` | The Host selected by this request. Runtime content requires the same operation's Read in addition to Manage. |
| `option:HostRuntimeStopReceipt.runtime_id` | The exact selected unit; a later runtime or native start cannot replace its acknowledgement. |
| `option:ProviderStopReceipt.projection` | The selected provider's Stop request completed without disclosing existing provider content. This does not assert physical absence. |
| `option:ProviderStopReceipt.provider_id` | The provider selected by this operation. Existing provider content additionally requires the same operation's Read. |
| `option:Cmd.authority_until` | Server-stamped absolute expiry of the original command's credential and operation. Native queue or preparation time never extends it. Explicit null means no finite time bound; it supplies no permission. |
| `option:PagePreview.server_host_use` | Current warning and exact acknowledgement for a Host Use, Manage or ownership grant on the host installed with the server; absent when it gains no new grant. |
| `option:ServerHostUseDisclosure.hosts` | How many protected server hosts gain or change a native grant through this page or its defaults; no host identities or names are disclosed. |
| `option:ServerHostUseDisclosure.confirmation` | Acknowledge the exact permission change and current native grants with the orch-server-host-use-confirmed header. Changed intent asks again; this proof grants no access. |
| `option:ProviderModelPauseQuery.note` | A short note beside the disabled model, visible in its affected threads. |
| `option:ProviderModelPauseQuery.run_waiting` | Enable runs eligible waiting messages only with true; omitted or false keeps them held for Resume. |
| `option:ProviderModelPauseQuery.notify` | Disable explicitly sends one renewable lifecycle notice per affected person for this exact model; omitted is silent. The dialog starts from Operation notices. |
| `option:Notification.data` | Model lifecycle notices use operation_kind provider_model_disable or provider_model_delete with running, done or failed; the same grouped item records progress and outcome. |
| `option:ResourceDeleteQuery.notify` | Explicitly send one grouped lifecycle notice per affected person when deleting a provider, one of its models or an MCP definition; omitted is silent. Provider child-job progress updates the same Inbox item. MCP removal records the definition change while durable machine cleanup continues. |
| `option:EnvironmentListRow.lifecycle` | The reported unit or attached-directory lifecycle used by Start and Stop in the environment list. |
| `option:HostListRow.bundled_with_server` | Identifies the protected host enrolled with this server in a compact list. |
| `option:ResourceImpact.notification` | The reviewed recipients, their count, and the initial Notify choice for this provider, model, mount, or MCP server. A person can inspect the audience before confirming. |
| `option:ResourceOperationNotification.preview_id` | Caller-bound review of this operation’s affected people. |
| `option:ResourceOperationNotification.people` | Total number of eligible people across the reviewed places. |
| `option:ResourceOperationNotification.audience` | Visible affected people and the count whose association remains private; the confirming person is excluded and each person is counted once. |
| `option:ResourceOperationNotification.default_on` | The inherited Operation notices default for this consequence; the confirmer may change the choice before acting. |
| `option:ExportLimits.max_file_bytes` | The current server allowance for one exported file, shown beside the archive allowance in the plan. A larger file is omitted and reported; this does not raise or replace the place's total archive allowance. |
| `option:ExportJobResult.omitted_items` | The completed archive's actual omission count. Unknown stays unknown when final metadata cannot be read. A ready archive with omissions says Partial; its download keeps the omission report, also offered beside it when the saved report is available. |
| `option:SystemComponent.issue` | A diagnosed operational issue beside the service's existing reason. It does not replace its own detail or imply that the API has stopped serving. |
| `state:SystemComponentIssue.metrics_listener_unavailable` | The API could not bind its optional metrics listener. Management names this beside the API's status; the API keeps serving, and certificate renewal and backup-record reconciliation continue unchanged. |
| `option:MetricSeries.resolution` | The chart names whether its returned readings come from minute or hourly stored data. This does not claim complete history, an oldest retained boundary, or one point per raw sample. |
| `option:ThreadQueue.eligibility` | The existing preflight’s checked selection, not the first loaded message. No read reserves work. |
| `option:QueueEligibility.input_ref` | The exact selected pending input, never a bare display identifier. |
| `option:QueueEligibility.safe_author` | Original author under the current safe person projection. |
| `option:QueueEligibility.safe_authority` | The actual person whose authority was checked, which can differ from the author. |
| `option:QueueEligibility.blocking_reason` | The typed thread-wide condition that still prevented this eligible input from starting. |
| `option:QueueEligibility.evaluated_at` | When this actual eligibility observation was captured. |
| `option:QueueEligibility.queue_revision` | The queue revision checked by this observation; dependency or queue changes make it unreported until rechecked. |
| `state:QueueEligibility.selected` | An actual pending input passed the checked per-person rules. |
| `state:QueueEligibility.none` | The checked queue had no eligible input; this is not an inference from an empty loaded page. |
| `state:QueueEligibility.unreported` | The observation is missing or invalidated. Opening Details does not rerun preflight. |
| `option:Turn.start_wait` | Captured policy and absolute deadline for this exact admitted native start; missing records stay unreported. |
| `option:ThreadStartWait.stage` | Native acceptance, native start confirmation, or actual event-ingestion grace. |
| `option:ThreadStartWait.began_at` | When the server entered this observed stage. |
| `option:ThreadStartWait.deadline` | This admission’s actual absolute deadline. A finite calendar overflow is not indefinite waiting. |
| `option:ThreadStartWait.after_millis` | Captured finite stage allowance where milliseconds can represent it. |
| `option:ThreadStartWait.admitted_default_source` | Setting scope recorded at admission, never replaced with a later effective default. |
| `option:ThreadStartWait.closed_at` | Confirmed native start or terminal time closes this wait; old receipts do not describe later work. |
| `state:ThreadStartWaitStage.acceptance` | Waiting for the native start command to be accepted. |
| `state:ThreadStartWaitStage.start` | Acceptance succeeded; the native start event is still awaited using the captured latest-acceptance bound. |
| `state:ThreadStartWaitStage.events` | The session’s native events are still being ingested, within the captured extra grace. |
| `option:Approval.delivery` | Native decision delivery is separate from the accepted decision. Old missing receipts are not assumed applied. |
| `option:ApprovalDelivery.recipient_kind` | The actual destination kind; no other person’s delivery route is disclosed. |
| `option:ApprovalDelivery.state` | Pending, acknowledged applied, or unknown after delivery could not be confirmed. |
| `option:ApprovalDelivery.last_attempt_at` | Last actual new delivery attempt, not a later read or replay of the same command. |
| `option:ApprovalDelivery.problem` | Typed delivery problem; it does not rewrite the person’s accepted answer or expose native private text. |
| `state:ApprovalDeliveryRecipient.native_harness` | The native harness receives this retained decision. |
| [Decision accepted · Delivery pending](state-reference.md#state:ApprovalDeliveryState.pending) | The decision is accepted and delivery remains due until a native acknowledgement. |
| [Applied](state-reference.md#state:ApprovalDeliveryState.applied) | The native delivery command acknowledged success. |
| [Decision accepted · Delivery unknown](state-reference.md#state:ApprovalDeliveryState.unknown) | Delivery was not confirmed; this is not a claim that the decision was lost. |
| `state:ApprovalDeliveryProblem.rejected` | The native request could not accept this decision. |
| `state:ApprovalDeliveryProblem.timed_out` | The actual delivery command deadline passed. |
| `state:ApprovalDeliveryProblem.cancelled` | The delivery command was cancelled. |
| `state:ApprovalDeliveryProblem.delivery_failed` | The attempt could not confirm native receipt. |
| `option:InputSchedule.timing_policy` | Typed rules for the current schedule preview, separate from old occurrence receipts. |
| `option:InputScheduleRun.outcome.timing_policy` | The actual rules captured when this occurrence was decided, independent of later timing edits. |
| `option:InputScheduleOccurrenceOutcome.timing_policy` | Captured rules for this occurrence; absent rules remain unreported. |
| `option:InputTimingPolicy.clock` | Civil, UTC cron, or a fixed chosen instant. |
| `option:InputTimingPolicy.repeated_time` | Recurring civil folds use the earlier instant; once schedules keep the explicitly chosen instant. |
| `option:InputTimingPolicy.missing_time` | Recurring civil gaps are skipped; invalid once times are refused. UTC has no civil gap. |
| `option:InputTimingPolicy.overdue` | A recurring backlog coalesces to the latest occurrence; a once schedule has one occurrence. |
| `option:InputTimingPolicy.paused_dates` | Paused dates are not replayed when a schedule resumes. |
| `state:InputTimingClock.fixed_instant` | The explicitly chosen absolute instant. |
| `state:InputTimingClock.civil` | Recurring local date/time in the authored zone. |
| `state:InputTimingClock.utc` | Cron is evaluated in UTC. |
| `state:InputTimingFold.explicit_instant` | The author chose the instant explicitly. |
| `state:InputTimingFold.earlier_instant` | A recurring repeated time occurs once at its earlier instant. |
| `state:InputTimingFold.not_applicable` | No civil repeated-time rule applies. |
| `state:InputTimingGap.refused` | Invalid authored once times cannot be saved. |
| `state:InputTimingGap.skipped` | A nonexistent recurring local time is recorded as skipped. |
| `state:InputTimingGap.not_applicable` | No civil missing-time rule applies. |
| `state:InputTimingOverdue.single_occurrence` | The schedule has one occurrence. |
| `state:InputTimingOverdue.latest_occurrence` | Earlier overdue occurrences coalesce into the latest due one. |
| `state:InputTimingPause.not_replayed` | Paused dates do not replay on resume. |
| `option:ThreadInput.delivery_resolution` | Original requested/applied delivery, captured only when this input actually used a fallback. |
| `option:AnnouncementPayload.delivery_resolution` | The same captured delivery resolution beside the retained announcement, independent of later edits. |
| `option:InputDeliveryResolution.requested` | The original requested delivery, never inferred from current capabilities. |
| `option:InputDeliveryResolution.applied` | The applied input mode; this does not certify native receipt. |
| `option:InputDeliveryResolution.reason` | Captured typed fallback cause. |
| `option:InputDeliveryResolution.source` | The explicit send choice or effective setting origin captured at acceptance. |
| `option:InputDeliveryResolution.resolved_at` | When the receiver accepted this delivery decision. |
| `option:InputFallbackOrigin.value` | Captured fallback value, retained even after policy changes. |
| `option:InputFallbackOrigin.scope` | Captured setting scope; unchecked ancestor identities remain private. |
| `state:InputFallbackOrigin.request` | The sender explicitly chose this fallback for the send. |
| `state:InputFallbackOrigin.setting` | The effective broadcast fallback setting supplied this choice. |
| `state:InputDeliveryResolutionReason.steer_unavailable` | No steering was available in this actual receiving turn. |
| `state:InputDeliveryResolutionReason.steer_unavailable_and_reach_limited` | Steering was unavailable and the sender could not interrupt this turn. |
| `option:MarkerSessionReopened.outcome` | Confirmed native result of this exact opening; an earlier intent or an old record does not claim a successful resume. |
| `option:MarkerAgentHistory.outcome` | Reconstruction confirmed by an accepted native import or actual handoff delivery, kept with this history record. |
| `option:Segment.session_outcome` | Last confirmed outcome for inspection; never evidence that the native conversation is still alive now, and never a substitute for an old marker’s own facts. |
| `option:NativeSessionOutcome.kind` | A verified resume and an applied recorded-history reconstruction are distinct outcomes. |
| `option:NativeSessionOutcome.reason` | Known recorded reopen cause; absent means it was not reported. |
| `option:NativeSessionOutcome.carried_method` | Actual reconstruction method; not applicable to a native resume that carries no replacement history. |
| `option:NativeSessionOutcome.coverage` | Recorded turn selection and a delivered transcript file have separate coverage, without claiming completeness of raw native events or original output bytes. |
| `option:NativeSessionOutcome.coverage.recorded_turns` | All or a suffix of the recorded turns was represented; a summary without captured selection says not reported. |
| `option:NativeSessionOutcome.coverage.transcript_file` | Coverage of the full recorded transcript actually made available to the new session; native-only import does not claim a file transfer. |
| `option:NativeSessionOutcome.at` | Server observation of the successful native acknowledgement or actual reconstruction delivery. |
| `state:NativeSessionOutcomeKind.verified_resume` | The native program acknowledged resuming the retained identity. |
| `state:NativeSessionOutcomeKind.reconstructed` | The recorded conversation was actually imported or its handoff delivered. |
| `state:SessionHistoryCoverage.complete` | All recorded turns of this selected range were represented, or the whole recorded transcript was supplied. |
| `state:SessionHistoryCoverage.partial` | Only the recorded newest suffix was represented. |
| `state:SessionHistoryCoverage.not_applicable` | This result did not carry that kind of material. |
| `state:SessionHistoryCoverage.not_reported` | This retained result did not capture that coverage; it is not silently treated as complete. |
| `option:Turn.offline_episode` | The current or last captured outage of this exact turn; absence means not reported, not indefinite waiting. |
| `option:ThreadOfflineEpisode.began_at` | The recorded start of this outage, independent of later setting edits or API restarts. |
| `option:ThreadOfflineEpisode.deadline` | Tagged finite or indefinite policy captured for this actual outage. |
| `option:ThreadOfflineEpisode.deadline.after_seconds` | Captured finite duration, with no added upper cap. A calendar overflow does not change it into indefinite waiting. |
| `option:ThreadOfflineEpisode.deadline.at` | Captured absolute deadline where representable; otherwise the finite duration remains explicit. |
| `option:ThreadOfflineEpisode.policy_source` | The captured setting-source scope; unchecked private ancestor identities are not disclosed. |
| `option:ThreadOfflineEpisode.episode_revision` | Changes only for a new outage of this turn, so a duplicate offline report does not retime it. |
| `option:ThreadOfflineEpisode.closed_at` | When this observation stopped being current; it is not proof that native work ended. |
| `state:OfflineEpisodeDeadline.finite` | A captured finite duration and its representable deadline. |
| `state:OfflineEpisodeDeadline.indefinite` | This outage was explicitly captured without a deadline. |
| `option:RetainedInput.id` | The original input identity; it is not a manufactured native Item. |
| `option:RetainedInput.continuation_id` | The input’s owning suffix under the selected read projection. Reading it does not select execution. |
| `option:RetainedInput.author` | Original sender through the current safe person reference; missing names are not replaced with identifiers. |
| `option:RetainedInput.submitted_at` | Original submission time for display, independent of its anchor-based position. |
| `option:RetainedInput.native_action` | The explicit native action retained with the original input; the same input visibility still applies. |
| `option:RetainedInputContinuation.after_item` | The anchor whose retained inputs continue, or the position before all items. |
| `option:RetainedInputContinuation.cursor` | Opaque ordinary timeline continuation for the same anchor and observed history cut. Opening it neither repeats Items nor runs work. |
| `option:InputScheduleRun.outcome` | Captured occurrence decision; omission means not reported. It is separate from the input’s later queued, held, started or failed state. |
| `option:InputScheduleRun.outcome.reason_code` | Typed acceptance, pending-input skip, nonexistent civil time, ended schedule or admission refusal. Never inferred from an English sentence. |
| `option:InputScheduleRun.outcome.coalesced_count` | Earlier occurrences actually combined into this accepted input; zero adds no coalescing note and a skip never claims an accepted input. |
| `option:InputScheduleRun.outcome.scheduled_for` | The recorded occurrence instant; null for a nonexistent local time. Later schedule edits do not replace it. |
| `option:InputScheduleRun.outcome.zone` | Time zone captured for this occurrence, independent of the schedule’s current definition. |
| `option:InputScheduleRun.outcome.accepted_input_ref` | Accepted input identity only while its destination is currently readable; otherwise withheld. |
| `option:ThreadInput.cancellation` | Retained cancellation cause, time and actor. Missing facts are not guessed from current permissions or defaults. |
| `option:InputCancellation.cause` | The actual recorded reason: access changed, a person removed it, Stop, edited away, superseded, thread ended, or the selected source became unavailable. A held Stop queue is not a cancellation. |
| `option:InputCancellation.at` | The recorded cancellation instant, not the time the reader opened the conversation. |
| `option:InputCancellation.by` | The recorded actor through the current safe person reference; absent for an automatic outcome or an unshared identity. |
| `option:ThreadSync.retained_inputs` | Cancelled authored inputs in this opening or sync, separate from Items and valid even with no native segment. Their existing input visibility still applies. |
| `option:ThreadSync.retained_inputs_next` | Intent-read continuations for anchor groups that do not fit the window’s shared byte allowance. |
| `option:ThreadItemListReply.retained_inputs` | Retained inputs owned by this page’s anchors, each returned once in an ordinary cursor walk. |
| `option:ThreadItemListReply.retained_inputs_next` | More input records for each returned anchor, without repeating the Item body. |
| `option:ThreadItemListQuery.retained_inputs_cursor` | Continue one retained-input group through the ordinary timeline reader; current thread/history authorization is checked again. |
| `option:RetainedInput.after_item` | Last preceding conversation Item, or no anchor before all Items. Clients use the anchor and recorded input order, never clock comparisons. |
| `option:RetainedInput.seq_created` | Immutable submission order among inputs sharing an anchor. |
| `option:RetainedInput.seq_updated` | Outcome revision used to merge live cancellation once. |
| `option:RetainedInput.outcome` | Captured cancellation facts; null means the reason and time were not reported. |
| `option:RetainedInput.parts` | Original authored parts under the input’s existing visibility, without manufacturing native delivery. |
| `option:ThreadData.retained_inputs` | Live retained input records from the same currently authorized thread projection. |
| `option:ThreadData.retained_inputs_next` | Retained-input continuations accompanying the same authorized live cut. |
| `state:InputScheduleOccurrenceReason.accepted` | An ordinary input was accepted; coalescing remains an independent recorded count. |
| `state:InputScheduleOccurrenceReason.previous_input_pending` | This occurrence was skipped because the previous input still waited. |
| `state:InputScheduleOccurrenceReason.nonexistent_local_time` | The captured local time did not exist in its time zone. |
| `state:InputScheduleOccurrenceReason.schedule_ended` | The accepted schedule allowance ended before this claim. |
| `state:InputScheduleOccurrenceReason.admission_failed` | Current admission refused this occurrence; its actual problem remains separate. |
| `state:InputCancellationCause.access_changed` | The original author can no longer send to this thread. |
| `state:InputCancellationCause.person` | A person explicitly removed the queued input. |
| `state:InputCancellationCause.stop` | A Stop actually cancelled this input; queue holding alone does not qualify. |
| `state:InputCancellationCause.edited_away` | An accepted edit removed this input from future work. |
| `state:InputCancellationCause.superseded` | Another accepted input replaced this one. |
| `state:InputCancellationCause.thread_ended` | The thread ended before taking this input. |
| `state:InputCancellationCause.source_unavailable` | The selected source was removed or disabled. The cancelled words remain in the conversation; this does not claim the thread itself ended. |
| `option:TurnLost.reason` | The connector’s explanation of the lost turn. |
| `option:TurnLost.turn_id` | Identifies the admitted turn the connector could no longer follow after restart. |
| `option:ThreadSideCreateBody.acting` | Choose an independent session that can act in the parent workspace. Omit for a verified read-only side. |
| `option:ThreadSides.acting` | The parent has a bound agent, provider and environment for an acting side session. The ordinary side-thread policy also applies. |
| `option:system.web_push_contact` | In the deployment’s `orch.toml`, the public HTTPS page or `mailto:` address sent to browser push services. The default uses the public HTTPS origin. A local HTTP installation offers browser registration without a sender contact. Some push services require a contact for delivery. |
| `option:NoticeDecision.requested_but_suppressed` | True for a withheld turn notice when the reader had manually followed at a matching level, allowed that notice kind and was away from the thread at the decision. |
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
| `option:UploadSessionCreateReply.policy` | Sizing and shared admission accepted for this upload; later setting edits do not rewrite it. |
| `option:UploadSessionCreateReply.chunk_bytes` | Accepted maximum piece bytes, from the captured policy. |
| `option:UploadSessionPutQuery.offset` | Any in-bounds offset. Identical acknowledged overlap is safe to retry; different bytes are refused. |
| `option:UploadSessionStatus.received` | Total distinct durable bytes, including an acknowledged suffix beyond a missing middle. |
| `option:UploadSessionStatus.ranges` | Sorted, coalesced durable intervals from GET session status; retry only their holes. |
| `option:UploadSessionStatus.policy` | This session’s accepted transfer policy. The status read changes no upload bytes. |
| `option:UploadRange.start` | First byte, counted from zero. |
| `option:UploadRange.len` | Number of bytes; the interval excludes start + len. |
| `option:TransferStatus.progress` | Distinct received bytes; complete means size and whole-content hash were verified. |
| `option:TransferStatus.ranges` | Machine progress JSON keeps durable ranges out of HTTP headers. |
| `option:TransferStatus.policy` | Accepted machine-upload policy, or current system policy before a transfer begins. |
| `option:TransferPolicy.initial_requests` | Small initial bandwidth probes, bounded by the shared request and byte allowances. |
| `option:TransferPolicy.initial_piece_bytes` | Bytes sent for the first bandwidth measurement, before the line rate is known. |
| `option:TransferPolicy.min_piece_bytes` | Smallest ordinary file piece; a final piece may be smaller. |
| `option:TransferPolicy.max_piece_bytes` | Maximum bytes in one file piece. Each unfinished upload keeps its accepted policy. |
| `option:TransferPolicy.target_window_ms` | Target data duration of the shared window, before adding measured round-trip headroom. |
| `option:TransferPolicy.window_rtts` | Round trips of data kept in flight ahead of acknowledgements, within the byte ceiling. |
| `option:TransferPolicy.max_window_bytes` | Maximum in-flight file bytes shared by simultaneous files from one sender. |
| `option:TransferPolicy.requests` | Simultaneous piece requests shared by files. Five leaves one HTTP/1.1 connection for other work. |
| `option:UploadSessionStatus.completed` | Completed receipt retained until session expiry; recovering a lost completion answer sends no file bytes and restores no released custody. |
| `option:FileTransferReply.transfer` | The announced native manifest transfer for the current environment, machine and generation. It is unrelated to browser upload sessions; reading starts no native work. |
| `option:FileTransferFacts.expires_at` | Last accepted piece plus the idle allowance captured at the first piece. No date is invented when the original receipt is absent; a later setting edit changes neither this expiry nor its source. |
| `option:FileTransferFacts.retention_source` | The captured registry-default or System setting source. Cleanup retains this receipt after removing expired pieces. |
| [Transferring](state-reference.md#state:FileTransferState.transferring) | The native publisher announced a transfer that has not completed; no successful publication is implied. |
| [Interrupted](state-reference.md#state:FileTransferState.interrupted) | The native publisher reported interruption of this exact transfer. Its recorded expiry still applies. |
| [Expired](state-reference.md#state:FileTransferState.expired) | Cleanup confirmed removal of the unfinished transfer after its recorded idle expiry. |
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
| `state:DirEntryKind.permission_denied` | The machine can list the name but its account cannot read the entry. |
| `state:DirEntryKind.vanished` | The name disappeared while the directory was being read. |
| `state:DirEntryKind.unreadable` | The machine encountered another filesystem error reading this entry. |
| `state:FileSourceKind.workspace` | The attached workspace uses Orch exclusions and protected control-file rules. |
| `state:FileSourceKind.mount` | An attached read-only root without workspace exclusions. |
| `state:FileSourceKind.owned_data` | An owned root copied whole without workspace exclusions. |
| `state:FileRuleKind.configured` | Configured Orch patterns contributed to this path’s inclusion. |
| `state:FileRuleKind.local` | An applicable local exclusion file contributed; its name is redacted when excluded. |
| `state:FileWatchSource.machine_configuration` | The fallback interval was supplied by the machine configuration accepted by the installed watcher. |
| `option:Item.text_replacements` | Replacement operations applied to source U+0000 characters in the item's received text and keys. The safe display uses U+FFFD; repeated full updates can contribute further operations. |
| `option:Item.storage_gap` | The last source event that could not be stored after text repair. Its safe refusal facts remain with the last good item contents while later events continue. |
| `option:ItemStorageGap.sqlstate` | PostgreSQL's data or program-limit refusal code, without the rejected value or database statement. |
| `option:ItemStorageGap.event_seq` | The rejected native event's sequence number, when a native event supplied it. |
| `option:ItemStorageGap.at` | When the refusal was recorded. |
| `option:OutputRef.inline_source_len` | Positive original byte count represented by a repaired inline preview; an empty preview omits this member. It keeps retained byte boundaries exact when a replacement glyph has more UTF-8 bytes than the source character. |
| `error:storage.data` | A source value still could not be stored after repair. Its item retains an explicit gap; this value is not retried and later native events continue. |
| `option:Turn.sandbox_resolution` | The connector’s captured requested and applied sandbox for this turn. Absent means not reported; later machine-policy edits do not rewrite it. |
| `option:ThreadBinding.sandbox_resolution` | The last captured sandbox decision for this current native session, separate from a later turn’s decision. |
| `option:SandboxResolution.requested` | The sandbox requested before the machine applied its ceiling. |
| `option:SandboxResolution.applied` | The sandbox actually passed to the native program, rather than the current machine setting. |
| `option:SandboxResolution.governing_policy` | The typed source that governed this selection. Local policy contents and unrelated settings are not exposed. |
| `option:SandboxResolution.command_id` | The exact native command whose applied sandbox was captured; it grants no permission to inspect that command. |
| `option:SandboxResolution.at` | When the connector confirmed the applied command, rather than the time of a later read. |
| `option:Machine.spool` | Last authenticated out-of-band output-storage observation. It remains reportable while authoritative spool writes are held. An offline machine shows this as last reported. |
| `option:MachineListRow.spool` | The same storage-backpressure fact in compact machine rows, without an extra detail read. |
| `option:SpoolStatus.problem` | Null when storage has no current failure; otherwise full, write, sync, damaged or event_rejected. Native text and filesystem paths are not included. |
| `option:SpoolStatus.waiting_events` | Events held by their publishers until whole-record storage and durability succeed; a failed append is retried without acknowledging or retiring the native event. |
| `state:SpoolProblem.full` | Storage or quota has no space. Authoritative output waits and automatically retries; it is not discarded to satisfy the diagnostic byte allowance. |
| `state:SpoolProblem.write` | A write or rollback failed. The original publication remains held, while the machine can still report its state and handle controls. |
| `state:SpoolProblem.sync` | Forcing accepted bytes to durable storage failed. Dirty revisions and publisher receipts remain pending for another force. |
| `state:SpoolProblem.damaged` | Startup found damaged retained output. The report distinguishes this from ordinary network disconnection. |
| `state:SpoolProblem.event_rejected` | The harness keeps an event that exceeds the link’s current limit for retry. Other streams can deliver output. A report on a stream the machine cannot publish is refused immediately. |
| `option:ToolCallPayload.masked_fields` | JSON pointers to native public fields changed by capture masking. A changed address is displayed as text and is never used as an executable link; original execution operands remain encrypted and private. |
| `option:InstructionSource.path_redacted` | The native source address was masked. Its row explains why it cannot open that address; the native source remains private. |
| `option:RawEvent.masking` | Why native diagnostic text is withheld: pending native end, unknown continuity, or incomplete matching context. Complete captured text is joined before it is masked. |
| `state:NativeTraceMasking.pending` | The adapter-declared native text stream has not ended; only an encrypted private copy is retained meanwhile. |
| `state:NativeTraceMasking.unclassified` | The native protocol did not classify text continuity, so enabled capture masking withholds that text. |
| `state:NativeTraceMasking.incomplete` | The matching prefix was not captured or is outside retention. Remaining text stays hidden; an adapter-declared new field has its own complete context. |
| `option:NativeTraceText.start` | The adapter declares a native field beginning with fresh complete matching context. After a capture gap, continuity is not inferred before a new start or a complete snapshot. |
| `option:RawFrame.text` | Adapter-declared semantic text streams, fragment JSON pointers and native end markers. Their operands are private; masking joins fragments before ordinary diagnostic storage. |
| `option:RawFrame.text_complete` | The adapter certifies that all other frame text is complete. Unclassified continuity is hidden while native masking is enabled. |
| `option:ResourceNotificationQuery.notify` | Notify the reviewed affected people once for this deletion. Absent or false leaves notification off; personal routes remain a separate choice. |
| `option:ResourceNotificationQuery.notification_review` | The notice applies only to the reviewed audience. If the review expires or that audience changes, the operation proceeds silently. |
| `option:ResourceStopQuery.note` | An optional one-line note for threads waiting on the disabled provider. |
| `option:ResourceStopQuery.notify` | Notify the reviewed affected people once for this provider stop. Absent or false leaves notification off. |
| `option:ResourceStopQuery.notification_review` | The notice applies only to the reviewed audience. If the review expires or that audience changes, the operation proceeds silently. |
| `option:ResourceImpactQuery.operation_notice` | Include the optional notice recipients for a stop or removal. |
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
| `option:AskModelOption.unavailable_reason.selected_backend_unavailable` | The saved backend is unavailable. Choose a currently allowed model to send the next question. |
| `option:AskModelOption.unavailable_reason.placement_unavailable` | The program needs a readable, usable environment and a helper topic in the conversation’s root organization. |
| `option:AskModelOption.unavailable_reason.invalid_backend` | Choose an offering and a declared Ask route in the model policy. |
| `option:ask.models.*.backend` | Exact dialogue route, account offering and adapter configuration. Child organizations narrow whole entries in parent order. |
| `option:ask.models.*.backend.route` | The declared route used for this offering. |
| `option:ask.models.*.backend.offering` | Account and enabled model selected together. |
| `option:ask.models.*.backend.offering.provider` | The selected account. Both the person asking and the current payer need Use. |
| `option:ask.models.*.backend.offering.offering` | Routing name of the enabled model on that account. |
| `option:ask.models.*.backend.config` | The selected route’s configuration, including placement references when required. |
| `option:AccessOptions.purposes` | Uses requested from a provider: Ask or other work. Requesting Ask alone does not authorize ordinary turns with the same model. |
| `option:ProviderUse.purposes` | Allowed uses: Ask, other work, or both. Omitted permits both; an empty list permits neither. A single entry must allow the use, model, effort and place together. |
| `option:ProviderUpdateBody.expected_revision` | The editable configuration revision. Facts, runtime status and source observations leave it current; another configuration edit requires reading the new revision. |
| `option:ProviderModelDefaultsQuery.model_name` | The exact routing name to preview with the provider home’s catalogue policy. This reads stored facts; it neither starts discovery nor adds a model. An explicit create parameters object saves the complete enabled definition and keeps cleared optional choices off. |
| `option:Meta.interface_defaults` | Installation defaults for retaining recently closed views, loading indications and read freshness. Each person may override them in their own interface preferences. |
| `option:InterfaceDefaults.kept_closed_tabs` | Additional recently closed views to retain. Open views are always retained. |
| `option:InterfaceDefaults.progress_delay_ms` | Milliseconds before a read shows its spinner. |
| `option:InterfaceDefaults.catch_up_above_items` | Effective installation threshold for the automatic catch-up card. A missing fact stays Not reported; it is not zero. The reader’s saved Interface choice overrides it. |
| `option:InterfaceDefaults.catch_up_above_items_source` | Captured system/default provenance for the same catch-up count. An absent source remains unreported. |
| `option:ThreadBinding.session_skills` | Small actual accepted-session state, absent when no decision was captured. It never contains skill material. |
| `option:SessionSkillsSummary.session_revision` | Immutable accepted opening identity fencing the detail pages. |
| `option:SessionSkillsSummary.segment_id` | The segment that owns the accepted session. |
| `option:SessionSkillsSummary.applied_at` | Actual accepted application time, not a later resource edit. |
| `option:SessionSkillsSummary.supported` | Whether the accepted program takes skills; false is distinct from unreported. |
| `option:SessionSkillsSummary.selected` | Count actually given to this session. |
| `option:SessionSkillsSummary.skipped` | Candidates omitted at that recorded selection decision. |
| `option:SessionSkillRef.id` | Currently readable skill identity. |
| `option:SessionSkillRef.name` | Current safely projected resource name, never a retained private fallback. |
| `option:SessionSkillVersion.id` | Exact accepted version identity, only when its skill is currently readable. |
| `option:SessionSkillVersion.number` | Number captured for the accepted version, never the current resource latest. |
| `option:SessionSkillSource.kind` | Recorded source category: place or personal. |
| `option:SessionSkillSource.setting` | Current safe place/source projection; an unreadable source remains Not shared. |
| `option:SessionSkillDecision.kind` | Recorded decision category: selected or skipped. |
| `option:SessionSkillDecision.version` | Accepted version reference, withheld when the skill cannot currently be read. |
| `option:SessionSkillDecision.reason` | Typed reason captured when the candidate was skipped. |
| `option:SessionSkillEntry.skill` | Current readable identity, or null without a private identifier/name fallback. |
| `option:SessionSkillEntry.contributor` | Safe current person reference for the actual giver checked at decision time. |
| `option:SessionSkillEntry.source` | Recorded personal/place choice under current source-name authority. |
| `option:SessionSkillEntry.decision` | Actual selected or skipped decision, independent of later edits. |
| `option:ThreadSessionSkills.projection` | The exact selected continuation read scope for this answer. |
| `option:ThreadSessionSkills.summary` | Small actual accepted-session state. |
| `option:ThreadSessionSkills.entries` | One authorized reference-only page; use its cursor for all later entries. |
| `option:ThreadSessionSkills.next_session_changed` | Current metadata-only proposal differs from this active session; null for unreported or inapplicable comparisons. |
| `option:ThreadSessionSkills.observed_at` | Actual observation time for this current proposal and projection. |
| `option:ThreadSessionSkillsQuery.continuation_id` | Explicit thread read selection; omitted follows active execution. |
| `option:ThreadSessionSkillsQuery.projection_revision` | Read projection fence for continued pages. |
| `option:ThreadSessionSkillsQuery.inspect_deleted` | Uses the existing current restoration authority for retained deleted content. |
| `option:ThreadSessionSkillsQuery.segment_id` | The exact segment owning the captured outcome. |
| `option:ThreadSessionSkillsQuery.session_revision` | Immutable accepted opening, not an automatic latest lookup. |
| `option:ThreadSessionSkillsQuery.cursor` | Continuation tied to the same immutable session and position. |
| `option:ThreadSessionSkillsQuery.limit` | Positive requested page count; ordinary list default when omitted, with no extra product ceiling. |
| `option:SessionSkillEntryPage.items` | Current safe decision entries on this immutable page. |
| `option:SessionSkillEntryPage.next_cursor` | Native continuation or null only after all decisions have been served. |
| `option:ThreadSkill.contributor` | Native contributor checked for Use; the public projection separately masks its safe reference. |
| `option:SkippedSkill.contributor` | Native contributor associated with the recorded omission. |
| `state:SessionSkillSource.place` | The choice came from a recorded place row. |
| `state:SessionSkillSource.personal` | The choice came from the starter’s personal skills. |
| `state:SessionSkillDecision.selected` | This version was actually given to the accepted session. |
| `state:SessionSkillDecision.skipped` | The selection decision omitted this candidate for its captured reason. |
| `state:SkillSkip.missing` | Not found. |
| `state:SkillSkip.disabled` | Off. |
| `state:SkillSkip.deleted` | Deleted. |
| `state:SkillSkip.no_version` | No version. |
| `state:SkillSkip.version_not_found` | Version unavailable. |
| `state:SkillSkip.not_usable` | Contributor lacks current Use at the decision. |
| `state:SkillSkip.name_taken` | Another selected skill uses this name; a winner is named only when readable. |
| `option:SystemInfo.effective_configuration` | Optional captured safe running values for the actual reporting process. Missing data is unreported, never assumed compiled defaults. |
| `option:EffectiveConfiguration.service` | The process whose values were applied; another service’s environment is never inferred. |
| `option:EffectiveConfiguration.process_revision` | Opaque identity of this startup observation, used as a reader fence and not displayed as a setting. |
| `option:EffectiveConfiguration.measured_at` | Actual configuration capture time, independent of a later file edit. |
| `option:EffectiveConfiguration.values` | Complete authorized page of allowlisted safe rows; the current API process reports only its applied database deadlines. |
| `option:EffectiveConfigurationValue.public_key` | Name from the explicit safe allowlist, rendered with a public label rather than raw loader data. |
| `option:EffectiveConfigurationValue.typed_value` | Finite deadline or an explicitly disabled deadline; absence is not an indefinite deadline. |
| `option:EffectiveConfigurationValue.origin` | Captured compiled, file or applied environment origin; unknown provenance remains unreported. |
| `option:EffectiveConfigurationValue.application` | Whether a later change needs process restart or is applied live by that consumer. |
| `option:EffectiveConfigurationValuePage.items` | Safe running rows on this authorized page. |
| `option:EffectiveConfigurationValuePage.next_cursor` | Native continuation, or null when every reported safe row is included. |
| `option:ConfigurationValue.kind` | A captured finite deadline or explicitly indefinite deadline; missing configuration is unreported. |
| `option:ConfigurationValue.milliseconds` | Exact unsigned decimal millisecond text, preserving the native range without browser-number rounding. |
| `state:ConfigurationValue.finite` | A finite captured deadline with its exact millisecond value. |
| `state:ConfigurationValue.indefinite` | This deadline was explicitly disabled at startup. |
| `state:ConfigurationOrigin.compiled` | The compiled default was used, with no explicit file/environment override. |
| `state:ConfigurationOrigin.file` | The file explicitly supplied this member, even if equal to its compiled default. |
| `state:ConfigurationOrigin.environment` | The loader actually applied an environment override. |
| `state:ConfigurationApplication.restart` | The consumer is constructed at startup; changes require restarting that process. |
| `state:ConfigurationApplication.live` | The actual consumer applies changes while running. |
| `state:ConfigurationKey.database.acquire_timeout_ms` | Safe public deadline key; only the reporting process’s actual applied value is served. |
| `state:ConfigurationKey.database.request_statement_timeout_ms` | Safe public deadline key; only the reporting process’s actual applied value is served. |
| `state:ConfigurationKey.database.background_statement_timeout_ms` | Safe public deadline key; only the reporting process’s actual applied value is served. |
| `state:ConfigurationKey.database.idle_transaction_timeout_ms` | Safe public deadline key; only the reporting process’s actual applied value is served. |
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
| `option:Started.setupObservation` | The same private server read that supplies the last successful setup, scoped to the current account reader. Details opens no additional read and reports saved choices only through the current native options; remembered arrangement is never new credential consent. |
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
| `option:MeLandingQuery.thread` | The thread remembered on this device. Without it the signed-in landing uses the conversation in the person's saved interface state. |
| `option:MeLandingReply.shell` | The person's shell from the usual authorized bootstrap read. |
| `option:MeLandingReply.thread_id` | The conversation selected for opening, or null when the saved place is another kind of page. |
| `option:MeLandingReply.thread` | The conversation's authorized first screen, its native refusal, or an omitted section when no conversation was selected. |
| `option:AuthSessionOpenQuery.read` | One native read of the page being opened, as its path and query below /api/v1, given once for each read. POST /auth/session/open restores the browser session and answers a line of JSON for the session, then one for each read as it is ready, with the read's own answer or its own refusal. A browser without a session cookie receives only `{"session":null}`. |
| `option:AuthThreadOpening.session` | A thread address restores the browser session with POST /auth/session/thread/{id}. A browser without a session cookie receives null. |
| `option:AuthThreadOpening.opening` | The signed-in reader's native thread opening, or its individual problem if that thread is unavailable. |
| `option:ThreadOpeningGetQuery.limit` | Positive item count for this opening, overriding the resolved threads.opening_items default. Older items remain available through history cursors. |
| `option:ThreadOpeningGetQuery.continuation_id` | Choose the history continuation for this read without changing which continuation executes. |
| `option:ThreadOpeningGetQuery.projection_revision` | Name the known history projection. A stale value returns the native reset with sync.complete=false and the current selector, which can be used to open a fresh window. |
| `option:ThreadModelParameter.options_partial` | True when only first-paint parameter traits and the current or visibly recorded models are supplied. Read full parameters when a control opens for its complete choices, help and forms. |
| `option:ThreadOpening.screen` | Independent activity and plan-answer facts for this actual thread. A failed part is retried through GET /threads/{id}/screen-facts without loading model metadata or the full settings list. |
| `option:ThreadOpening.parameters` | Compact current values and source/default facts for the first screen. Approval offers, reviewer details, subagent controls and port-tool facts come from the full GET /threads/{id}/parameters when their control opens. |
| `option:ThreadComposer.approval_default` | True when the native approval choice is supplied outside this thread. The closed approval chip can show its default mark without reading the full settings panel. |
| `option:ThreadComposer.sandbox_mode` | Effective native sandbox value for the compact control; null when no value is reported. Full choices, locks and writable revisions are read when the control opens. |
| `option:ThreadComposer.sandbox_default` | True when the sandbox choice is supplied outside this thread. Its compact control retains the default mark before its settings panel is read. |
| `option:ThreadScreenFacts.silent_after_secs` | Effective thread.silent_after_secs value in seconds. It controls the running thread's silence notice and uses the registered default unless a place sets it differently. |
| `option:ThreadScreenFacts.plan_answered_by` | Effective thread.plan_answered_by policy, writers or managers. Proposed plans use it before enabling an answer; it is read independently from model choices. |
| `option:Feed.provider_row` | The provider-row:{id} live feed carries the same authorized compact provider facts as a thread's opening. Provider View is checked again at delivery. It carries no models, source configuration or private login progress; the provider page keeps its full resource feed. |
| `option:ProviderRowEvent.provider` | Current compact thread-provider facts on a changed event. Null withdraws the previous facts when their native View authority is lost. |
| `option:AccessOptions.models` | The models. |
| `option:InvitationCreateBody.expires_in_minutes` | Invitation lifetime in whole minutes. The default and longest permitted lifetime come from invitations.default_minutes and invitations.max_minutes. The expiry is rounded down to its displayed minute. A policy default beyond the public clock ends at its last minute in year 9999; an explicit length outside that representation is refused. |
| `option:OrgTreeGetQuery.parent_id` | Read only the direct children of this node in the named root organization. Its cursor stays bound to that branch and reader; use a snapshot rather than a since sequence. |
| `option:OrgTreeGetQuery.limit` | Positive hierarchy page size. Branches default to navigation.tree_page_size; a larger explicit page is allowed. Full-tree reads keep the ordinary sync batch default. |
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
| `option:ModelParameters.purposes` | A provider manager's explicit model purposes replace discovery: generation, embedding, image generation, transcription, speech generation, reranking or structured decisions. Omission follows the reported purposes; an empty list offers this model for no work. |
| `option:ModelRecent.provider_id` | The provider of this person's chosen model; the same routing name at another provider is a distinct choice. History confers no access to the provider. |
| `option:ModelRecent.model_name` | The exact chosen routing name. Deliberate selection and admitted native use update private recency; opening a picker does not. |
| `option:ModelRecents.recent` | This person's model identities in server-accepted order, across devices and threads. Another person's history is never included; pickers match only their current suitable choices. |
| `option:ModelRecents.recent_count` | The current interface.recent_models default for the short group. Other choices keep their ordinary order and remain searchable. |
| `option:PersonalState.thread_setup` | The last successful native setup, saved atomically with birth and revalidated before reuse. Explicit place defaults and locks win; an environment is remembered only for that place. Failed creation does not replace it. |
| `state:PersonalStateKey.thread_setup` | The caller's portable successful setup preference; never another person's or authority to use its resources. |
| `state:UserEvent.model_recents` | This person's accepted model history changed. Their open pickers reread the private order without replacing unrelated preferences. |
| `option:ModelOffered.purposes` | Work reported by the model listing. A shared list does not prove every entry can chat. Purpose facts are separate from enabled speeds, costs and access. |
| `option:ProviderModel.purpose` | Read-only model and current endpoint purpose facts. They never rewrite editable parameters or make an unchanged model editor stale. |
| `option:ModelOption.purpose` | The same current work facts on a thread or Ask model choice, without provider configuration or credentials. |
| `option:ModelPurposeInfo.declared` | The manager’s choice takes precedence. Otherwise Orch uses the listing, a recognized family or routing name, then a dedicated endpoint. An unknown model stays selectable with “Purpose not reported”. |
| `option:ModelPurposeInfo.reachable` | Work reachable through the current declared endpoint kinds. A model-purpose override does not create a missing interface. Unknown interfaces do not invent a restriction. |
| `state:ModelPurpose.generation` | Conversational text, agent turns, Ask and summaries. |
| `state:ModelPurpose.embedding` | Text vectors for semantic retrieval. |
| `state:ModelPurpose.image_generation` | Image generation rather than conversational text. |
| `state:ModelPurpose.transcription` | Speech converted into text. |
| `state:ModelPurpose.speech_generation` | Generated speech rather than conversational text. |
| `state:ModelPurpose.rerank` | Reranking existing retrieval results. |
| `state:ModelPurpose.decision` | Choosing among supplied structured alternatives; this alone does not declare conversational generation. |
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
| `option:Problem.ceiling` | On a refused setting write, the readable inherited maximum and its source at the time of the decision (SettingInheritance). Operation-scoped keys receive only the field refusal. |
| `option:ResolvedSetting.inherited` | The effective value and its source with this place’s contribution omitted; supplied when the reader has the place context. The own contribution remains the stored choice. |
| `option:SettingInheritance.value` | The value the inherited choice gives; null means no scope supplies one. It is computed by the same native merge and default rules. |
| `option:SettingInheritance.source` | The place or declared default supplying the inherited value; concealed names retain their ordinary privacy boundary. |
| `option:ThreadInterruptBody.turn_id` | The turn last seen by the caller. Stop acts on the work currently running or preparing in the thread. If the named turn just ended, Stop can still hold its queue. |
| `option:ThreadStop.state` | The immediate Stop acknowledgement: stopping while work remains, or stopped when the target has ended. The accepted request continues after this response. |
| `option:ThreadStop.requested_at` | When Stop was requested; the elapsed stopping time begins here, including a wait for another change to finish. |
| `option:ThreadStop.turn` | The named turn as it stands in the acknowledgement. Absent when stopping preparation before a turn exists. |
| `option:Thread.stopping` | Independently versioned accepted Stop state, shared by the thread's controls and activity line even while an ordinary row write waits. Absent before any native Stop has been accepted. |
| `option:ThreadStop.stopping` | The same accepted state in the Stop answer. A read-only program that already returns a canonical completed turn needs no separate observation. |
| `option:ThreadStopping.version` | Commit-ordered version of accepted Stop state, separate from the thread history sequence. An older observation never replaces a newer one. |
| `option:ThreadStopping.requested_at` | Start of the shared stopping clock, retained while the canonical completion catches up. |
| `option:ThreadStopping.pending` | True while acceptance, cancellation or the interrupted turn still waits. A completed observation cannot revive an older activity line. |
| `option:ThreadStopping.thread_seq` | Canonical thread cut accompanying this observation. Controls keep the stopping state until that cut catches up. |
| `option:ThreadStopping.state_seq` | The ordinary Thread state row that completes this observation. A newer queued-input or turn response alone does not prove that the activity row has arrived. |
| `option:Turn.interrupted_at` | When somebody requested this turn's Stop. Absent when nobody stopped it; this is separate from its eventual completion time. |
| `option:MarkerSessionReopened.reason` | The recorded reason the session opened again before a turn, such as changed instructions or a program restart. Unchanged observable instructions keep the existing session. |
| `option:PortOpening.refused_until` | The stored time after which an identical refused effect may be asked again. The opening remains refused; this metadata does not reopen it or create another approval. |

| `option:ModelOptionReason.shared_for_ask_only` | Shared for Ask only; this model cannot be chosen for a thread. |
| `option:ModelOptionReason.shared_for_other_work_only` | Shared for other work only; this model cannot be chosen for personal Ask. |
| `option:AskModelOption.unavailable_reason.shared_for_ask_only` | Shared for Ask only. |
| `option:AskModelOption.unavailable_reason.shared_for_other_work_only` | Shared for other work only. |

| `option:OrgTreeGetQuery.path_to` | One ancestry path, without siblings. |
| `option:OrgTreeGetQuery.q` | Server-side name search; empty lists a bounded first page. |
| `option:OrgTreeGetQuery.kind` | The native node kind selected by a paged place picker. |
| `option:OrgTreeGetQuery.below_id` | Places inside this node, including itself. |
| `option:OrgTreeGetReply.ancestors` | Authorized structural ancestors outside the page window. |
| `option:OrgTreeGetReply.feed` | The current bounded metadata feed for exactly this page. |
| `option:TreeNode.folded` | Readable children and folded unread, mentions, decisions and current work inside the branch. |
| `option:TreeFold.archived` | Authorized archived paths have their own counts and attention. Show archived includes this group; hiding archived paths uses only the ordinary fields. Neither group contains a chat signal without Chat View. |
| `option:TreeNode.chat` | The node’s known chat identity does not grant Chat View. Its message position is null while that independent View is withheld. |
| `option:TreeNodeChat.seq` | The node’s known chat identity does not grant Chat View. Its message position is null while that independent View is withheld. |
| `option:DocumentTreeView.hierarchy` | The current reader’s bounded hierarchy pages across roots in one metadata subscription; every page uses its own native rooted admission. No whole tree or durable replay. |
| `option:DocumentTreeData.hierarchy` | A complete replacement of its one subscribed hierarchy page. |

| `option:BindingProvider.setup` | The current setup issue shown in a thread’s provider picker. Expired subscription tokens take priority over an empty model list. |
| `option:BindingProviderSetup.reason` | Sign-in expired, sign-in needed, or no model enabled, according to the provider’s last reported credential and configured models. |
| `option:BindingProviderSetup.can_sign_in` | On a lapsed subscription, a provider manager can open Credentials to sign in again. |
| `option:BindingProvider.reason.shared_for_ask_only` | Shared for Ask only; asking for other work remains available. |

| `option:OrgTreeGetReply.page_size` | The effective page default for this opening; small trees that fit keep their familiar initial expansion. |

| `option:DocumentTreeData.hierarchy.window` | The client-chosen routing number of the one page replaced; it conveys no authority. |
| `option:DocumentTreeData.hierarchy.problem` | An existing refusal code for this page; other open pages continue. The screen uses its ordinary localized reason. |


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
| `option:Problem.database_wait` | Safe facts for this request's known native database timeout; absent for other failures. |
| `option:DatabaseWait.phase` | Captured acquisition, statement or idle-transaction timeout phase; never inferred from a generic cancellation. |
| `option:DatabaseWait.applied_limit_ms` | Original applied deadline in milliseconds; null means the pool profile was not reported, never zero or an indefinite wait. |
| `option:DatabaseWait.service` | The answering service, currently api; contains no database address, SQL or configuration origin. |
| `option:DatabaseWait.occurrence_id` | The failed request's identifier; matches request_id and does not identify a database object. |
| [Database connection wait expired](state-reference.md#state:DatabaseWaitPhase.acquire) | The native connection-pool wait expired. |
| [Database statement timed out](state-reference.md#state:DatabaseWaitPhase.statement) | PostgreSQL explicitly cancelled a statement because its statement deadline expired. |
| [Database transaction expired](state-reference.md#state:DatabaseWaitPhase.idle_transaction) | PostgreSQL explicitly ended a transaction because its idle deadline expired. |
| `option:NodeSettingSetBody.model_choice` | Only with a thread’s agent.model write: reviewed effort and parameter cleanup values commit atomically with the model; each supplied own revision must still match. |
| `option:WithheldParameter.requirement` | The actual public failed condition and declared alternatives; no private configuration expression is exposed. |
| `option:WithheldParameter.inapplicable_selected_value` | The stored choice that is not applied; an explicit null is kept apart from no stored choice. |
| `option:ParameterRequirement.kind` | All, any, not, credential path, provider kind, placement, model feature or value, program version, agent support, feature enablement or provider rule. |
| `option:ParameterRequirement.requirements` | Every declared condition in this group is required. |
| `option:ParameterRequirement.alternatives` | Any declared alternative can satisfy this requirement; none is inferred from a broad refusal category. |
| `option:ParameterRequirement.requirement` | The typed condition whose negation is required. |
| `option:ParameterRequirement.path` | Native, forwarded, shared or handed-over model credentials, as the program declares. |
| `option:ParameterRequirement.provider_kind` | The API key or subscription kind that the declaration requires. |
| `option:ParameterRequirement.placement` | The managed or connected environment kind that the declaration requires. |
| `option:ParameterRequirement.feature` | The declared model capability required for this parameter. |
| `option:ParameterRequirement.list` | The effort, speed or access-program list required by the declaration. |
| `option:ParameterRequirement.value` | The declared model-list choice that is required. |
| `option:ParameterRequirement.range` | The public agent-version range required by the declaration. |
| `option:ThreadParameters.approval_choices` | Native approval choices with their current conformance and admission reasons; the ordinary writer still rechecks every gate. |
| `option:ApprovalModeChoice.mode` | The native mode, including a dim Auto choice explaining why its conformance is unavailable. |
| `option:ApprovalModeChoice.available` | Whether policy, reviewer access and actual isolation currently admit this choice. |
| `option:ApprovalModeChoice.reason` | The actual known reviewer, policy, isolation or build refusal category. |
| `option:ApprovalModeChoice.requirement` | The missing native reviewer, model Use, policy allowance, isolation or build support. |
| `option:ToolError.rate_limit` | The deciding tool limiter’s captured allowance, source and reported wait; ordinary errors omit it. |
| `option:ToolPayload.rate_limit` | Typed refusal facts carried through the native tool result; absence is not zero or a current retry promise. |
| `option:ToolRateLimit.allowed_calls` | The per-thread call allowance in force at this refusal. |
| `option:ToolRateLimit.period_secs` | The unit period of that recorded allowance. |
| `option:ToolRateLimit.retry_at` | The reported historical retry instant, absent only when it cannot be represented; no automatic retry is added. |
| `option:ToolRateLimit.observed_at` | The captured refusal time; later settings and limiter restarts do not rewrite it. |
| `option:ToolRateLimit.source` | The supplying setting identity; its current name remains under the current reader’s authority. |
| `option:ProviderRefreshReply.trial` | The actual recovery check receipt; it never infers a reservation from credential-error status. |
| `option:ProviderTrial.state` | Reserved, native Running, Used or Not reserved; no captured record is explicitly unreported. |
| `option:ProviderTrial.at` | The recorded state time; a reservation is not a successful sign-in or a promised start. |
| `option:ProviderTrial.input_ref` | The exact reserved input, only within current live thread and input visibility. |
| `option:ProviderTrial.thread_ref` | The currently authorized thread reference; a provider-only key does not acquire it. |
| `option:ProviderTrial.reserved_for` | The captured responsible person, projected only when the original input is currently readable. |
| [Reserved](state-reference.md#state:ProviderTrialState.reserved) | One specific input has the single trial reservation. |
| [Running](state-reference.md#state:ProviderTrialState.running) | The reserved native turn is actually running. |
| [Used](state-reference.md#state:ProviderTrialState.consumed) | The admitted trial ended or verified recovery consumed its reservation; this does not assert vendor success. |
| [Not reserved](state-reference.md#state:ProviderTrialState.not_reserved) | This observed check holds no active trial reservation. |
| `option:AnnouncementPayload.delivered_as` | The actual queued, steered, interrupted or deferred recipient outcome, separate from requested delivery. |
| `option:ThreadBinding.call_route` | The active admitted failover and current return context; absent when no route is reported, including retained inspection. |
| `option:ThreadCallRoute.configured` | The person’s configured route, separate from a temporary override; unknown older origins stay unreported. |
| `option:ThreadCallRoute.active_override` | The actual admitted override, retired when the person’s binding or choice changes. |
| `option:ThreadCallRoute.original` | The captured original route, without its private parameters. |
| `option:ThreadCallRoute.return_policy` | The current next-turn-when-ready or stay policy, rechecked on the next call. |
| `option:ThreadCallRoute.return_state` | The actual returning state or current policy; never readiness inferred from a pending candidate. |
| `option:ThreadCallRoute.observed_at` | When the current route facts were observed. |
| `option:CallRouteChoice.provider_id` | The admitted route’s provider identity; this carries no provider configuration or credentials. |
| `option:CallRouteChoice.model` | The admitted model’s routing name; normal screens use its safe resource name. |
| `option:CallRouteChoice.display_name` | The admitted display name captured with the route, or unreported on older receipts. |
| [When ready on a later turn](state-reference.md#state:CallReturnState.when_ready) | Return when ready on a later eligible turn; no start is promised. |
| [Stay on failover](state-reference.md#state:CallReturnState.stay_on_failover) | Keep the admitted failover route until an ordinary change. |
| [Returning](state-reference.md#state:CallReturnState.returning) | The engine has actually begun returning to the original route. |
| [Not reported](state-reference.md#state:CallReturnState.unreported) | The original route or return fact was not captured. |
| `option:ModelChoiceSettings.effort` | A changed writable effort committed with the model; omission keeps an unchanged or independently locked effort. |
| `option:ModelChoiceSettings.params` | The reviewed own parameter map after incompatible choices are removed, committed in the same transaction. |
| `option:ModelChoiceSetting.value` | The explicit related setting value; null follows the model’s effort default. |
| `option:ModelChoiceSetting.expected_revision` | The displayed own revision; zero requires an absent own row. A stale component refuses the whole model choice. |
| `option:ProviderModelUpdateBody.expected` | The definition read when opening a draft; editors submit it and a changed baseline refuses the whole save. Omission is a deliberate unconditional replacement, which requires no prior Read. |
| `option:ProviderModelState.parameters` | The enabled model options on which the draft was based, compared under the model row lock. |
| `option:ProviderModelState.groups` | The ordered model groups on which the draft was based. |
| `option:BindingDefaults.locks` | Each creation default has its own lock; one fixed choice leaves unrelated choices available. |
| `option:BindingDefaultLocks.provider` | Whether the default provider is fixed; models within it remain separately selectable. |
| `option:BindingDefaultLocks.agent` | Whether the default agent profile is fixed. |
| `option:BindingDefaultLocks.model` | Whether the model routing name is fixed, independently of effort. |
| `option:BindingDefaultLocks.effort` | Whether effort is fixed; compatible model choices remain available. |

| `option:BindingAgent.approval_resolution` | The approval preference and the actual declared replacement in a start proposal; absent when the program has not reported its modes. |
| `option:ApprovalDefaultResolution.preferred` | The effective inherited approval preference. |
| `option:ApprovalDefaultResolution.effective` | The program mode selected by the same rule used at creation; null when no ordinary replacement is declared. |
| `option:ApprovalDefaultResolution.reason` | Why the inherited preference needs replacement, when it does. |
| `option:ApprovalDefaultResolution.source` | The protected setting source of the preference. |
| `option:ApprovalDefaultResolution.revision` | Identifies this observed preference, source and declared modes; creation still checks current authority and inputs. |
| `state:ApprovalDefaultReason.not_declared` | The program does not declare the preferred mode. |
| `state:ApprovalDefaultReason.no_ordinary_mode` | The program declares no ordinary replacement. |

| `option:ParameterRequirement.actual` | The safe observed credential, provider or placement category, or reported program version, at the same eligibility check; absence is unreported. |
| `option:ThreadParameters.orch_tools` | A small summary of the current native session’s accepted tool offer; current settings do not rewrite it. |
| `option:OrchToolsSummary.session_revision` | Identifies the exact accepted catalogue and captured inputs. |
| `option:OrchToolsSummary.offered_count` | The number actually offered, including measured zero. |
| `option:OrchToolsSummary.accepted_at` | When the offer was accepted. |
| `option:OrchToolFact.key` | The declared native tool key. |
| `option:OrchToolFact.offered` | Whether the accepted session was given this tool. |
| `option:OrchToolFact.reason` | The captured build, setting, continuation, goal or capability exclusion. |
| `option:OrchToolFact.source` | The supplying setting source, only under current settings-read authority. |
| `option:OrchToolsQuery.cursor` | Native continuation within the same accepted session offer. |
| `option:OrchToolsQuery.search` | Filters declared tool names within this retained catalogue. |
| `option:OrchToolsQuery.session_revision` | Requires the displayed accepted offer; a changed session must be read again. |
| `option:OrchToolsPage.summary` | The actual session summary, or null when no offer was captured. |
| `option:OrchToolsPage.tools` | One native page of offered and excluded declarations; follow its continuation for more. |
| `option:NativeImageReceipt.native_image_bytes` | The effective native image byte maximum at admission; zero selects materialized paths. |
| `option:NativeImageReceipt.source` | The captured allowance source; its name remains protected by current authority. |
| `option:NativeImageReceipt.resolved_at` | The actual resolver observation time. |
| `state:OrchToolUnavailable.not_built` | This engine does not implement the declaration. |
| `state:OrchToolUnavailable.off_here` | The tool is off at this place. |
| `state:OrchToolUnavailable.read_only` | The continuation permits reading tools only. |
| `state:OrchToolUnavailable.no_tools` | The continuation receives no tools. |
| `state:OrchToolUnavailable.goal_required` | No active goal supplies the goal tool. |
| `state:OrchToolUnavailable.capability_unavailable` | The native capability gate did not offer this tool. |
| `state:OrchToolUnavailable.program_unsupported` | The program does not accept Orch tools. |
| `state:OrchToolUnavailable.environment_tools_off` | The environment turns Orch tools off. |
| `state:OrchToolUnavailable.capability_work` | This capability-work continuation receives no Orch tools. |
| `option:SubFrame.window` | Incremental page change on an existing hierarchy metadata subscription; it keeps other pages and uses no extra subscription slot. |
| `option:HierarchyWindow.kind` | query registers consecutive selector fragments; remove releases that page. |
| `option:HierarchyWindow.window` | Connection-local page routing number, retained only while that page is held. |
| `option:HierarchyWindow.data` | Consecutive URL-safe unpadded base64 fragments of UTF-8 selector JSON, each inside the announced physical frame boundary. |
| `option:HierarchyWindow.complete` | The final fragment validates and follows the bounded selector; invalid selectors affect only that page. |
| `option:ReadyFrame.max_client_frame_bytes` | Native maximum bytes of one client JSON frame, including its envelope; metadata selectors are split to fit it. |
| `option:HierarchyWindow.root` | Actual root of this page selector; it does not grant View. Every update uses the same rooted native authorization as HTTP. |
| `option:TreeFold.has_children` | A native View-authorized direct child exists in this visibility class. Expand stays available without counting or retaining a quiet subtree. |
| `option:TreeFold.small_tree` | The root’s permission-resolved descendants fit this initial page. This optional opening fact preserves the initial small-tree shape; live refreshes omit it and larger trees remain fully browsable one level at a time. |
| `option:OrgTreeGetQuery.include_threads` | False excludes threads before a place page is filled; omission includes them. It is not combined with path_to, and its effective value is bound into the cursor. |
| `option:OrgTreeGetQuery.sort` | Order the whole eligible set before paging. Navigation uses the caller’s saved choice, then the org default; explicit newest, changed, name and custom are also available. Omission uses native identity order. |
| `option:OrgTreeGetQuery.locale` | en or zh-CN chooses navigation’s numeric, base-letter name ordering. The API process locale does not change it. |
| `option:OrgTreeGetReply.ordering` | The effective order and safe partial sibling ordering for already returned identities, without exposing raw saved preference maps. |
| `option:HierarchySort` | navigation resolves saved preferences; newest, changed, name and custom request one mode directly. |
| `option:HierarchyOrderBy` | The resolved newest, changed, name or custom mode used for this window. |
| `option:HierarchyLocale` | en and zh-CN follow their numeric, case/accent-insensitive name order. |
| `option:HierarchyOrdering.by` | The effective mode, including an org default a reader follows without reading the whole settings page. |
| `option:HierarchyOrdering.revision` | Opaque order-and-locale revision for retiring stale cursors and partial sibling constraints. |
| `option:HierarchyOrdering.known_order` | Only relative orders of visible or inspectable identities already in the response. Private ranks and other saved identities are absent. |
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
| `option:BindingEnvironment.room` | Measured CPU and available RAM from the connected machine or current host, with its sample time. An absent reading means the source has not reported it. |
| `option:PlaceEnvironment.room` | Room measured on this admitted placement source at the recorded time. |
| `option:PlaceHost.room` | The selected host’s latest measured free CPU and memory. |
| `option:PlaceHost.running_threads` | Primary threads currently running in this host’s current environment units. |
| `option:MachineFacts.room` | Aggregate available memory and measured unused logical CPU time on this machine. |
| `option:PlaceRoom.cpu_free_millis` | Unused processor capacity in thousandths of a core. An absent measurement is unknown. |
| `option:PlaceRoom.memory_free_bytes` | Available RAM in bytes. Zero is a measured value. |
| `option:PlaceRoom.observed_at` | Time of the resource sample. |
| `option:Job.handoff_result` | The completed change's same retained handoff receipt under current job read authority; progress-only reads withhold it. Missing capture stays Not reported. |
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
| [Next update](state-reference.md#state:AutomaticScheduleState.queued) | Actual accepted automatic work is queued or claimed; eligibility is not a promised start. |
| [Waiting to retry](state-reference.md#state:AutomaticScheduleState.retrying) | An actual failed attempt has a recorded retry eligibility. |
| [Skipped · No summarizer](state-reference.md#state:AutomaticScheduleState.skipped) | The last attempt was skipped or paused with no current queued work. |
| `state:AutomaticScheduleState.idle` | No automatic work is currently queued. |

### Native approval checks

| Reference | Meaning |
| --- | --- |
| `option:AgentDescriptor.approval_auto` | Current server-owned proof for Auto at the selected program build; profile overrides cannot create it. |
| `option:ThreadComposer.auto_unavailable` | Whether the stored Auto approval can run, shown on the closed control. True means it is unavailable. |
| `option:AutoApprovalCheck.machine_id` | Selected machine for the native check. |
| `option:AutoApprovalCheck.sha256` | Digest of the exact selected program build. |
| `option:ApprovalCheckTarget.machine_id` | Machine selected for the retry. |
| `option:ApprovalCheckTarget.sha256` | Only this checksum may be checked for the selected machine. |
| `option:HarnessCheckQuery.machine_id` | Select the machine together with the build checksum for a retry. |
| `option:HarnessCheckQuery.sha256` | Digest supplied together with the machine for a single-build retry. |
| `option:MachineFactsHarness.sha256` | Digest of a confirmed executable read by its connector. |
| `option:RunningHarnessBuild.check_machine_id` | The machine that can check the selected running build. |
| `option:AutoApprovalCheck.declared` | Whether the original program declaration offered Auto, retained so a passing recheck can restore it. |
| `option:AutoApprovalCheck.state` | Not proven, failed, passed or no check available for this program. |
| `option:AutoApprovalCheck.check_available` | Whether this kept program build has an Auto conformance case that can be checked. |
| `option:ResolvedSetting.approval_choices` | Approval mode choices with their actual current proof, policy, reviewer and isolation reasons, including dim choices. |
| `option:ApprovalModeChoice.check` | The existing program check to offer beside unavailable Auto; only current system administrators may start it. |
| `option:ApprovalCheckTarget.kind` | The program whose kept build is checked. |
| `option:ApprovalCheckTarget.version` | The exact program version checked through the ordinary build check action. |
| `option:Problem.session_end` | Own-session ending facts, only after an exact retained browser credential match. Unknown credentials, purged records and automation keys receive no cause. |
| `option:SessionEnd.cause` | The native expired, revoked or account_unavailable outcome. Access-token expiry alone does not end a browser session. |
| `option:SessionEnd.at_if_known` | The stored expiry or revocation instant; omitted when no ending instant was retained. Observation time is never substituted. |
| `state:SessionEndCause.expired` | The proven browser session passed its recorded idle deadline. |
| `state:SessionEndCause.revoked` | The proven browser session was explicitly revoked at its recorded time. |
| `state:SessionEndCause.account_unavailable` | The proven browser session belongs to an unavailable account; no unrecorded suspension time is implied. |
| `option:MachinePorts.loopback_tcp` | Exact nonzero TCP port numbers the machine permits for personal loopback forwarding. The list starts empty. |
| `option:PortTicketClaims.machine_reach` | The signed personal TCP use carries an explicit Machine reach. The connector checks the selected port against its current policy. |
| `option:PortTarget.named_loopback` | The current reader has Machine reach for a loopback port declared by the operator. The connector checks that it is listening when a connection opens. |
| `option:FileAccess.reach` | On Ports View, Roots covers environment listeners. An explicit Machine entry also permits personal TCP forwarding to loopback ports named by the operator. Roots is the default for Files View and Manage entries. Machine must be named explicitly and reaches only paths the environment’s own OS user can open; it grants no missing permission and cannot exceed the giver or machine ceiling. |
| `state:FileReach.roots` | The environment workspace and its attached mounts, checked against canonical native targets. |
| `state:FileReach.machine` | Every native path the environment’s own user may open, subject to the machine’s current ceiling and ordinary operation restrictions. |
| `option:LocalPolicy.environment.files_reach` | The machine-owned ceiling for human file reach: roots or machine. Machine is the ceiling default, not a Files grant; individual file entries still default to roots. Agent confinement is separate. |
| `option:ConnectorConfig.environment.files_reach` | The connector’s environment.files_reach policy value. The machine enforces it locally and reports it with its current facts. |
| `option:FsAccess.view` | The native read reach admitted for this command; null admits no read. |
| `option:FsAccess.manage` | The native mutation reach admitted for this command; a broader read never widens it. |
| `error:files.reach_denied` | The target is outside the current holder or machine file reach; no file is opened through that refused act. |
| `option:Environment.file_reach` | Current caller file reach on the full environment page; absent from a limited through-thread read. |
| `option:PermissionPage.file_reach` | The current caller’s file reach and native bound beside an environment’s permission choices. |
| `option:FileReachFacts.view` | Current read reach after the machine bound; null grants no read. |
| `option:FileReachFacts.manage` | Current mutation reach, kept separate from read reach; null grants no mutation. |
| `option:FileReachFacts.view_from_terminal` | Usable Terminal widened an existing Files View grant for this same credential. |
| `option:FileReachFacts.manage_from_terminal` | Usable Terminal widened an existing Files Manage grant without inventing a missing act. |
| `option:FileReachFacts.machine_bound` | The current native machine policy ceiling, independent of a named entry or agent sandbox. |
| `state:FileSourceKind.machine` | An intentional native path outside attached roots, opened as the environment’s own user without workspace indexing. |
| `option:FsFetchResult.minimum_reach` | The minimum reach proved by the native resolver for these exact bytes, retained with cached content so a relative symlink cannot turn machine data into roots data. |
| `option:EnvironmentFileListQuery.prefix` | Optional case-insensitive child-name prefix for one-level path completion, filtered before pagination. |
| `option:FsList.prefix` | The same native directory-name prefix; completion reads one level and uses the existing bounded observation cursor. |
| `option:ReferencePreviewQuery.path` | A relative attached-root path or explicit native absolute address. The current reader’s file reach and original native source are checked for live and retained file data. |

<a id="resource-pool-targets"></a>

## Resource pool targets

| Option | Meaning |
| --- | --- |
| `pool` | A configured resource group with its own View, Use and Manage. Pool Use alone grants no member access. |
| `pool_own` | The pool's own inherited routing-default source. |
| `model`, `provider`, `environment`, `host` | Pool member family; a choice accepts only its own family. |
| `single`, `pool` | One exact target or a configured pool; resolved members remain separate from selected intent. |
| `ordered`, `weighted`, `round_robin`, `least_cost`, `least_loaded` | Explicit policy: saved order, proportional weights, rotation, comparable cost or reported load. |
| `cpu`, `memory`, `combined`, `threads`, `processes` | Reported load measure; threads and processes are counts without an invented percentage denominator. |

| Reference | Meaning |
| --- | --- |
| `option:PoolDefinition.kind` | Model, provider, environment or host; immutable after creation. |
| `option:PoolDefinition.enabled` | Enables selection and lending through this pool; a disabled pool remains inspectable and its saved Where set remains meaningful. |
| `option:PoolDefinition.policy` | An explicit selection policy is required before enabling. Preview never reserves a member. |
| `option:PoolDefinition.lend_members` | Allows this pool to receive a member's current Through pools consent. It grants no direct member Use or Read. |
| `option:PoolMember.priority` | Optional priority tier; absence follows the inherited default. |
| `option:PoolMember.weight` | Optional positive decimal weight; absence follows the inherited default. |
| `option:PoolMemberRule.grouping` | The exact live display group, including missing home, owner, machine or tag metadata. It is not a snapshot of the loaded page. |
| `option:PoolLendingPutBody.matcher` | A named pool, pools homed at a place, or pools managed by a named person. Each native use rechecks the match and giver. |
| `option:PoolLendingPutBody.options` | One complete allowance, including provider purpose/model/service/effort/arrangement/Where, host or template limits. Different consents cannot supply fragments of one request. |
| `option:PoolLendingPutBody.show_names` | Permits member identity in the admitted pool context; it gives no definition Read or general member access. |
| `option:PoolLendingPutBody.expires_at` | Explicit expiry; outside sharing also obeys the current inherited lifetime bound. Reads never renew it. |
| `option:PoolUse.provider` | Provider limits stay together through every nested pool. |
| `option:PoolUse.host` | Host creation, storage, template and resource limits. |
| `option:PoolUse.template` | Template capability and resource limits. |
| `option:WhereName.environment_pool` | A live structural set of existing environments. Membership grants no environment Use. |
| `option:WhereName.host_pool` | Every environment on a current host member; selection policy, cooling and a disabled pool do not change the saved location set. |
| `state:PoolMember.member_hidden` | The current reader cannot inspect this member's identity; the authored row stays present without its target. |
| `option:PoolMemberMetadataBody.enabled` | Enables or disables only the saved member row; the native writer preserves its hidden target. |
| `option:PoolMemberMetadataBody.priority` | Explicit tier or inherited default, without replacing the rule. |
| `option:PoolMemberMetadataBody.weight` | Positive decimal weight or inherited default, without replacing the rule. |
| `option:PoolMemberMetadataBody.position` | Saved written-rule order, guarded by the observed pool revision. |
| `option:PoolMemberPutBody.position` | Omit to append a new rule after every saved row, or preserve an existing rule's place. An explicit position changes only that row. |
| `option:PoolDeleteBody.expected_revision` | Removes the pool the manager reviewed. Shared member resources and historical receipts remain; live pool or Where references must first be replaced. |
| `state:PoolMember.member_unavailable` | The current saved target or offering is missing. A continued preview checks this again before showing its row. |
| `state:PoolMember.request_required` | Inspected facts are available; actual eligibility still needs the accepted request's purpose, placement, limits and native capability checks. Inspection spends nothing. |
| `option:Pool.target` | The canonical typed selection intent for this pool; it grants no Use and is distinct from the eventual admitted member. |
| `option:PoolPolicy.least_cost` | Compares known request-credit estimates within the first feasible priority tier. Unknown prices follow known prices and use the saved weights. Nested pools propose under their own policy first; unchosen rotation never advances. |
| `option:PoolPolicy.least_loaded` | Compares the selected actual observation with its native denominator; missing data is unavailable. Combined is the greater of normalized CPU and memory, while threads/processes remain counts. Native capacity is still rechecked at admission. |
| `option:NodeLoad.pids` | Actual process count in the reporting host scope, supplied with native load observations; missing is unknown. This is a count without a percentage denominator. |
| `option:PoolRecoveryBody.expected_revision` | The reviewed pool revision for Try again now. This releases only its local suppression, retains shared facts and outstanding native trials, and sends no probe. |
| `state:PoolMember.member_cooling` | Qualifying current native evidence suppresses this member under the pool’s own window and cooldown. |
| `state:PoolMember.member_trial_in_progress` | An actual native recovery attempt already owns the permitted trial; inspection never starts another. |
| `option:Candidate.target` | The canonical exact model/backend, provider, model pool or provider pool. The actual resolved member is stored separately and rechecked by the native source. |
| `option:FallbackView.pool_kind` | An unresolved pool family in a rule preview; it discloses no pool members or backend configuration. |
| `state:CallRecovery.pool_exhausted` | No untried member is eligible under the reviewed pool budget and caller rules. Only a configured later step can choose elsewhere. |
| `state:PoolMember.member_already_tried` | The same canonical leaf has already belonged to this logical recovery chain, including its initial failed execution. Different exact placements or configuration remain distinct. |
| `option:ResourceLeaf.model.backend.config` | Exact adapter configuration participates in identity. The generation.model route applies its object as native parameters, with explicit candidate parameters overriding matching keys; native option validation still applies. |
| `option:NewEnvironment.host_id` | Canonical single-host or host-pool target. Omission inherits Default; explicit null chooses Most room. Accepted creation stores one exact host choice, rechecks both lending authorities and every capacity audience, and keeps it across native job replay. |
| `option:ForkNewEnvironment.host_id` | The same typed host intent for an explicitly new environment in the ordinary fork/switch plan. Merely changing a default never relocates an existing environment. |
| `option:env.host_id.type` | single for an exact host or pool for a managed host pool. |
| `option:env.host_id.resource` | The exact host leaf of a single target. |
| `option:env.host_id.resource.kind` | host; another family is refused before placement. |
| `option:env.host_id.resource.id` | The actual host identifier of a single target. |
| `option:env.host_id.id` | The host pool identifier; the admitted host is stored separately. |
| `option:env.host_id.kind` | host; membership does not grant direct Host Use. |
| `option:runtime.host_id.type` | single for an exact host or pool for a managed host pool. |
| `option:runtime.host_id.resource` | The exact host leaf of a single target. |
| `option:runtime.host_id.resource.kind` | host; another family is refused before placement. |
| `option:runtime.host_id.resource.id` | The actual host identifier of a single target. |
| `option:runtime.host_id.id` | The host pool identifier; the admitted host is stored separately. |
| `option:runtime.host_id.kind` | host; membership does not grant direct Host Use. |
| `option:HostUse.max_environments` | Counts undeleted environment and provider registrations made through the real Host Use entry. Pool and lending audiences keep their independent limits; native placement locks and counts them again before accepting a new unit. |

| Reference | Meaning |
| --- | --- |
| `state:Turn.pool_access_changed` | Current pool, member or lending access no longer permits the admitted work. Its actual native turn stops; direct users of the member keep working and no replacement is chosen implicitly. |
| `error:pool_access_changed` | The safe localized stop reason, without private pool, member or giver names. The original task stays stopped until a permitted selection is restored. |

| Reference | Meaning |
| --- | --- |
| `option:PoolDuplicateBody.expected_revision` | The source revision reviewed for copying; a changed source requires a fresh review. |
| `option:PoolDuplicateBody.home_id` | The actual destination home, checked through ordinary pool creation authority. |
| `option:PoolDuplicateBody.name` | The new pool name. Every written rule and own routing setting is copied without selecting a member. |
| `option:PoolDuplicateBody.access` | The new pool starts with this explicit access choice; source permissions, member consents, health, rotation and usage are not copied. |

| Reference | Meaning |
| --- | --- |
| `option:PoolChoicesQuery.before_created_at` | The saved time of the last row on the previous newest-first page; supply it together with before_id. |
| `option:PoolChoicesQuery.before_id` | The immutable choice ID paired with before_created_at. A cursor grants no access and never reserves a member. |
| `option:PoolChoiceDetailsQuery.after_ordinal` | Continue the original recorded walk after this ordinal. Current membership is not expanded or executed. |
| `option:PoolChoiceView.resource` | The saved selected leaf, disclosed only under current Read for every pool on its path and the leaf, including the actual key response-content sources. |
| `option:PoolChoiceView.policy` | The actual saved root policy of the selected path. Reading it never advances rotation. |
| `option:PoolChoiceView.details_expired_at` | When verbose expansion details expired; the original selection, policy receipt and native execution association remain. |
| `option:PoolChoiceDetailsReply.expired_at` | An explicit expired detail receipt, rather than an invented empty expansion. No member is selected again. |
| `option:PoolChoiceDetail.cost` | The recorded request estimate in credits, available only with the candidate Read sources. Absent means unknown, never free; this is not a usage bill. |
| `option:PoolChoiceDetail.cost_basis` | Details shows the saved rate source and cache assumption alongside a readable estimate. |
| `option:PoolCostBasis.source` | The rate source used when the choice was made. |
| `option:PoolCostBasis.uncached_input` | True when the estimate explicitly priced all input at the uncached rate. |
| [Choosing](state-reference.md#state:PoolChoiceState.building) | The accepted choice is still being recorded; inspection starts no work. |
| [Selected](state-reference.md#state:PoolChoiceState.selected) | One exact saved leaf was selected by the native consumer. Current Read still controls disclosure. |
| [No eligible member](state-reference.md#state:PoolChoiceState.exhausted) | The recorded attempt found no eligible member. Inspection does not retry it. |
| `option:PoolChoiceView.id` | An independent read identity, separate from the private native producer or job ID. Current pool Read is required again on every detail page. |
