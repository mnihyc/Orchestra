---
group: settings
screens: [/help, /thread/$threadId, /resources/providers/$id]
keywords: [reference, settings, options, states, errors, 参考, 设置, 选项, 状态, 错误]
summary: Every setting, where it can be set and what it changes.
---

# Settings reference

Every built-in setting has a full entry in Help → Settings, with its default, example, allowed scopes and current source when opened from a control. This index names each setting and every declared structured field; `*` stands for a name chosen by its author. See [settings and usage](settings-and-usage.md) for how inheritance works.

For personal read-only questions, see [Ask](ask.md). Interpret reported lifecycle values with the [state reference](state-reference.md).

## Entries

| Reference | Meaning |
| --- | --- |
| `setting:agents.harness_fetch_timeout_secs` | **Program download timeout** — Additional seconds to download and verify a named agent build; defaults to 1800 seconds, matching host tool preparation. Installed local programs keep their ordinary start allowance. |
| `setting:mount.copy_review_secs` | **Copy review time** — A positive number of seconds at the system or organization, 900 (15 minutes) by default. This is an editable review lifetime with no fixed maximum beyond the clock representation. Accepted jobs keep their original review. |
| `setting:mount.delete_data_fate` | **Mount data on deletion** — Keep as shipped, or preselect Purge now where permitted. Each deletion shows its real choices; destroying owned data still requires the mount name. Existing folders are kept. |
| `setting:server.update_prepare_timeout_secs` | **Server update preparation** — 60 seconds by default for the bundled node to prepare its fixed host updater. Positive, with no fixed maximum beyond the native clock. Release download, health and rollback remain governed by update.sh. |
| `setting:public_address.prepare_timeout_secs` | **Address preparation wait** — 60 seconds by default for the managed certificate/worker proof and every candidate origin. Positive, with no fixed maximum beyond the clock representation; failure preserves the effective revision. |
| `setting:public_address.probe_interval_ms` | **Address probe interval** — 250 milliseconds by default between fresh browser-facing attempts while a certificate generation is loading; positive with no fixed maximum. |
| `setting:gateways.primary_change_timeout_secs` | **Gateway change wait** — Time for a saved primary choice and its durable native reply, including reconnecting; 30 seconds by default, positive with no hard upper limit beyond the clock representation. |
| `setting:calls.rules` | **Call recovery** — Ordered rules for retrying, continuing, failing over or waiting after a model call fails. |
| `setting:calls.repeat_limit` | **Repeat limit** — Automatic recoveries of one failure kind within the repeat window; zero stops automatic recovery. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:calls.repeat_window` | **Repeat window** — The rolling time window used to stop repeated recovery. |
| `setting:calls.notify_on_stop` | **Notify on stop** — Tell the thread owner once when the repeat guard stops recovery. |
| `setting:calls.return` | **After failover** — Return to the thread’s own choice at the next turn when ready, or stay on the candidate. |
| `setting:agent.model` | **Model** — The model these threads use. Enter the model ID supplied by its provider. |
| `setting:agent.effort` | **Effort** — The reasoning effort asked of the model, one of those the model lists. |
| `setting:agent.params` | **Agent parameters** — Values for the parameters that the agent program declares, by parameter. |
| `setting:agent.features` | **Agent features** — Explicit choices laid over each program’s declared shipped feature states; false switches a feature off. The shared default is empty, and the switches show the program default and override source. |
| `setting:agent.instructions` | **Instructions** — Standing instructions given to the agent with every session; each scope adds its own under those above it. |
| `setting:agent.memory` | **Agent notes** — Whether an agent keeps notes between sessions: through any available method, through its own program, or not at all. |
| `setting:agents.harness_versions` | **Agent program versions** — The build each agent program uses when an environment is created or updated, inherited through the project and overridden by the environment. An exact agent profile pin wins; changing this does not change a running environment. |
| `option:agents.harness_versions.*.use` | Program build selection policy. |
| `option:agents.harness_versions.*.version` | The exact selected program version. |
| `setting:agents.harness_channel_hours` | **Release check interval** — How often to read release channels when a place requests newer builds; 0 keeps scheduled reads off. |
| `setting:agents.harness_interface_timeout_secs` | **Interface check timeout** — How long a build may take to print its interface. |
| `setting:agents.harness_case_timeout_secs` | **Conformance case timeout** — How long each compatibility test of an agent program may take against the scripted model. |
| `setting:agents.harness_conformance_timeout_secs` | **Conformance check timeout** — How long the complete compatibility check of an agent program may take when run in isolation. |
| `setting:agents.harness_trust_mirror` | **Trust mirror checksums** — Allow the release mirror to publish executable checksums of its own. These builds are labelled as mirror checksums, not vendor checksums. |
| `setting:agents.harness_mirror` | **Release mirror** — A server that serves the vendors' release files where this server cannot reach them. |
| `setting:agents.prompt_cache_ttl` | **Prompt cache lifetime** — An explicit cache lifetime for the agent program's main conversation. Unset uses the program's own default; programs without this control ignore it. |
| `setting:agents.web_action_wait_secs` | **Web action wait** — Close a native Codex web row that has not reported its end, with a missing-end explanation. The default is 120 seconds, inherited through the place settings without an upper cap. The turn continues and a later native end can update the row. Applies when the session opens again. |
| `setting:agents.native_result_wait_ms` | **Native result wait** — System default of 2000 milliseconds for programs that declare a retained-evidence read wait. A profile can choose its own wait within the engine's interrupt window. An expired read keeps the reported action result and leaves unproved usage unknown; other sessions continue. Applies when the program starts again. |
| `setting:agents.native_result_buffer_bytes` | **Native result buffer** — System default of 8 MiB for later native input retained in order while a session reads optional evidence. A profile may choose another size, including zero. When full, only the optional read ends as unverified; all input is processed. Applies when the program starts again. |
| `setting:agents.subagents_at_once` | **Sub-agents at once** — How many sub-agents the agent program may run at once. The smallest value at this place or above applies. An unsupported control says why it does not apply. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:agents.subagent_depth` | **Sub-agent depth** — How many layers of sub-agents may start; 1 allows no nesting. The smallest value at this place or above applies. An unsupported control says why it does not apply. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:agents.skills` | **Skills** — Which skills the agents of threads here are given, by skill: off, the latest version or a pinned version; a lower place may change or switch off an inherited one. |
| `setting:agents.personal_skills` | **Personal skills** — Whether the agent of a thread is also given the personal skills of the person who started it. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:thread.cwd` | **Start directory** — Where the agent starts inside the thread's directory, as a relative path. |
| `setting:thread.side_threads` | **Side threads** — Whether side threads may be started from the threads here. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:thread.side_max` | **Side threads at once** — How many side threads of one thread may be open at once. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:thread.side_idle_hours` | **Side thread idle time** — After how many hours without a turn an open side thread is closed. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:thread.inputs_without_use` | **Writers without use** — What happens to a message for the agent from somebody who may write here but may not use the thread's model or its environment. A lower scope can only choose a stricter option. Strictness order: `allowed` → `held` → `refused` (strictest last). |
| `setting:thread.switch_history` | **History on a switch** — What a switch or a fork carries of the conversation unless its plan says otherwise. |
| `setting:thread.switch_carry_share` | **Carried context** — How much of the new model's context window a switch may fill with the conversation it carries. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:thread.native_fork_check_bytes` | **Built-in fork check size** — The most a conversation may take for a fork at one of its turns to be checked against the agent program's own record, so that the program can make the fork itself. A longer conversation is carried as the newest turns that fit. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:thread.read_turn_bytes` | **Turn read size** — The most the messages of one turn may take, written out in full, for Orch to read that turn into a summary of the conversation or a goal check; a larger turn is named and left out, and nothing of it is read. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:thread.plan_first` | **Plan first** — Whether the composer's Plan switch starts on for a thread's first message. The approval mode stays as it is. |
| `setting:thread.plan_answered_by` | **Who answers plans** — Who may proceed with a plan an agent proposes, or keep it planning. A lower scope can only choose a stricter option. Strictness order: `writers` → `managers` (strictest last). |
| `setting:thread.silent_after_secs` | **Silence notice** — How many seconds nothing may arrive from a running agent, with no tool running, before the status line says how long nothing came. |
| `setting:thread.goal.max_turns` | **Goal turns** — The most turns a goal may use here; 0 is no limit. A goal may set a stricter own limit; it cannot raise an ancestor limit. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:thread.goal.max_minutes` | **Goal minutes** — The most minutes a goal may be active here; 0 is no limit. It is looked at between turns: a running turn is not cut short. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:thread.goal.max_credits` | **Goal credits** — The most credits a goal may use here, written as a decimal number; 0 is no limit. Usage caps hold as always. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:approval.mode` | **Approval mode** — How an agent program asks before it acts: Ask, On request, Auto (its own reviewer decides) or Bypass (nothing is asked). |
| `setting:approval.allowed_modes` | **Allowed approval modes** — The approval modes that may be chosen here. |
| `setting:sandbox.mode` | **Tool access** — What the agent program lets commands do: read files, change workspace files, or use everything the environment allows. |
| `setting:agent.messages` | **Messages to agents** — Texts that replace what Orch itself says to agents, by message. The `turn_pool_access_changed` message explains that live pool access stopped the previous turn; it does not attribute the stop to a person judging the task. |
| `setting:models.allowed` | **Allowed models** — Patterns naming the models that may be used here at all; a model has to be named by every scope that sets a list. |
| `setting:services.allowed` | **Allowed services** — Patterns naming enabled provider services that may be used here; every place that sets a list must include the service. |
| `setting:providers.allowed` | **Allowed providers** — Patterns over providers' Orch addresses; every scope that sets a list must admit the provider used for work here. |
| `setting:models.add_switches_on` | **Switched on when added** — Context, speeds and access (`all`) enables the reported context sizes, speeds and access programs; Efforts and context (`basic`) enables context sizes; Only what it states (`nothing`) keeps the model's own window and efforts. Web search, cache lifetimes, reasoning modes, capacity tiers and request options are configured separately. Larger contexts remain a per-thread choice. |
| `setting:collab.attribution` | **Attribution** — Whether Orch's composed input, stop, denial and forwarded text names people: auto once a second person's words are delivered, always, or never (a person/an approver, with source, time and notes kept). Authored chats and documents retain their writers. |
| `setting:collab.default_delivery` | **Default delivery** — What an input does when a turn is running, unless its sender says otherwise: wait for the next turn, or steer the running one. |
| `setting:composer.paste_inline_context_share` | **Pasted text in the message** — The default share of remaining context for pasted text. 0 always sends it as a file; 100 always sends it in the message. The sender can choose for each paste. |
| `setting:env.thread_default` | **New threads** — Whether a new thread joins the default environment inherited from the place above or is given one of its own. |
| `setting:env.default_id` | **Default environment** — The environment new threads join when they share one and ask for no other. The creator must manage it. Clear the choice to inherit the place above. |
| `setting:env.update` | **Updates** — Whether an environment that is behind waits for somebody to update it, or is updated whenever it starts. |
| `setting:env.template_id` | **Template** — The template that a managed environment is made from. |
| `setting:env.host_id` | **Host** — A host or host pool for a newly made environment; unset chooses Most room. An existing environment keeps its admitted host. |
| `setting:env.runtime` | **Container runtime** — Choose a container runtime the host offers. Leave unset to use the template’s default. |
| `setting:environments.runtimes` | **Container runtimes** — Allow, forbid or require container runtimes by name. Use * for every container runtime not named. |
| `setting:env.storage` | **Storage** — Named storage locations on the machine for new workspace, home and connector state data. Existing data keeps its recorded location. |
| `option:env.storage.workspace` | Storage for workspace files. |
| `option:env.storage.home` | Agent home storage. |
| `option:env.storage.state` | Recorded file-state storage. |
| `setting:env.data_sources` | **Workspace and home** — Where the workspace and the home each live: made by Orch, or an existing directory that this machine's owners allow. Existing directories keep their owner and mode, have no volume size limit and are never removed by a purge. |
| `option:env.data_sources.workspace` | Source of workspace files. |
| `option:env.data_sources.workspace.kind` | The declared source or content kind. |
| `option:env.data_sources.workspace.path` | The path on the machine that owns this setting. |
| `option:env.data_sources.home` | Agent home storage. |
| `option:env.data_sources.home.kind` | The declared source or content kind. |
| `option:env.data_sources.home.path` | The path on the machine that owns this setting. |
| `setting:env.after_up` | **Commands after start** — Named commands, each with its arguments, run inside a managed environment after every start. Setting a name to null removes one inherited from above; a command that fails stops the start from completing. |
| `option:env.after_up.*.command` | Executable and arguments, run directly without a shell. |
| `option:env.after_up.*.timeout_secs` | Command timeout in seconds. |
| `setting:env.mounts` | **Mounts** — The mounts attached beside the workspace, each as the mount says, off, read-only, or read-only with the environment's own changes kept apart. |
| `setting:env.resources` | **Limits** — The limits of an environment in place of its template's: processor, memory, processes and disk. |
| `option:env.resources.cpu_millis` | CPU allowance in thousandths of a CPU. |
| `option:env.resources.memory_mb` | Memory allowance in MiB. |
| `option:env.resources.memory_swap_mb` | Combined memory and swap allowance in MiB. |
| `option:env.resources.blkio_weight` | Relative block-I/O scheduling weight. |
| `option:env.resources.pids` | Maximum process count. |
| `option:env.resources.disk_gb` | Disk allowance in GiB. |
| `setting:env.needs` | **Needs** — What an environment declares it needs at least, for choosing where it runs: memory, processor and disk. |
| `option:env.needs.memory_mb` | Memory allowance in MiB. |
| `option:env.needs.cpu_millis` | CPU allowance in thousandths of a CPU. |
| `option:env.needs.disk_gb` | Disk allowance in GiB. |
| `setting:env.idle_stop_minutes` | **Idle stop** — Stop a managed environment after this many minutes without activity; 0 never stops it. |
| `setting:env.variables` | **Variables** — Variables given to the environment, by name. |
| `setting:env.secret_names` | **Secrets** — The names of the secrets that an environment is given. |
| `setting:network.allowed_hosts` | **Allowed hosts** — Exact names of other hosts that a restricted environment may reach; the lists set at different places are intersected, and an empty list allows only Orch's own connection. |
| `setting:mcp.enabled` | **MCP servers** — Which MCP servers are switched on, by server; a lower scope may switch an inherited one off. |
| `setting:exec.version_overrides` | **Any agent program version** — Machine program paths and versions that an environment manager allows with any version of the agent program. A changed path or version needs approval again. |
| `option:exec.version_overrides.*.path` | The path on the machine that owns this setting. |
| `option:exec.version_overrides.*.version` | The exact selected program version. |
| `option:exec.version_overrides.*.by` | The recorded author, or the declared sort order for navigation. |
| `option:exec.version_overrides.*.at` | When the decision was recorded. |
| `setting:exec.offers` | **Exec offers** — How an agent running with its provider may run tools in this environment, in the order tried. |
| `setting:exec.orch_tools` | **Orch's own tools** — Whether the agents that work in this environment are offered Orch's own tools. |
| `setting:orch_tools.enabled` | **Orch tools** — Which of Orch's own tools an agent is offered. |
| `setting:orch_tools.owner_reads` | **Personal read tools** — Off as shipped. Let the thread use its owner’s current read access, including when shared. Its audience banner shows who can see or act and warns that personal reads are visible to them. Sharing earlier results still requires informed confirmation. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:orch_tools.calls_per_minute` | **Tool calls a minute** — How many calls of Orch's tools one thread's agent may make in a minute. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:orch_tools.read_bytes` | **Shared read bytes** — Default UTF-8 bytes in a shared read or tool answer; explicit positive byte windows can differ. |
| `setting:orch_tools.list_depth` | **Shared listing depth** — Default levels in a shared listing; continuation retains the remaining entries. |
| `setting:orch_tools.list_rows` | **Shared listing rows** — Default rows in a shared listing page; explicit positive windows can differ. |
| `setting:orch_tools.search_query_chars` | **Shared search query budget** — The query character budget advertised to this thread's agent and used by shared search. Defaults to 1000; increase it for longer queries. |
| `setting:orch_tools.search_results` | **Shared search result budget** — The result budget advertised to this thread's agent and used by shared search. Defaults to 200; the search can still stop at its byte and time limits, and can then be continued. |
| `setting:orch_tools.search_context` | **Shared search context budget** — The surrounding-line budget advertised to this thread's agent and used by shared-search snippets. Defaults to 5; increase it for wider context. |
| `setting:threads.opening_items` | **Thread opening items** — Positive number of timeline items read when a thread first opens. Defaults to 40; an explicit request can choose another window, and older items remain available by cursor. |
| `setting:threads.offline_deadline` | **Offline turn deadline** — End the active turn after the machine has been offline this many seconds; leave it empty to keep waiting. An outage that has begun keeps the setting it began with. |
| `setting:threads.start_acceptance_seconds` | **Start acceptance wait** — Seconds an admitted start waits for native acceptance. Zero expires immediately; each admission keeps its own deadline. Defaults to 60 seconds; there is no additional upper cap. |
| `setting:threads.start_confirmation_seconds` | **Native start wait** — Seconds after the latest possible acceptance to wait for the native start event. Zero adds no wait. Changes apply to later admissions. Defaults to 60 seconds; there is no additional upper cap. |
| `setting:threads.start_event_grace_seconds` | **Start event ingestion grace** — Extra seconds while this session’s native events are still being ingested. Zero adds no grace; each admission keeps this allowance. Defaults to 600 seconds; there is no additional upper cap. |
| `setting:threads.output_capture_bytes` | **Retained command output** — Bytes retained for each native text output, including its inline tail; default 67108864 (64 MiB). Larger outputs keep the exact last bytes and disclose the missing original interval. Applies to the next native session. |
| `setting:threads.background_snapshot_seconds` | **Roster retention** — Idle seconds for native snapshots and reviewed End identities; default 30. Page reads renew retention; expired reads restart before confirmation. |
| `setting:threads.background_page_items` | **Native roster page** — Preferred commands per native page; default 200. Every page transfers, without a complete-list row cap. |
| `setting:threads.background_page_bytes` | **Native roster page bytes** — Preferred bytes per native page; default 131072. One complete command may exceed the target; negotiated transport limits still apply. |
| `setting:threads.background_preview_bytes` | **Roster command preview** — Preview bytes per command; default 4096. Longer previews are explicitly marked truncated; every identity remains available. |
| `setting:threads.background_wake` | **Background wake** — Wake when background tasks or sub-agents finish. Off keeps Codex results for the next input. Claude Code background tasks are not offered and commands run in the foreground. Changes apply to the next session. |
| `setting:threads.agent_access` | **Agent access** — The initial reach and area levels shown when creating a thread. The creator confirms what they may give. Applies to new threads, without rewriting existing grants. |
| `option:threads.agent_access.where` | The area to which the agent contribution applies. |
| `option:threads.agent_access.parts` | Rights on individual parts of the chosen area. |
| `option:threads.agent_access.parts.chat` | Rights on the discussion. |
| `option:threads.agent_access.parts.documents` | Rights on documents. |
| `option:threads.agent_access.parts.timeline` | Rights on thread history. |
| `option:threads.agent_access.parts.thread_fork` | Rights to fork threads. |
| `option:threads.agent_access.parts.thread_archive` | Rights to archive threads. |
| `setting:threads.let_viewers_in` | **Viewers can open the environment** — The initial choice to let a new thread’s viewers look at its files, terminal and screens, without Use or permission to change them. Confirmed when the thread is created; changing this default grants no access by itself. |
| `setting:broadcast.chat_messages_per_hour` | **Agent chat messages per hour** — Successful posts across all chats from one origin in a rolling hour; zero forbids posting. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:sharing.snapshot_max_bytes` | **Largest copy** — The largest copy that may be shared with an agent in one request, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:export.max_bytes` | **Largest export** — The largest size one export of what lies here may come to, in bytes; one that would pass it is not begun, and says so with its size. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:export.max_items` | **Most items in an export** — How many threads, chats and documents one export of what lies here may hold. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:export.keep_hours` | **Exports are kept** — For how many hours a finished export is kept for the person who started it to download before it is removed. |
| `setting:import.max_bytes` | **Import size limit** — The expanded bytes allowed in one import. When unset, the place's export size limit applies. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:import.max_items` | **Import item limit** — The objects allowed in one import. When unset, the place's export item limit applies. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:workspace.ignore` | **Ignored paths** — Patterns, written as in a gitignore file, for paths to hide in a workspace: they are absent from its file lists, search, listings of the tree and recorded turn files, and cannot be read or written through Orch. It is for what a person wants hidden, never for speed: a large tree needs no list. |
| `setting:workspace.index` | **Index** — Whether the whole tree of a workspace is indexed for use while it is offline, or only listed while its environment is online. An online workspace is listed one directory at a time from its machine either way, however large the tree. |
| `setting:workspace.index_entries` | **Cached file entries** — Path metadata kept in the incremental working set; larger directories remain browsable and searchable live. |
| `setting:workspace.index_scan_secs` | **Indexing slice** — Seconds of initial indexing before retaining incremental coverage and continuing with live reads. |
| `setting:workspace.ignore_max_bytes` | **Exclusion parsing** — Bytes allowed when parsing one local exclusion file; a file that cannot be applied reports its source and never exposes an unknown scope. |
| `setting:workspace.ignore_cache_bytes` | **Cached exclusion rules** — Source bytes and record metadata retained for parsed exclusion rules; older parses are read again when needed. |
| `setting:workspace.directory_idle_secs` | **Folder continuation idle time** — Seconds an unused folder observation is kept before transparent renewal when the folder is unchanged. |
| `setting:workspace.directory_slots` | **Folder observations** — Folder observations retained on one machine; unused observations are evicted before admitting another reader. |
| `setting:workspace.directory_cache_bytes` | **File read cache** — Shared working bytes for directory metadata and live file-read continuation buffers; existing work stays charged until it ends. |
| `setting:workspace.directory_builds` | **Folder metadata reads** — Concurrent folder metadata builds on one machine; waiting readers retain their place. |
| `setting:workspace.directory_page_bytes` | **Folder page bytes** — Default metadata bytes in one folder page; continuation retains all remaining entries. |
| `setting:workspace.directory_entries` | **Folder rows** — Default rows in one page; an explicit window can differ and continuation keeps the rest. |
| `setting:workspace.search_results` | **File search rows** — Default rows in one page; an explicit window can differ and continuation keeps the rest. |
| `setting:workspace.archive_results` | **Archive rows** — Default rows in one page; an explicit window can differ and continuation keeps the rest. |
| `setting:workspace.search_slice_secs` | **File search slice** — Seconds of work in one file-search page; continuation resumes the remaining search. |
| `setting:workspace.git_diff_bytes` | **Diff bytes** — Default bytes in one diff read; continuation retains the rest of the selected diff. |
| `setting:source.clone_idle_secs` | **Clone idle time** — Seconds without repository transfer progress before a clone stops and can be retried. |
| `setting:source.repository_url` | **Repository** — The repository prepared only when the thread's directory is missing; an existing directory keeps its own files. |
| `setting:source.branch` | **Source branch** — The branch to prepare; unset, use the repository's advertised default branch. |
| `setting:source.secret_name` | **Source secret** — The name of an existing secret that holds the credential for reading the repository over HTTPS; its value is given only to the command that prepares the repository. |
| `setting:workspace.record_days` | **Keep turn files** — Days to keep complete turn and restore undo files in the environment; zero disables recording. |
| `setting:workspace.record_max_bytes` | **Largest recorded directory** — The included file and link bytes eligible for whole-state recording; larger directories use per-file undo. |
| `setting:workspace.record_wait_secs` | **Wait for turn files** — Seconds to wait for a whole recording before a turn. The shipped default skips initial indexing; choosing a wait also waits for indexing. Changed files can still be restored individually when whole recording is not ready. |
| `setting:broadcast.delivery` | **Announcement delivery** — How an announcement reaches a thread whose turn is running: it interrupts, steers, or waits for the next turn. |
| `setting:broadcast.fallback` | **When steering is impossible** — What is done instead when an announcement is to steer an agent that cannot be steered. |
| `setting:broadcast.wake_idle` | **Allow waking idle threads** — Whether a send may wake an idle thread here; on as shipped. Each announcement or thread message chooses wake explicitly, false when omitted. |
| `setting:broadcast.agent_options` | **Delivery choices for agents** — Allowed delivery overrides for an agent's announcement, excluding wake; each send chooses wake separately. A lower scope may remove allowed choices; it cannot add one excluded above. |
| `setting:terminal.enabled` | **Terminals** — Whether interactive terminals may be opened. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:terminal.record` | **Keep the last output** — Whether the last of what a terminal printed is kept in its environment's history when the terminal closes. |
| `setting:terminal.detached_minutes` | **Detached time** — For how many minutes a terminal keeps running with nobody attached before it is closed. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:terminal.max_per_person` | **Terminals per person** — How many terminals one person may have open at once in one environment. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:terminal.max_per_environment` | **Terminals per environment** — How many terminals may be open at once in one environment, everybody's together. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:browser.enabled` | **Browsers** — Whether the environments here show their browsers, start one when asked, and offer their agents the tool for them. Off, an agent may still run a browser as it runs any program. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:ports.enabled` | **Ports** — Whether people who already have access to the environment may use owned TCP and UDP ports through Orch. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:ports.max_open` | **Wider ports** — The distinct port numbers with wider or pending audiences; zero leaves own use available. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:ports.link_max_secs` | **Link lifetime** — The longest link lifetime from creation, including later expiry changes. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:requests.expiry_minutes` | **Pending agent requests** — The longest pending boundary decision an agent requests; an earlier requested port-link lapse also ends it. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:browser.preset` | **Browser preset** — How a browser is started where nobody says: headless, with no window and no display, or with a window on a display that Orch opens for it. |
| `setting:browser.args` | **Browser arguments** — Arguments added to those a browser is started with; the ones Orch gives itself, the profile's directory and the address it is driven at, cannot be set here. |
| `setting:browser.viewport` | **Browser window size** — The size of a new browser's window, written width x height, from 320x240 to 3840x2160. |
| `setting:browser.idle_minutes` | **Browser idle close** — Close a browser that Orch started once it has been idle this many minutes: nobody watched or drove it and no agent's tool named it; 0 never does. |
| `setting:browser.max_per_environment` | **Browsers per environment** — How many browsers that Orch started may run at once in one environment. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:vnc.enabled` | **Displays** — Whether the environments here show their displays, open one when asked, and offer their agents the tool for them. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:vnc.geometry` | **Display size** — The size of a display that Orch opens, written width x height, from 320x240 to 3840x2160. |
| `setting:vnc.max_per_environment` | **Displays per environment** — How many displays that Orch opened may run at once in one environment. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:env.idle_warning_minutes` | **Idle warning** — How many minutes before an idle stop its warning is sent; 0 sends none. |
| `setting:env.removed_keep_days` | **Removed files** — Days to keep owned files after an environment is removed, bounded by the system retention window; 0 makes them eligible for immediate removal. |
| `setting:env.unused_remove_days` | **Remove when unused** — Schedule the removal of an environment made from a template once no thread has been in it for this many days; 0 never does. |
| `setting:env.lifetime` | **Lifetime** — Keep this environment, remove it after idle time, or also after its creating thread is archived. Only owned files are deleted. |
| `setting:env.temporary_idle_hours` | **Temporary idle time** — Hours without activity before a temporary environment and its owned files are removed. Its owner gets an hour to keep or export it. |
| `setting:env.recycle_idle_minutes` | **Recycle idle time** — The idle time, in minutes, that the Recycle dialog starts from; 0 applies no minimum idle time. |
| `setting:hosts.full_percent` | **Full at** — The share of a host's memory, declared as needed by what runs there, at which the host counts as full. |
| `setting:hosts.when_full` | **When a host is full** — Whether making or moving an environment onto a full host only warns, or is refused beforehand. |
| `setting:runtime.idle_stop_minutes` | **Idle stop** — Keep a managed provider running with 0, the default. A positive number stops it after that many idle minutes without a session or a running turn. Scheduled reads leave it stopped until a turn, sign-in, Start, Refresh or renewal starts it again. |
| `setting:runtime.template_id` | **Template** — The provider template that a managed provider is made from. |
| `setting:runtime.host_id` | **Host** — A host or host pool for a newly made provider runtime; unset chooses Most room. Existing volumes stay on their admitted host. |
| `setting:delegation.background_headroom_percent` | **Background plan reserve** — Percentage of a subscription left for other work. Zero disables the policy. With a positive reserve, automatic titles, tags and resource summaries wait at or below the reserve, or for a current applicable measurement. They keep the same request and check again at the provider’s observation cadence. Personal refreshes and already admitted work keep their original allowance. |
| `setting:delegation.session_idle_secs` | **Context idle time** — Close a delegated program's private conversation after 300 idle seconds by default. Zero closes it as soon as its work finishes. Keep the helper history and the provider's independent runtime policy; active or later independent work is left alone. |
| `setting:runtime.resources` | **Limits** — Limits over the template's values: processor, memory, processes and disk. The driver reports which limits it enforces. |
| `option:runtime.resources.cpu_millis` | CPU allowance in thousandths of a CPU. |
| `option:runtime.resources.memory_mb` | Memory allowance in MiB. |
| `option:runtime.resources.memory_swap_mb` | Combined memory and swap allowance in MiB. |
| `option:runtime.resources.blkio_weight` | Relative block-I/O scheduling weight. |
| `option:runtime.resources.pids` | Maximum process count. |
| `option:runtime.resources.disk_gb` | Disk allowance in GiB. |
| `setting:runtime.storage` | **Storage** — Named storage locations on the machine for new provider home and connector state data. Existing data keeps its recorded location. Managed providers have no workspace. |
| `option:runtime.storage.workspace` | Storage for workspace files. |
| `option:runtime.storage.home` | Agent home storage. |
| `option:runtime.storage.state` | Recorded file-state storage. |
| `setting:runtime.home` | **Agent home** — A home made by Orch or an existing directory allowed by this machine. Leave unset to keep the agent profile’s choice or the agent program’s default and its existing sign-in. Existing directories keep their owner, permissions and files; deleting the managed provider never deletes them. |
| `option:runtime.home.kind` | The declared source or content kind. |
| `option:runtime.home.path` | The path on the machine that owns this setting. |
| `setting:runtime.needs` | **Needs** — The processor, memory and disk a managed provider declares it needs, for choosing where it runs. |
| `option:runtime.needs.memory_mb` | Memory allowance in MiB. |
| `option:runtime.needs.cpu_millis` | CPU allowance in thousandths of a CPU. |
| `option:runtime.needs.disk_gb` | Disk allowance in GiB. |
| `setting:runtime.after_up` | **Commands after start** — Named commands, each with its arguments, run for a managed provider after each start or attachment, where its machine permits them. Only its owners can change them on the provider. |
| `option:runtime.after_up.*.command` | Executable and arguments, run directly without a shell. |
| `option:runtime.after_up.*.timeout_secs` | Command timeout in seconds. |
| `setting:runtime.variables` | **Variables** — These variables override the template and agent profile when the managed provider next starts or attaches. |
| `setting:runtime.secret_names` | **Secrets** — Names of provider secrets that override the agent profile's variables. Each value is kept readable only by the provider's machine, is entered separately and is never shown again. |
| `setting:runtime.update` | **Updates** — Whether a managed provider that is behind waits for somebody to update it, or is updated whenever it starts. |
| `setting:runtime.renew_days` | **Renewal** — Renew a sign-in when fewer than this many days remain, or this many days after its last renewal when its expiry is unknown; 0 switches renewal off. |
| `setting:providers.facts_idle_minutes` | **Idle facts interval** — Read provider facts this often while no bound thread has worked recently. Defaults to 60 minutes; at least 1 minute. |
| `setting:providers.facts_active_minutes` | **Active facts interval** — Read provider facts this often while a bound thread runs, has worked recently, or waits on a used-up plan. Defaults to 5 minutes; at least 1 minute. |
| `setting:providers.facts_recent_minutes` | **Recent provider activity** — Keep a provider on the active facts interval for this many minutes after a bound thread's turn. Defaults to 15 minutes; at least 1 minute. |
| `setting:providers.facts_refused_minutes` | **Refused facts interval** — Read provider facts this often while its credential is refused, to find an external repair. Defaults to 5 minutes; at least 1 minute. |
| `setting:providers.presets` | **Provider presets** — Ready-made API upstreams offered when a provider is added, by name; a lower scope adds one, replaces one, or hides one. |
| `option:providers.presets.*.note` | The explanation recorded with the rule. |
| `option:providers.presets.*.docs_url` | The source documentation address. |
| `option:providers.presets.*.upstreams` | Upstream endpoints for the offering. |
| `option:providers.presets.*.upstreams.*.protocol` | The endpoint interface declared by this source. |
| `option:providers.presets.*.upstreams.*.base_url` | The endpoint base address. |
| `option:providers.presets.*.upstreams.*.options` | Options passed through the declared endpoint schema. |
| `option:providers.presets.*.template` | The environment template to apply. |
| `option:providers.presets.*.model` | The selected model. |
| `option:providers.presets.*.dimensions` | Vector dimensions for this index. |
| `setting:providers.allow_http_upstreams` | **Upstreams without TLS** — Whether an API endpoint may be reached without TLS, as for a model server on a private network. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:providers.expiry_notice_days` | **Expiry notice** — How many days before a provider token expires its managers are told; 0 tells them only after expiry. A provider may override its home’s value; removing its override follows the home again. |
| `setting:providers.subscription_harnesses` | **Subscriptions allowed** — The agent programs whose subscriptions may be added as providers here. |
| `setting:providers.subscription_sharing` | **Subscriptions shared** — The agent programs whose subscriptions may be used by people other than their owner. |
| `setting:resources.created_by_members` | **Resources members may add** — The kinds of resource that any member may make in an organization as their own. |
| `setting:skills.max_bytes` | **Skill size** — The most bytes the files of one version of a skill hold together. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:skills.max_files` | **Skill files** — The most files one version of a skill holds. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:notify.subagents` | **Sub-agent notices** — Whether the people who follow a thread are told when one of its sub-agents finishes, fails or is stopped, as they are told when a turn ends. |
| `setting:message.body_max_chars` | **Message length** — How many characters an administrator message may contain; the default can be changed at the system or organization. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:notify.operations` | **Operation notices** — Whether Notify starts ticked in an operation (Never by default): when work is blocked, always, or never. The person confirms the choice; recipients keep their own notification choices. |
| `setting:broadcast.rate_per_hour` | **Announcements an hour** — How many announcements one thread's agent may send in a rolling hour; default 12, 0 allows none. Individual thread messages count separately. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:broadcast.thread_messages_per_hour` | **Thread messages an hour** — How many individual thread messages one sending thread's agent may send in a rolling hour; default 60, 0 allows none. Announcements count separately. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:capability.recreate_grace_secs` | **Recreate grace** — How many seconds running turns may finish before a recreate interrupts them; 0 interrupts them immediately. |
| `setting:access.request_expiry_minutes` | **Access request expiry** — After how many minutes a request for access that nobody decided lapses. |
| `setting:access.max_pending_requests` | **Pending requests** — How many requests for access one person may have waiting. |
| `setting:access.quick_grant_durations` | **Quick access durations** — Durations offered for giving access in one click: a positive number of days followed by d, or always. A lower scope may remove allowed choices; it cannot add one excluded above. |
| `setting:nodes.new_color` | **New place colour** — Whether a new place starts with its kind's icon colour or one of the twelve picked at random. |
| `setting:tags.max_per_place` | **Tags on a place** — How many tags one place carries at most; 0 allows none. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:agent.default_provider_id` | **Default provider** — The provider that new threads here are put on. |
| `setting:agent.default_agent_id` | **Default agent profile** — The agent profile that new threads here use. |
| `setting:templates.restricted_drivers` | **Restricted drivers** — The drivers whose templates only system admins may define. |
| `setting:switch.batch_parallel` | **Batch size** — How many switches, moves or updates of one batch run at the same time. |
| `setting:switch.batch_threads` | **Threads per batch** — How many threads one batch may plan; raise this default for a larger reviewed switch. |
| `setting:jobs.measurement_keepalive_secs` | **Keep counts running** — Seconds a count keeps running after its last plan closes. |
| `setting:jobs.measurement_valid_secs` | **Reuse counts** — Seconds a completed count stays valid for another plan. |
| `setting:jobs.measurement_timeout_secs` | **Counting time** — Seconds a machine may take to finish a count. |
| `setting:jobs.stall_secs` | **Unchanged progress** — Seconds without changed progress before the job names the wait. |
| `setting:jobs.stop_wait_secs` | **Stopping wait** — Seconds before an unconfirmed stop names its machine. |
| `setting:jobs.io_timeout_secs` | **Copy time** — Seconds a machine may take to finish one file operation. |
| `setting:switch.active_turn` | **Running turn** — What a switch, a move, a recreate or an update does with a turn that is running: wait for it, or interrupt it. |
| `setting:switch.wait_minutes` | **Wait** — For how many minutes it waits for running turns to end before it gives up and leaves everything as it was; 0 does not wait. |
| `setting:switch.verify_timeout_secs` | **Health check time** — How many seconds the checks of the new side may take before everything is rolled back. |
| `setting:switch.keep_previous` | **Keep the old side** — Whether the side that a switch or a move leaves behind is kept instead of scheduled for removal. |
| `setting:binding.prefer` | **Preferred agent setup** — Ways to run an agent, tried first in this order. Other supported options follow. |
| `setting:binding.allowed` | **Allowed agent setups** — Control where agents may run and whether sign-in access may be shared with an environment. |
| `setting:credential.refresh_margin_secs` | **Credential refresh margin** — A shared copy of a credential that expires is renewed when fewer than this many seconds remain. |
| `setting:credential.refresh_min_interval_secs` | **Credential renewal interval** — System default: 30 seconds between automatic renewals of an expiring shared copy; 0 removes this spacing. A harness request for a fresh copy is served immediately after any issue already in progress, regardless of this interval. The provider retains its original sign-in. |
| `setting:auto.title` | **Titles** — How automatic titles are written: no title, the first input, the agent’s title, or a summary. |
| `setting:auto.tags` | **Tags** — Whether the summarizer sets a place's tags. |
| `setting:summary.handoff_input_share` | **Summary input share** — How much of the summary model's context window a hand-off, a fork or a goal check may fill with the conversation it sends: the newest turns first, older ones as their one-line index. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:auto.summarizer` | **Summarizer** — Summarizer and model token allowances. Target character counts are separate. |
| `option:auto.summarizer.backend` | The exact model backend, including its route, account offering and adapter references. Re-reading a choice never changes its route or bill. |
| `option:auto.summarizer.backend.route` | The installed execution route selected for the model producer. An unavailable route is reported rather than replaced. |
| `option:auto.summarizer.backend.offering` | The selected account and provider-local model identity. |
| `option:auto.summarizer.backend.offering.provider` | The selected provider identity; ordinary Use and sharing still apply. |
| `option:auto.summarizer.backend.offering.offering` | The selected provider-local model routing name. |
| `option:auto.summarizer.backend.config` | Adapter-owned references, including admitted program placement where needed. It contains no credential or source authority. |
| `option:auto.summarizer.kind` | The declared source or content kind. |
| `option:auto.summarizer.params` | For the model summarizer, provider request options. Explicit effort choices are preserved within the current model and permission limits. |
| `option:auto.summarizer.answer_max_output_tokens` | For the model summarizer, answer tokens for paragraphs, descriptions, tags and handoffs; defaults to 1,024, separate from target characters and added reasoning room. |
| `option:auto.summarizer.title_max_output_tokens` | For the model summarizer, title answer tokens; defaults to 120, separate from the 80-character title target and added reasoning room. |
| `option:auto.summarizer.goal_max_output_tokens` | For the model summarizer, answer tokens for a goal verdict and its reason; defaults to 512. Any positive choice is kept without an extra minimum. |
| `option:auto.summarizer.reasoning_tokens` | For the model summarizer, extra output room for known or unrecorded upstream reasoning; defaults to 2,000 tokens. Zero adds no room. Only an explicit nonreasoning model fact removes this reserve. |
| `option:auto.summarizer.context_tokens` | For the model summarizer, input tokens including instructions and framing; defaults to 32,000 within the model's window after the actual field's complete output reserve. |
| `option:auto.summarizer.prompts` | For the model summarizer, replacement instructions by field. An omitted or null instruction keeps that field's shipped prompt. |
| `setting:auto.triggers` | **Triggers** — When automatic titles, tags and summaries are written again: after a turn, at a milestone or at intervals, and how often at most. |
| `option:auto.triggers.on` | Events that trigger this automation. |
| `option:auto.triggers.min_interval_secs` | Minimum gap between automatic runs in seconds. |
| `option:auto.triggers.interval_secs` | Scheduled interval in seconds. |
| `setting:capability.policy` | **Capability policy** — For each capability, whether a request for it is allowed, asked about or denied, and who decides. |
| `option:capability.policy.*.decision` | Whether the capability is allowed or requires a decision. |
| `option:capability.policy.*.approver_scope` | Which area supplies the approvers. |
| `setting:sharing.allow_external` | **Owners share outside** — Whether the owners of a thing here may themselves add somebody from outside its root organization on its page. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:sharing.allow_copy_out` | **Copies to other orgs** — Whether things here may be duplicated or moved into another organization. On by default. A lower scope can switch this off; it cannot undo a refusal set above. Exporting and creating at the destination still need their own permissions. |
| `setting:sharing.external_max_minutes` | **Outside entries end** — The longest an entry that names somebody from outside may last after it is written or renewed. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:sharing.allow_holders` | **Holders share** — Whether somebody who holds a thing here without owning it or managing its page may share it, at most at their own level: name others on its page, give a role in a place, share it with agents. When off, only a thing's owners and whoever manages its page may share it. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:sharing.on_link` | **Share viewing on send** — The default viewing-share choice for links sent here. A person can switch this off for themselves. |
| `setting:usage.rates` | **Rate card** — Model prices in credits, matched by model ID. |
| `option:usage.rates.*.web_search_per_call` | Credits per successful web-search call. |
| `option:usage.rates.*.input_per_mtok` | Credits per million input tokens. |
| `option:usage.rates.*.cached_input_per_mtok` | Credits per million cached input tokens. |
| `option:usage.rates.*.output_per_mtok` | Credits per million output tokens. |
| `option:usage.rates.*.cache_write_per_mtok` | Credits per million cache-write tokens. |
| `option:usage.rates.*.cache_write_1h_per_mtok` | Credits per million cache-write tokens retained for one hour. These tokens are part of the total cache writes; this quote replaces their ordinary write price. An absent quote stays unknown. |
| `option:usage.rates.*.cache_write_12h_per_mtok` | Credits per million cache-write tokens retained for twelve hours. They are a disjoint part of total writes; this quote replaces their ordinary write price. An absent quote stays unknown. |
| `option:usage.rates.*.tiers` | Rates or limits by the declared tier. |
| `option:usage.rates.*.capacity_tiers` | Decimal price multipliers by the capacity tier actually reported for a call, independent of its speed. Native search and fetch fees keep their own prices. |
| `option:usage.rates.*.above` | Rates that apply above the given context size. |
| `option:usage.rates.*.above.*.context_tokens` | Context-size boundary in tokens. |
| `option:usage.rates.*.above.*.input_per_mtok` | Credits per million input tokens. |
| `option:usage.rates.*.above.*.cached_input_per_mtok` | Credits per million cached input tokens. |
| `option:usage.rates.*.above.*.output_per_mtok` | Credits per million output tokens. |
| `option:usage.rates.*.above.*.cache_write_per_mtok` | Credits per million cache-write tokens. |
| `option:usage.rates.*.above.*.cache_write_1h_per_mtok` | The one-hour cache-write quote above this context size, in credits per million tokens. An absent quote stays unknown. |
| `option:usage.rates.*.above.*.cache_write_12h_per_mtok` | The twelve-hour cache-write quote above this context size, in credits per million tokens. When absent, the root quote applies; without either quote the price stays unknown. |
| `setting:usage.catalogue` | **Estimates** — Whether use that has no rate is estimated from the rate catalogue that ships with Orch. |
| `setting:usage.catalogue_scale` | **Catalogue scale** — What one unit of the rate catalogue is worth in this place's credits. |
| `setting:usage.catalogue_url` | **Catalogue address** — An https address from which the system may fetch a newer rate catalogue. |
| `setting:usage.catalogue_refresh_days` | **Catalogue refresh** — How many days pass between the system's automatic fetches of the rate catalogue; 0 fetches only when an admin asks. |
| `setting:chat.enabled` | **Chats** — Whether people may post in the chats of the places here. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:chat.direct` | **Direct and group chats** — Whether the members of a root organization may start direct and group chats with each other. A lower scope can only choose a stricter option. Strictness order: `members` → `off` (strictest last). |
| `setting:chat.group_max_members` | **Group size** — How many people one group chat may have. |
| `setting:chat.group_mentions` | **Role and group mentions** — Who may name role and group audiences among current readers: chat writers, or its current managers for a group chat. A lower scope can only choose a stricter option. Strictness order: `writers` → `managers` (strictest last). |
| `setting:notifications.person_rate_per_hour` | **Person notifications per hour** — How many people one person may explicitly notify in one root organization in a rolling hour; 0 disables the limit. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:notifications.person_note_chars` | **Person notification note length** — How many characters an optional notification note may have; 0 allows no note. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:references.preview_bytes` | **File preview size** — Bytes a reference card reads for a text excerpt. |
| `setting:references.preview_lines` | **File preview context** — Lines shown before and after a referenced line; 0 shows only the referenced line. |
| `setting:files.preview_source_bytes` | **Preview source size** — The largest original decoded for a static file preview. |
| `setting:files.preview_pixels` | **Preview image pixels** — The largest image decoded for a static preview, checked before allocating pixels. |
| `setting:files.preview_edge` | **Preview dimensions** — The longest edge of a static file thumbnail, in pixels. |
| `setting:files.preview_bytes` | **Preview size** — The most encoded bytes in one static thumbnail or text excerpt. |
| `setting:files.preview_cache_bytes` | **Preview cache** — Bytes kept for derived previews; older previews are discarded first, and zero disables caching. |
| `setting:files.viewer_bytes` | **Media viewer size** — The largest original image, PDF, audio or video file read into a browser viewer. |
| `setting:files.native_image_bytes` | **Size of images sent directly** — The largest image sent directly to a model; larger images are delivered by path, and zero always uses paths. |
| `setting:docs.enabled` | **Documents** — Whether new documents may be made beside the places here. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:docs.templates` | **Starting documents** — Current readable ordinary documents offered when creating one here; a template grants no source access or creation permission. |
| `setting:docs.max_bytes` | **Document size** — The most text one document here may hold, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:docs.max_blocks` | **Document blocks** — The most blocks one document here may have. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:docs.versions_kept` | **Versions kept** — How many versions of one document here are kept at most. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:docs.history_max_bytes` | **History size** — How much the kept versions of one document here may take together, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:docs.file_max_bytes` | **File size** — The largest one file a document here may hold, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:docs.files_max_bytes` | **Files together** — The most the files of one document here may come to together, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:documents.links.shown` | **Links tabs** — The tabs of a document's Links section that its readers see. A document's owner can switch tabs off for one document; a lock here decides for the whole place. |
| `setting:documents.links.page_size` | **Links page size** — Rows shown in a Links list before Show more. |
| `setting:documents.links.occurrences` | **Mentions per source** — Mentions listed under one source in Referenced by before Show more. |
| `setting:documents.links.line_chars` | **Mention line length** — Characters of the line shown around each mention in Referenced by. |
| `setting:documents.links.mentions_counted` | **Mentions counted** — Mentions of the document counted in one source before its count reads “or more”. |
| `setting:documents.links.access_checks` | **Links access checks** — Sources whose reading access is checked for one page of Referenced by. A very popular document continues on the next pages. |
| `setting:documents.links.graph_nodes` | **Graph nodes** — The most nodes the Links graph draws. |
| `setting:documents.readme_blocks` | **README preview** — Top-level blocks shown in a place's README preview, with their children. An earlier page break ends the preview; Open document always reaches the whole page. |
| `setting:chat.attachment_max_bytes` | **Attachment size** — The largest file attached to a chat message, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:search.text_chars` | **Conversation search text** — Characters indexed per conversation item after masking. Changes apply to new text and a search rebuild; raw calls, output, hidden reasoning, side threads and text with unavailable masking inputs are excluded. |
| `setting:search.word_recent_candidates` | **Recent word sample** — Recent rows sampled before probing the text index. |
| `setting:search.word_candidates` | **Word candidates** — Candidates per indexed field before exact access and ranking. |
| `setting:search.word_probe_candidates` | **Word index probe** — Indexed matches sampled before switching to a recency scan. |
| `setting:search.word_tuning` | **Word retrieval** — Estimated rows below which word search uses scoped reads. Missing field limits use Word candidates. Windows per request defaults to one; Show more continues. |
| `option:search.word_tuning.scope_candidates` | Rows inspected inside the authorized search scope. |
| `option:search.word_tuning.request_windows` | Search request window limits. |
| `option:search.word_tuning.tiers` | Rates or limits by the declared tier. |
| `option:search.word_tuning.tiers.title` | Title text or its ranking weight. |
| `option:search.word_tuning.tiers.description` | Description text or its ranking weight. |
| `option:search.word_tuning.tiers.heading` | Heading text or its ranking weight. |
| `option:search.word_tuning.tiers.body` | Body text or its ranking weight. |
| `setting:search.page_size` | **Page size** — Results shown before Show more. |
| `setting:navigation.tree_page_size` | **Hierarchy page size** — Default rows in a hierarchy branch page, initially 50. A caller may request a larger page; this is a default, not a cap. |
| `setting:search.reorder_window_secs` | **Reorder window** — How long new results may reorder the first screen. |
| `setting:search.meaning_pause_ms` | **Typing pause** — How long typing pauses before an embeddings request, in milliseconds. |
| `setting:search.cursor_entries` | **Saved searches** — Continuations kept for each reader. |
| `setting:search.cursor_ttl_secs` | **Search lifetime** — How long a search can still load more results. |
| `setting:search.vector_cache_entries` | **Query cache size** — Query vectors cached per organization; zero disables caching. |
| `setting:search.vector_cache_secs` | **Vector cache** — How long an organization reuses a query vector for the same model. |
| `setting:search.semantic.enabled` | **Semantic search** — Also find results with a similar meaning, using the embeddings below. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:related.weights` | **Related weights** — Weights from zero to one for meaning, place and references, as decimal text. Zero leaves a signal out. |
| `option:related.weights.meaning` | Weight of similarity in meaning. |
| `option:related.weights.place` | Weight of proximity in the place tree. |
| `option:related.weights.references` | Weight of explicit references. |
| `setting:related.place_scores` | **Related places** — Scores from zero to one for the same folder, topic, project or organization. |
| `option:related.place_scores.folder` | Score for a shared document folder. |
| `option:related.place_scores.topic` | Score for a shared topic. |
| `option:related.place_scores.project` | Score for a shared project. |
| `option:related.place_scores.org` | Score for a shared organization. |
| `setting:related.count` | **Related items** — Items shown in Related. Zero switches it off. |
| `setting:related.candidates` | **Related candidates** — Candidates per signal and reference neighborhood before exact reading-access checks. |
| `setting:search.semantic.embeddings` | **Embeddings** — Provider and model that compute embeddings; their use is counted to whoever names them. |
| `option:search.semantic.embeddings.provider_id` | The selected provider identity. |
| `option:search.semantic.embeddings.model_name` | The selected model routing name. |
| `option:search.semantic.embeddings.dimensions` | Vector dimensions for this index. |
| `option:search.semantic.embeddings.query_prefix` | Text placed before a search query for embedding. |
| `option:search.semantic.embeddings.document_prefix` | Text placed before document text for embedding. |
| `setting:search.semantic.max_disk_share` | **Disk limit** — Index size limit as a share of the indexed content's size; indexing pauses at the limit. Empty: no limit. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:search.semantic.tuning` | **Indexing and ranking** — How text is split, embedded and ranked. |
| `option:search.semantic.tuning.chunk_tokens` | Target tokens per indexed text chunk. |
| `option:search.semantic.tuning.batch_tokens` | Maximum tokens in one indexing batch. |
| `option:search.semantic.tuning.candidates` | Candidate matches considered by ranking. |
| `option:search.semantic.tuning.oversample` | Extra vector candidates to consider. |
| `option:search.semantic.tuning.fusion_k` | Rank-fusion weighting constant. |
| `option:search.semantic.tuning.same_model_cosine_milli` | Same-model similarity threshold in thousandths. |
| `option:search.semantic.tuning.interval_secs` | Scheduled interval in seconds. |
| `option:search.semantic.tuning.exact_filter_candidates` | Filtered candidates considered by exact search. |
| `option:search.semantic.tuning.source_batch` | Source records read per indexing batch. |
| `option:search.semantic.tuning.max_scan_tuples` | Maximum index tuples visited by the scan. |
| `option:search.semantic.tuning.scan_mem_multiplier` | Memory multiplier for the vector scan. |
| `option:search.semantic.tuning.ef_search` | Breadth of the graph search. |
| `option:search.semantic.tuning.iterative_scan` | How the vector scan continues for more candidates. |
| `option:search.semantic.tuning.query_wait_secs` | How long a query waits for matching index work, in seconds. |
| `setting:retention.archive_idle_days` | **Archive idle topics** — Archive a topic after this many days without activity; 0 never does. |
| `setting:retention.delete_archived_days` | **Delete archived places** — Delete a place this many days after it was archived; 0 never does. |
| `setting:retention.agent_history_idle_days` | **Agent history kept** — Remove a thread's agent history after this many days without a turn; 0 never does. |
| `setting:retention.inbox_days` | **Inbox kept** — For how many days a notification stays in a person's Inbox, read or not, before it is removed. |
| `setting:retention.deleted_keep_days` | **Deleted items kept** — Days a deleted item can be restored. At zero, the next purge removes it. |
| `setting:retention.blob_grace_hours` | **Unreferenced files kept** — Hours to keep file content after its last reference is removed. Complete a backup’s blob copy within this window from the start of its dump. |
| `setting:redaction.mask_secrets` | **Mask secrets** — Whether the values of the secrets a thread was given are masked in stored text. A lower scope can require this protection; it cannot switch off protection required above. |
| `setting:redaction.patterns` | **Patterns** — Further patterns to mask in stored text, each with a name. |
| `option:redaction.patterns.*.name` | A readable name for the declared item. |
| `option:redaction.patterns.*.regex` | Regular expression used by the rule. |
| `setting:diagnostics.retention_days` | **Diagnostics kept** — For how many days diagnostics are kept. |
| `setting:diagnostics.frame_bytes` | **Whole protocol frames** — Masked protocol bytes kept whole per frame; defaults to 65536. Larger frames retain a size note and the configured prefix; 0 keeps no complete frames. Applied capture limits stay with their recorded decision. |
| `setting:diagnostics.frame_head_bytes` | **Protocol frame prefix** — Masked bytes retained from an oversized frame's start; defaults to 4096. 0 keeps its size note only. |
| `setting:diagnostics.frames_per_segment` | **Protocol frames kept** — Newest captured frames retained per native session; defaults to 20000. 0 switches capture off. Omission counts and reasons survive body cleanup; missing older facts stay Not reported. |
| `setting:database.retry_attempts` | **Database repeats** — Transient repeats per recovery burst, default 8; 0 skips immediate repeats. Data and program-limit refusals are never repeated. |
| `setting:database.retry_initial_ms` | **Database repeat pause** — Initial transient retry pause in milliseconds, default 100. It doubles within the bounded burst. |
| `setting:database.recovery_secs` | **Database recovery interval** — Cooldown after a burst and interval for counted failure summaries, default 60 seconds. Pending work remains retained; transactions are released before waiting. |
| `setting:orgs.personal_enabled` | **Create personal organizations** — Give new accounts a personal organization. Disabling this keeps existing organizations; people can still join organizations and receive document shares. |
| `setting:orgs.personal_closed` | **Personal orgs closed** — Whether a person's own organization is created admitting no member but that person. |
| `setting:orgs.personal_flat` | **Personal orgs flat** — Whether a person's own organization is created holding no department inside it. |
| `setting:orgs.created_by` | **Who may make an org** — Who may make a team organization: anybody signed in, system admins only, or somebody a system admin invited to make one, each invitation used once. Personal organization creation is a separate policy. |
| `setting:search.history_candidate_batches` | **History search work per page** — Default 4 projected native batches; any positive count. A work boundary yields native continuation, never a total result cap. |
| `setting:search.name_candidate_batches` | **Name search work per page** — Default 4 batches per native name page; any positive count. More candidates always have a continuation, including when hidden matches fill a batch. |
| `setting:ui.picker_recent_count` | **Recent picker choices** — Default 5 entries per person and kind, kept privately on the server across screens and devices. Any nonnegative count; zero disables remembering. Search still offers other entries. |
| `setting:navigation.order` | **Order of the tree** — The order in which the organization's tree is shown to whoever follows the organization's own: as somebody arranged it, the newest first, the most recently changed first, or by name. An owner or admin of the organization saves it from the order they see. |
| `option:navigation.order.by` | The recorded author, or the declared sort order for navigation. |
| `option:navigation.order.custom` | Explicit per-parent navigation order. |
| `setting:registration.mode` | **Registration** — Who may register: nobody, only somebody with an invitation, or anybody who reaches the system. |
| `setting:org.terms` | **Terms for people who join** — Markdown terms for joining this organization or accepting a guest invitation to its places or chats. Root terms come first, followed by each nearer organization’s terms. Empty text adds no terms; existing members are not asked to accept later edits. |
| `setting:invitations.forms` | **Invitation forms** — The forms of invitation that may be made for an organization of this tree: a link to share, a personal link for one address, or both. A lower scope may remove allowed choices; it cannot add one excluded above. |
| `setting:invitations.default_minutes` | **Invitation life** — For how many minutes an invitation is good when its inviter says nothing. |
| `setting:invitations.max_minutes` | **Longest invitation** — The most minutes an invitation may be good for. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:invitations.max_uses` | **Uses of a shared link** — How many people one shared link may bring in at most. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:mail.shared` | **Shared mail server** — Whether a root organization that has no mail server of its own sends its mail through the system's. |
| `setting:api.idempotency_window` | **Creation replay window** — How many hours Orch remembers a creation request, so that sending it again returns the first result instead of creating a second one; 0 turns this off. The default is 24 hours and can be raised. |
| `setting:api.discovery_cache_idle_seconds` | **Expire unused discovery catalogues** — Default 300 idle seconds; active native-session paging renews the captured source. 0 switches idle expiry off. No session count is cropped. |
| `setting:api.discovery_cache_bytes` | **Discovery cache target** — Default 67,108,864 bytes. Older captures leave first; one larger capture is kept whole on its own. 0 switches budget eviction off. An expired source is refreshed by its list. |
| `setting:api.read_repeats` | **Repeated reads** — How many times a read is made again, in place of being refused, when what it showed has moved while it was being answered; 0 makes no repeat. |
| `setting:interface.kept_closed_tabs` | **Recently closed tabs** — Ten recently closed views, their Reopen history and presentation keep local state by default; open views always keep theirs. Zero keeps one Reopen locator but no closed body or presentation. Choose another count in Interface. |
| `setting:interface.recent_models` | **Recent model choices** — Five suitable choices appear in each person's Recent model group by default. Set any nonnegative count; zero hides the group while retaining private server history. Unavailable and differently purposed entries never consume a Recent place. |
| `setting:interface.progress_delay_ms` | **Loading indication delay** — A read shows its spinner after 200 milliseconds by default. Zero shows it immediately. Pending controls prevent a duplicate act throughout the wait. |
| `setting:interface.catch_up_above_items` | **Automatic catch-up threshold** — Show the automatic card above 20 unread items by default. Choose another nonnegative count in Interface; zero includes any unread range. The New divider’s Details shows the current value and its source. Explicit catch-up stays available below the threshold, and opening Details sends no summary request. |
| `setting:interface.read_freshness_ms` | **Read freshness** — Returning readers reuse directory answers for 60000 ms, lookups for 15000 ms, reports for 60000 ms and status for 5000 ms by default. Zero revalidates on every return. Release catalogues remain for the page. |
| `option:interface.read_freshness_ms.directory` | Freshness in milliseconds for directory contents and shared-path listings. |
| `option:interface.read_freshness_ms.lookup` | Freshness in milliseconds for repeated name and people searches. |
| `option:interface.read_freshness_ms.report` | Freshness in milliseconds for reports and public installation facts. |
| `option:interface.read_freshness_ms.status` | Freshness in milliseconds for live health and usage status. |
| `setting:edge.transport` | **HTTPS transport** — Built-in HTTPS edge defaults; committed changes reload gracefully. External TLS fronts keep their own settings. |
| `option:edge.transport.session_cache_mib` | Shared TLS session cache in MiB, default 16; 0 turns the cache off. Positive whole MiB values satisfy nginx’s shared-memory representation. No upper cap. |
| `option:edge.transport.session_timeout_secs` | Positive resumption lifetime in seconds, default 86400. Applies to both cached sessions and tickets. |
| `option:edge.transport.session_tickets` | TLS session tickets, on by default. With its shared cache enabled, nginx rotates ticket keys across workers; no private ticket key is exposed through settings. |
| `option:edge.transport.early_data` | TLS early data, on by default for safe reads. Every early write receives 425 before authentication or business effects and can be repeated after the handshake. |
| `option:edge.transport.http2` | HTTP/2 negotiation on the built-in TLS address, on by default. |
| `option:edge.transport.http3` | HTTP/3 over QUIC beside HTTP/2, on by default. The HTTPS overlay publishes TCP and UDP at the same port; an external firewall must also pass UDP. |
| `option:edge.transport.alt_svc_secs` | HTTP/3 advertisement lifetime in seconds, default 86400; 0 expires immediately. Disabling HTTP/3 clears a previous advertisement. |
| `option:edge.transport.reload_secs` | Positive interval between local certificate/policy file checks and retrying a failed publisher, default 1 second. Idle checks read no database and make no network request. |
| `setting:interface.connection_quality` | **Connection warning** — Compare this person’s recent median with their preceding usual median. Defaults: 3 recent replies, 20 usual replies, a 100% increase and 200 ms extra. Warn only when both margins are exceeded; a stable 700 ms line is normal. Windows and thresholds have no fixed upper cap. |
| `option:interface.connection_quality.recent_samples` | Positive count of recent ping replies used for the current median. Authentication time is never a ping sample. |
| `option:interface.connection_quality.usual_samples` | Positive count of preceding replies used for this person’s usual. A sustained route change gradually becomes the new usual; a replacement reader starts fresh. |
| `option:interface.connection_quality.slow_increase_percent` | Required percentage increase above the usual, zero or more. Zero removes the relative margin; the extra-time margin still applies. |
| `option:interface.connection_quality.slow_extra_ms` | Required extra milliseconds above the usual, zero or more. Zero removes the noise margin; the percentage margin still applies. |
| `setting:release.check_days` | **Look for a release** — Days between read-only checks of the operator's configured release source; 0 is off. A check sends no installation statistics and installs nothing. |
| `setting:cleanup.release_keep_count` | **Keep unused releases** — Unused releases kept per payload after an update; 0 keeps all. Current and previous releases are always kept. |
| `setting:cleanup.unused_keep_days` | **Clean unused caches** — Days to keep images and tool versions that nothing uses; 0 switches automatic cleanup off. Data follows its own retention dates. |
| `setting:tunnels.control_timeout_ms` | **Tool admission wait** — 5000 milliseconds by default for each Open or Resume to confirm the current physical session and tool member. Positive, with no fixed maximum beyond the native clock; timeout refuses the channel. |
| `setting:backup.warn_days` | **Database backup warning** — Warn when the last successful database-only backup is older than this many days. A failed attempt does not make it newer. |
| `setting:registration.email_verification` | **Email verification** — Whether a person has to confirm their address before they sign in. |
| `setting:auth.local_login` | **Password sign-in** — Whether people other than system admins may sign in with a password or a mailed code. |
| `setting:auth.email_code` | **Code by mail** — What a code sent to a person's confirmed address may be when they sign in: a way in by itself, a second step after the password, both or neither. A lower scope may remove allowed choices; it cannot add one excluded above. |
| `setting:auth.second_step` | **Second step** — Whether the people of this organization must set up and give a second proof of identity before they work here. A lower scope can only choose a stricter option. Strictness order: `optional` → `required` (strictest last). |
| `setting:human_check.provider` | **Human check** — Which service asks a person to prove they are not a program: none, hCaptcha, or Google Cloud Fraud Defense (reCAPTCHA). While none is chosen no page loads anything from outside and nothing is sent. |
| `setting:human_check.site_key` | **Site key** — The public key of the service's widget, which the sign-in and registration pages show. |
| `setting:human_check.project_id` | **Google Cloud project** — The identifier of the Google Cloud project that holds the reCAPTCHA key. Only CreateAssessment uses it. |
| `setting:human_check.verification` | **How reCAPTCHA is asked** — SiteVerify takes the site key and secret key that Google's console shows, and is what a key made today still gets. CreateAssessment takes a Google Cloud project and an API key restricted to the reCAPTCHA API, and is the way Google recommends. hCaptcha has one way. |
| `setting:human_check.mode` | **Widget** — How hCaptcha shows itself: a checkbox, or unseen, asking only when it doubts. Google Cloud Fraud Defense (reCAPTCHA) always runs unseen, whatever is set here. |
| `setting:human_check.acts` | **Where it asks** — The acts that need a check: signing in (every try, the second step included), asking for a sign-in code, registering, asking for a password reset link, and asking for a confirmation link again. |
| `setting:human_check.min_score` | **Least score** — The lowest score, in hundredths, that passes with Google Cloud Fraud Defense (reCAPTCHA); 50 is Google's own starting point. Without a billing account Google answers only 10, 30, 70 or 90. hCaptcha's score is not used. |
| `setting:human_check.when_unreachable` | **If it cannot be reached** — Whether a person is let in, with the event recorded, when the service cannot be asked, or refused until it can. Let in is the default: the form is the only way in, so an outage must not shut everyone out, and the limits on tries still hold. |
| `setting:human_check.alternate_host` | **Use recaptcha.net** — Load Google Cloud Fraud Defense (reCAPTCHA) from www.recaptcha.net, for people who cannot reach www.google.com. hCaptcha has no alternate address. |
| `setting:human_check.timeout_secs` | **Wait for the service** — How long one call to the service may take before it counts as not reached. Five seconds covers the services' usual answer and does not leave a person waiting at a form. |
| `setting:human_check.page_wait_secs` | **Wait of the page** — How long a sign-in page waits for the service's widget to give a token before it says that the service cannot be reached. Twenty seconds is four times the wait of the server, which leaves room for a challenge a person has to solve. |
| `setting:human_check.address_limit` | **Checks per address** — How many checks one address may ask this server to make in the window below. Thirty is the limit of wrong tries from one address, so that nobody can make this server call the service without end. |
| `setting:human_check.address_window_minutes` | **Window of checks per address** — How long a check counts against its address: fifteen minutes, the window of wrong tries from one address. |
| `setting:human_check.look_secs` | **Time between looks** — How long what the server found of the web edge's policy, and of whether the service answers, is remembered before it looks again. A minute follows an edge that is changed within minutes, and asks the service no more than once a minute when a page says its widget could not load. |
| `setting:tokens.max_days` | **Longest token life** — The longest total life of an API token that is admitted here: from when it was made to when it ends. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:system.banner` | **Banner** — A message shown to everybody on the system, with its level and when it ends. |
| `option:system.banner.text` | Text carried by this rule. |
| `option:system.banner.level` | The requested access level. |
| `option:system.banner.until` | When the rule ceases to apply. |
| `setting:outbound.allowed_private_hosts` | **Private hosts** — The hosts on private addresses that a channel or a destination of an organization or a person may reach. |
| `setting:storage.transfer_initial_piece_bytes` | **First transfer piece** — Bytes in each small initial bandwidth probe, before the line rate is known. |
| `setting:storage.transfer_initial_requests` | **Transfer probes** — Small concurrent probes used to measure bandwidth without counting connection setup as data time, within the shared request and byte ceilings. |
| `setting:storage.transfer_min_piece_bytes` | **Smallest transfer piece** — Smallest ordinary file piece; a final piece may be smaller. |
| `setting:storage.transfer_max_piece_bytes` | **Largest transfer piece** — Maximum bytes in one file piece. Each unfinished upload keeps its accepted policy. |
| `setting:storage.transfer_target_window_ms` | **Transfer data duration** — Target data duration of the shared window, before adding measured round-trip headroom. |
| `setting:storage.transfer_window_rtts` | **Transfer round trips ahead** — Round trips of data kept in flight ahead of acknowledgements, within the byte ceiling. |
| `setting:storage.transfer_max_window_bytes` | **Transfer window** — Maximum in-flight file bytes shared by simultaneous files from one sender. |
| `setting:storage.transfer_requests` | **Transfer requests** — Simultaneous piece requests shared by files. Five leaves one HTTP/1.1 connection for other work. |
| `setting:storage.max_blob_bytes` | **Largest file** — The largest single file that Orch stores, in bytes. |
| `setting:storage.machine_upload_idle_secs` | **Machine transfer retention** — How long an unfinished machine transfer may receive no piece before cleanup is eligible. Each transfer keeps the allowance accepted at its first piece. |
| `setting:storage.env_cache_quota_bytes` | **File cache** — How many bytes of cached file contents are kept for one environment. A lower scope can only reduce this limit; it cannot remove a limit set above. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:storage.relay_transfers` | **File relay** — Relayed by default: browser file content passes through memory and copies carried between machines are removed when the transfer ends. Offline files require a kept snapshot; the listing stays. Cached keeps opened versions up to the file cache size. The nearest system, organization or project choice wins unless the operator locks it. |
| `setting:storage.upload_session_hours` | **Upload idle retention** — Defaults to 24 hours without progress. Accepted upload progress renews its idle lease; it is not a whole-file duration cap. |
| `setting:storage.relay_concurrency` | **File relay concurrency** — Defaults to 32 active transfers. Further transfers wait for memory, without limiting their file size or count. Set at the system. |
| `setting:storage.relay_idle_secs` | **File relay idle wait** — Defaults to 120 seconds without progress before a transfer pauses. Its native acknowledged upload prefix remains resumable. Set at the system. |
| `setting:storage.relay_retry_ms` | **File relay retry interval** — Defaults to 250 milliseconds between transient transport failures within the idle allowance. Set at the system. |
| `setting:agents.page_fetch` | **Page fetch** — Unset keeps the program default. Off disables the capability; Native uses a declared page reader; Service uses the explicitly selected fetch backend. It is independent of Web search. |
| `setting:agents.image_generation` | **Image generation** — Unset keeps Program default. Native uses a supported native image tool, Off disables it, and Service uses the explicitly selected backend with native generation off. A default is not a lock; no paid service is selected implicitly. |
| `setting:gateways.tunnel_strategy` | **Tunnel gateway** — New pairing plans use the lowest combined fresh round trip, or stay on the executor’s gateway. Existing tunnels retain their frozen placement. |
| `setting:threads.agent_schedules` | **Agent schedules** — Let the agent schedule visible future inputs on this thread. Off by default; current permissions and provider allowances apply to every run. |
| `setting:agents.image_generation_backend` | **Image generation backend** — Select an exact image route and enabled provider offering. The choice is inherited through the thread and respects higher locks. Unset selects no paid service; current provider Use and limits still apply when it runs. |
| `option:agents.image_generation_backend.capabilities` | Explicit backend overrides keyed by capability. A null entry clears that capability’s service backend; an absent entry uses only the retained default. The capability’s mode is a separate choice. |
| `option:agents.image_generation_backend.capabilities.*.route` | The declared route for this capability, including a delegated program session where offered. It must carry this capability; naming another capability’s route cannot enable it. |
| `option:agents.image_generation_backend.capabilities.*.offering` | The exact provider and enabled model or service for this override. Each capability keeps its own selection and current Use checks. |
| `option:agents.image_generation_backend.capabilities.*.offering.provider` | The provider account selected for this capability. It may differ from the conversation provider; ordinary Use, sharing policy and allowed lists still apply. |
| `option:agents.image_generation_backend.capabilities.*.offering.offering` | The provider-local name of the selected model or service. Delegation names the model used by the helper; it does not select a sign-in or model automatically. |
| `option:agents.image_generation_backend.capabilities.*.config` | Only additional references documented by this route. Leave empty otherwise; credentials, permissions and usage allowances do not belong in this value. |
| `option:agents.image_generation_backend.default` | The retained backend value for capabilities without an explicit override. Changing one capability preserves this value for the others. An unset default selects no paid service. |
| `option:agents.image_generation_backend.route` | The declared image route, such as image_generation.service-api for an Images API, image_generation.service-model for a hosted model tool, or image_generation.delegation for an eligible native-program helper. An unavailable route stays unavailable; it never chooses another provider implicitly. |
| `option:agents.image_generation_backend.offering` | The exact provider and model or service used by this route. Omit only when that route needs no offering; selection alone grants no access. |
| `option:agents.image_generation_backend.offering.provider` | The provider account for this image call, independently of the conversation’s provider. The driver needs its current Use permission and the offering must fit the applicable allowed lists. |
| `option:agents.image_generation_backend.offering.offering` | The provider-local name of the enabled image service, hosted-tool model or native helper’s model required by the route. Choose the exact offering rather than an account’s default or a price-based fallback. |
| `option:agents.image_generation_backend.config` | Additional references understood by the selected route. Leave empty unless that route documents them; never put provider credentials here. This object cannot grant Use, bypass limits or override current source authority. |
| `setting:agents.web_search_backend` | **Web search backend** — Choose an enabled provider and a model with built-in search, or that provider's search and fetch services. Unset offers no paid service automatically. |
| `option:agents.web_search_backend.route` | The exact declared search or page-fetch route, backed by an enabled model, provider service or delegated session where supported. It never infers another provider. |
| `option:agents.web_search_backend.offering` | The provider and enabled model or service used by this route. Search and page fetch may choose separate offerings through their capability overrides. |
| `option:agents.web_search_backend.offering.provider` | The account supplying this search or page-fetch operation. Its current Use, sharing policy and limits are checked when the operation runs. |
| `option:agents.web_search_backend.offering.offering` | The exact model or service name within that provider. The route must support the requested capability; no default offering or paid fallback is inferred. |
| `option:agents.web_search_backend.config` | Additional route-documented references only. Leave empty when none are needed; never include a provider credential or use this value to bypass its limits. |
| `option:agents.web_search_backend.capabilities` | Independent backend choices keyed by capability, such as web_search and page_fetch. Changing search preserves the page-fetch choice. Null clears only that backend selection; the mode remains separate. |
| `option:agents.web_search_backend.capabilities.*.route` | The declared route serving this one capability. Choose a model-backed, API-service or delegated route only where the provider currently offers it. |
| `option:agents.web_search_backend.capabilities.*.offering` | The exact provider and enabled offering for this one capability; it does not borrow the other capability’s selection. |
| `option:agents.web_search_backend.capabilities.*.offering.provider` | The account for this capability’s backend. The requesting person still needs its current Use under the place’s policies. |
| `option:agents.web_search_backend.capabilities.*.offering.offering` | The provider-local model or service name for this capability. It must match this route’s support and the applying model or service lists. |
| `option:agents.web_search_backend.capabilities.*.config` | Only the selected route’s documented additional references. Credentials stay on the provider, and current source authority and caps remain unchanged. |
| `option:agents.web_search_backend.default` | The explicit retained backend for capabilities with no override, including an existing model or search/fetch service choice. Missing means no service backend is chosen, never automatic paid discovery. |
| `setting:threads.agent_schedule_hours` | **Agent schedule duration** — Default lifetime in hours for recurring agent schedules. An explicit end may be later; this is a default, not a cap. |
| `setting:threads.agent_schedule_runs` | **Agent schedule runs** — Default occurrence count for recurring agent schedules. The agent may choose another positive count; this is a default, not a cap. |
| `setting:summary.targets` | **Summary targets** — Refresh events and input/output defaults by target, off until enabled under the chosen summarizer and shared usage caps. Help and search results use written or existing summaries. |
| `setting:summary.follow_interval_secs` | **Summary follow interval** — Free reads while a summary is eligible or running; future work waits until its due time. Defaults to 5 seconds. |
| `setting:summary.read_repeats` | **Summary read repeats** — Unpaid source rereads and prompt rebuilds before the first model call, default 3 in each loop; 0 makes no repeat. Source movement after a model call never starts a paid retry. |
| `setting:summary.read_batch_size` | **Summary read batch** — Distinct source targets in one stored search-summary window; further targets continue. Defaults to 100. |
| `setting:summary.retry_ceiling_secs` | **Summary retry ceiling** — The longest backoff after failed automatic title or tag attempts. Call rules still decide whether another model call is allowed. Defaults to 3600; a positive value. |
| `setting:summary.goal_turn_items` | **Goal helper turn items** — The most items of the reported turn read for an independent goal check. A larger turn is omitted whole; the condition and evidence remain. Defaults to 2000; a positive value. |
| `setting:summary.goal_reason_chars` | **Goal helper reason characters** — The most characters kept from the independent helper’s explanation, separate from the model’s output token allowance. Defaults to 600; a positive value. |
| `option:agents.web_search_backend.kind` | Choose a model-backed or API-service-backed search backend. |
| `option:agents.web_search_backend.provider_id` | The enabled provider whose search offering is selected. |
| `option:agents.web_search_backend.model_name` | The native-search model on the selected provider, for a model backend. |
| `option:agents.web_search_backend.search_service` | The selected provider’s declared search service, for an API backend. |
| `option:agents.web_search_backend.fetch_service` | An optional declared page-fetch service on the selected provider. |
| `option:summary.targets.thread` | Thread summary options. |
| `option:summary.targets.thread.automatic` | Allow automatic refresh for this target. Off as shipped; explicit Refresh is separate. |
| `option:summary.targets.thread.on` | Events that may trigger automatic refresh. Omission uses this target’s ordinary events; an empty list disables event triggers. |
| `option:summary.targets.thread.min_interval_secs` | Minimum automatic-refresh interval in seconds; zero adds no minimum gap. |
| `option:summary.targets.thread.context_tokens` | Positive input token budget; otherwise the summarizer default is 32,000, within the available model window less output. |
| `option:summary.targets.thread.output_chars` | Positive output character budget. This never overrides provider limits or usage caps. |
| `option:summary.targets.topic` | Topic summary options. |
| `option:summary.targets.topic.automatic` | Allow automatic refresh for this target. Off as shipped; explicit Refresh is separate. |
| `option:summary.targets.topic.on` | Events that may trigger automatic refresh. Omission uses this target’s ordinary events; an empty list disables event triggers. |
| `option:summary.targets.topic.min_interval_secs` | Minimum automatic-refresh interval in seconds; zero adds no minimum gap. |
| `option:summary.targets.topic.context_tokens` | Positive input token budget; otherwise the summarizer default is 32,000, within the available model window less output. |
| `option:summary.targets.topic.output_chars` | Positive output character budget. This never overrides provider limits or usage caps. |
| `option:summary.targets.document` | Document summary options. |
| `option:summary.targets.document.automatic` | Allow automatic refresh for this target. Off as shipped; explicit Refresh is separate. |
| `option:summary.targets.document.on` | Events that may trigger automatic refresh. Omission uses this target’s ordinary events; an empty list disables event triggers. |
| `option:summary.targets.document.min_interval_secs` | Minimum automatic-refresh interval in seconds; zero adds no minimum gap. |
| `option:summary.targets.document.context_tokens` | Positive input token budget; otherwise the summarizer default is 32,000, within the available model window less output. |
| `option:summary.targets.document.output_chars` | Positive output character budget. This never overrides provider limits or usage caps. |
| `option:summary.targets.chat` | Chat catch-up summary options. |
| `option:summary.targets.chat.automatic` | Allow automatic refresh for this target. Off as shipped; explicit Refresh is separate. |
| `option:summary.targets.chat.on` | Events that may trigger automatic refresh. Omission uses this target’s ordinary events; an empty list disables event triggers. |
| `option:summary.targets.chat.min_interval_secs` | Minimum automatic-refresh interval in seconds; zero adds no minimum gap. |
| `option:summary.targets.chat.context_tokens` | Positive input token budget; otherwise the summarizer default is 32,000, within the available model window less output. |
| `option:summary.targets.chat.output_chars` | Positive output character budget. This never overrides provider limits or usage caps. |
| `option:summary.targets.chat_title` | Chat title options; an independent cached subject with an 80-character default. Explicit group names take precedence. |
| `option:summary.targets.chat_title.automatic` | Allow automatic refresh for this target. Off as shipped; explicit Refresh is separate. |
| `option:summary.targets.chat_title.on` | Events that may trigger automatic refresh. Omission uses this target’s ordinary events; an empty list disables event triggers. |
| `option:summary.targets.chat_title.min_interval_secs` | Minimum automatic-refresh interval in seconds; zero adds no minimum gap. |
| `option:summary.targets.chat_title.context_tokens` | Positive input token budget; otherwise the summarizer default is 32,000, within the available model window less output. |
| `option:summary.targets.chat_title.output_chars` | Positive output character budget. This never overrides provider limits or usage caps. |
| `option:summary.targets.help` | Authored Help summaries have no model-generation knobs. |
| `option:summary.targets.search_results` | Existing search-result summaries have no model-generation knobs. |
| `setting:ask.models` | **Ask models** — Ordered dialogue backends and offerings. Child organizations can narrow the entries. The first usable entry is the default. An unset or empty list blocks Ask. |
| `setting:ask.paid_by` | **Who pays for Ask** — The person asking, or the writer of this setting. Usage is always attributed to the person asking. |
| `setting:ask.retention_days` | **Ask history kept** — Days after the last message before permanent deletion, within the root organization's minimum and maximum. |
| `setting:ask.retention_min_days` | **Shortest Ask retention** — The minimum history retention chosen by administrators of the conversation's root organization. A lower scope can only raise this minimum. |
| `setting:ask.retention_max_days` | **Longest Ask retention** — The maximum history retention allowed by administrators of the conversation's root organization. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:ask.max_steps` | **Ask steps per turn** — The model steps allowed for one question before it asks the person to continue. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:ask.max_output_tokens` | **Ask answer tokens** — The most output tokens per Ask model request. The request is fitted to the window less this, and an answer takes half of the window at most. The lowest system or organization value applies. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:ask.context_tokens` | **Ask context tokens** — Unset uses the chosen model's context window; a model that states none is not given a small one. When set, the smaller value applies to Ask only. A window no larger than twice the answer tokens leaves the answer half of it. The lowest system or organization value applies. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:ask.max_tools` | **Ask tools per turn** — The read operations allowed for one question before it asks the person to narrow the question. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:ask.docs_hits` | **Ask guide pages per search** — The most guide pages one Ask search of the help gives the model, English and Chinese alternating where both match. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:ask.docs_excerpt_tokens` | **Ask guide excerpt size** — The most tokens of one guide page that Ask's help search gives the model; the page's link comes with it. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:ask.max_operations` | **Ask operations per search** — The most read operations one Ask search of the read catalogue returns. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:ask.max_links` | **Ask links per source** — The most canonical source links Ask keeps from one read. **Inherited ceiling.** A lower level may set the same or a smaller limit. |
| `setting:providers.model_groups_max` | **Model groups** — The maximum number of named groups per model. The default is 8 and can be raised; 0 allows no new group memberships. Existing memberships are kept. |
| `option:usage.rates.*.web_fetch_per_call` | Credits per successful page-fetch call. |
| `setting:providers.translation` | **Model translation** — Allow selected Responses / Messages conversions. Allowed as shipped; a lower place may refuse and cannot undo a refusal above. |
| `setting:providers.translation_output_tokens` | **Translated output default** — Output allowance when neither a translated request nor its offering supplies one; defaults to 4,096 tokens, configurable at system or organization. |
| `setting:providers.relay_concurrency` | **Public model calls in flight** — Concurrent public model memory exchanges. Later calls queue before their body is buffered; the default is 32. |
| `setting:providers.relay_request_bytes` | **Public model request size** — Buffered bytes for one public model request; defaults to 16 MiB and can be raised. |
| `setting:providers.relay_idle_secs` | **Public model relay silence** — Seconds a response fragment can wait for its reader before the exchange stops; defaults to 300. |
| `setting:providers.relay_retry_ms` | **Public model relay retry** — Milliseconds before retrying a transient native exchange within its captured silence allowance; defaults to 250. |


### Provider operation defaults

| Reference | Meaning |
| --- | --- |
| `setting:runtime.retry_after_secs` | **Runtime retry delay** — Seconds after failed creation before a model call may try again; Start retries immediately. Defaults to 300 seconds; zero removes the delay. |
| `setting:providers.upstream_head_secs` | **Upstream header deadline** — Seconds to wait for model response headers or a whole unstreamed reply; defaults to 600 seconds. |
| `setting:providers.upstream_idle_secs` | **Upstream silence** — Seconds without a model response fragment before the stream is cut; defaults to 300 seconds. |
| `setting:providers.upstream_whole_bytes` | **Whole model reply size** — Bytes retained for a whole model response, discovery page, complete normalized model catalogue or owned generation result; defaults to 16 MiB and can be raised. |


<a id="resource-pools"></a>

## Resource pools

| Key | Default | Effect |
| --- | --- | --- |
| `setting:pools.member_priority` | `0` | Default priority tier; lower tiers first. |
| `setting:pools.member_weight` | `"1"` | Positive decimal weight; equal weights give equal shares. |
| `setting:pools.max_depth` | `3` | Nested depth, counting the outer pool as one; `0` disables pool choices. |
| `setting:pools.read_batch_size` | `100` | Rows per read window; further pages remain available. |

These defaults inherit from system, organization and project to the pool. A pool's own values follow its home and its locks. Pool defaults do not replace the requesting work's access, budget or recovery rules.


| Reference | Default | Meaning |
| --- | --- | --- |
| `setting:pools.load_ceiling_percent` | `90` | Measured ratio selection ceiling; zero is off. It does not change native capacity or impose a host-wide cap. |
| `setting:pools.load_ceiling_count` | `0` | Observed thread/process count ceiling; zero is off. Counts are not percentages. |
| `setting:pools.cooldown_minutes` | `60` | Suppression after qualifying native evidence; zero adds no timed wait. A vendor wait still applies. |
| `setting:pools.failures_before_cooldown` | `1` | Qualifying failures within the pool failure window; local permission and policy refusals do not count. |
| `setting:pools.failure_window_secs` | `300` | Seconds grouping qualifying native failures for the threshold. |
| `setting:pools.recovery_trials` | `1` | Concurrent native recovery attempts per failure scope, fenced to actual command or turn receipts. |
| `setting:pools.attempts` | `null` | Automatic additional member attempts contributed to call recovery; unset tries each eligible leaf once, zero permits none. Explicit caller rules and stricter guards still apply. |
| `setting:pools.choice_detail_days` | `30` | Detailed choice and skip retention in days; zero keeps forever. Essential native receipts and active health remain. |
