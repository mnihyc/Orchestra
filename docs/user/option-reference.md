---
group: providers
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [reference, settings, options, states, errors, 参考, 设置, 选项, 状态, 错误]
summary: Every request option a provider endpoint accepts, and what it changes.
---

# Option field reference

An option is offered only where its interface or program declares it. Provider limits and permissions still apply. Fields you define in a custom schema keep that schema’s own meaning; this reference covers the shipped declarations. Setting fields are in the [settings reference](settings-reference.md). Read the plan before a change and [the program guide](register-your-program.md#choose-an-arrangement) before choosing an arrangement.

## Entries

| Reference | Meaning |
| --- | --- |
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


### Live file reads

| Reference | Meaning |
| --- | --- |
| `option:Thread.starting_since` | When the current preparation began; absent when no start is under way. |
| `option:Thread.starting_work` | The current preparation, its command and what is ahead of it. |
| `option:StartingWork.command` | The accepted native command for this preparation, when one is running or in line. |
| `option:StartingWork.behind` | The work that actually holds the serial line ahead; finished capture uploads do not hold later starts. |
| `option:Turn.files_changed` | Known changed paths at the turn end, from complete before/end coverage or the native change journal; absent means unknown. |
| `option:Turn.files_changed_paths` | The relative changed paths known for this turn, including changes made without a file-tool patch. |
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
