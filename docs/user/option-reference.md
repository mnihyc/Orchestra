---
group: providers
screens: [/help, /thread/$threadId, /resources/providers/$id, /settings/interface, /topic/$topicId]
keywords: [reference, settings, options, states, errors, 参考, 设置, 选项, 状态, 错误]
summary: Shipped request and interface options, and what they change.
---

# Option field reference

An option is offered only where its interface or program declares it. Provider limits and permissions still apply. Fields you define in a custom schema keep that schema’s own meaning; this reference covers the shipped declarations. Setting fields are in the [settings reference](settings-reference.md). Read the plan before a change and [the program guide](register-your-program.md#choose-an-arrangement) before choosing an arrangement.

## Entries

| Reference | Meaning |
| --- | --- |
| `option:ThreadOutlineGetQuery.q` | Words in the same projected outline title, goal condition or turn number, matched before the output page. Native history membership and frozen text stay unchanged. |
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
| `option:PersonalState.sidebar.open_links` | The person’s Open links in choice, kept with sidebar state: automatic by default, this or new. Automatic keeps each destination in one view; this replaces the current view and new opens a view. An already open target is focused. Choose it in Interface or a tab menu; explicit clones and browser modifier clicks keep their usual behavior. |
| `option:TopicSearch.q` | The thread-name search on a topic’s Overview and Threads tab. It filters the visible live collection, travels in the topic address and is not retained as a device default. |
| `option:codex.cyber_access_program` | The Codex access program offered by the model and enabled for the provider. The selected name reaches the native turn unchanged, including names outside the Daybreak family. Normal model/account eligibility and provider access still apply. |
| `option:ask.models.*.model` | The enabled API model’s routing name. Together with provider_id it identifies one allowed Ask model; child lists can narrow the choices while retaining the parent’s order. |
| `option:ask.models.*.provider_id` | The API provider’s identity for this allowed Ask model. The person and current payer must still be allowed to use that provider and model in the conversation’s organization. |
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
