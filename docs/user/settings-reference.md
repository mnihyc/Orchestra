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
| `setting:calls.rules` | **Call recovery** — Ordered rules for retrying, continuing, failing over or waiting after a model call fails. |
| `setting:calls.repeat_limit` | **Repeat limit** — Automatic recoveries of one failure kind within the repeat window; zero stops automatic recovery. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:calls.repeat_window` | **Repeat window** — The rolling time window used to stop repeated recovery. |
| `setting:calls.notify_on_stop` | **Notify on stop** — Tell the thread owner once when the repeat guard stops recovery. |
| `setting:calls.return` | **After failover** — Return to the thread’s own choice at the next turn when ready, or stay on the candidate. |
| `setting:agent.model` | **Model** — The model these threads use. Enter the model ID supplied by its provider. |
| `setting:agent.effort` | **Effort** — The reasoning effort asked of the model, one of those the model lists. |
| `setting:agent.params` | **Agent parameters** — Values for the parameters that the agent's adapter declares, by parameter. |
| `setting:agent.features` | **Agent features** — Which optional features of agents are switched on, by feature. |
| `setting:agent.instructions` | **Instructions** — Standing instructions given to the agent with every session; each scope adds its own under those above it. |
| `setting:agent.memory` | **Agent notes** — Whether an agent keeps notes between sessions: through any available method, through its own program, or not at all. |
| `setting:agents.harness_versions` | **Agent program versions** — The build each agent program uses in new or updated units, inherited through the project and overridden by the environment. An exact agent profile pin wins; changing this does not replace a running unit. |
| `option:agents.harness_versions.*.use` | Program build selection policy. |
| `option:agents.harness_versions.*.version` | The exact selected program version. |
| `setting:agents.harness_channel_hours` | **Release check interval** — How often to read release channels when a place requests newer builds; 0 keeps scheduled reads off. |
| `setting:agents.harness_interface_timeout_secs` | **Interface check timeout** — How long a build may take to print its interface. |
| `setting:agents.harness_case_timeout_secs` | **Conformance case timeout** — How long each native harness case may take against the scripted model. |
| `setting:agents.harness_conformance_timeout_secs` | **Conformance check timeout** — How long the complete native harness check may take in its isolated unit. |
| `setting:agents.harness_trust_mirror` | **Trust mirror checksums** — Allow the release mirror to publish executable checksums of its own. These builds are labelled as mirror checksums, not vendor checksums. |
| `setting:agents.harness_mirror` | **Release mirror** — A server that serves the vendors' release files where this server cannot reach them. |
| `setting:agents.prompt_cache_ttl` | **Prompt cache lifetime** — An explicit cache lifetime for the agent program's main conversation. Unset uses the program's own default; programs without this control ignore it. |
| `setting:agents.subagents_at_once` | **Sub-agents at once** — How many sub-agents the agent program may run at once. The smallest value at this place or above applies. An unsupported control says why it does not apply. |
| `setting:agents.subagent_depth` | **Sub-agent depth** — How many layers of sub-agents may start; 1 allows no nesting. The smallest value at this place or above applies. An unsupported control says why it does not apply. |
| `setting:agents.skills` | **Skills** — Which skills the agents of threads here are given, by skill: off, the latest version or a pinned version; a lower place may change or switch off an inherited one. |
| `setting:agents.personal_skills` | **Personal skills** — Whether the agent of a thread is also given the own skills of the person who started it. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:thread.cwd` | **Start directory** — Where the agent starts inside the thread's directory, as a relative path. |
| `setting:thread.side_threads` | **Side threads** — Whether side threads may be started from the threads here. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:thread.side_max` | **Side threads at once** — How many side threads of one thread may be open at once. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:thread.side_idle_hours` | **Side thread idle time** — After how many hours without a turn an open side thread is closed. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:thread.inputs_without_use` | **Writers without use** — What happens to a message for the agent from somebody who may write here but may not use the thread's model or its environment. A lower scope can only choose a stricter option. Strictness order: `allowed` → `held` → `refused` (strictest last). |
| `setting:thread.switch_history` | **History on a switch** — What a switch or a fork carries of the conversation unless its plan says otherwise. |
| `setting:thread.switch_carry_share` | **Carried context** — How much of the new model's context window a switch may fill with the conversation it carries. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:thread.native_fork_check_bytes` | **Native fork check size** — The most a conversation may take for a fork at one of its turns to be checked against the agent program's own record, so that the program can make the fork itself. A longer conversation is carried as the newest turns that fit. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:thread.read_turn_bytes` | **Turn read size** — The most the messages of one turn may take, written out in full, for Orch to read that turn into a summary of the conversation or a goal check; a larger turn is named and left out, and nothing of it is read. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:thread.plan_first` | **Plan first** — Whether the composer's Plan switch starts on for a thread's first message. The approval mode stays as it is. |
| `setting:thread.plan_answered_by` | **Who answers plans** — Who may proceed with a plan an agent proposes, or keep it planning. A lower scope can only choose a stricter option. Strictness order: `writers` → `managers` (strictest last). |
| `setting:thread.goal.max_turns` | **Goal turns** — The most turns a goal may use here; 0 is no limit. A goal may set a stricter own limit; it cannot raise an ancestor limit. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:thread.goal.max_minutes` | **Goal minutes** — The most minutes a goal may be active here; 0 is no limit. It is looked at between turns: a running turn is not cut short. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:thread.goal.max_credits` | **Goal credits** — The most credits a goal may use here, written as a decimal number; 0 is no limit. Usage caps hold as always. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:approval.mode` | **Approval mode** — When an agent has to ask before it acts: always, when it asks by itself, or never. |
| `setting:approval.allowed_modes` | **Allowed approval modes** — The approval modes that may be chosen here. |
| `setting:sandbox.mode` | **Tool access** — What the agent program lets commands do: read files, change workspace files, or use everything the environment allows. |
| `setting:agent.messages` | **Messages to agents** — Texts that replace what Orch itself says to agents, by message. |
| `setting:models.allowed` | **Allowed models** — Patterns naming the models that may be used here at all; a model has to be named by every scope that sets a list. |
| `setting:services.allowed` | **Allowed services** — Patterns naming enabled provider services that may be used here; every scope that sets a list must admit the service. |
| `setting:providers.allowed` | **Allowed providers** — Patterns over providers' Orch addresses; every scope that sets a list must admit the provider used for work here. |
| `setting:collab.attribution` | **Attribution** — Whether Orch's composed input, stop, denial and forwarded text names people: auto once a second person's words are delivered, always, or never (a person/an approver, with source, time and notes kept). Authored chats and documents retain their writers. |
| `setting:collab.default_delivery` | **Default delivery** — What an input does when a turn is running, unless its sender says otherwise: wait for the next turn, or steer the running one. |
| `setting:composer.paste_inline_context_share` | **Pasted text in the message** — The default share of remaining context for pasted text. 0 always sends it as a file; 100 always sends it in the message. The sender can choose for each paste. |
| `setting:env.thread_default` | **New threads** — Whether a new thread joins the resolved default environment or is given one of its own. |
| `setting:env.default_id` | **Default environment** — The environment new threads join when they share one and ask for no other. The creator must manage it. Clear the choice to inherit the place above. |
| `setting:env.update` | **Updates** — Whether an environment that is behind waits for somebody to update it, or is updated whenever it starts. |
| `setting:env.template_id` | **Template** — The template that a managed environment is made from. |
| `setting:env.host_id` | **Host** — The host that a managed environment is made on; unset, the one with the most room. |
| `setting:env.runtime` | **Runtime** — Choose a runtime the host offers. Leave unset to use the template's default. |
| `setting:environments.runtimes` | **Runtimes** — Allow, forbid or require runtimes by name. Use * for every runtime not named. |
| `setting:env.storage` | **Storage** — Named machine storages for new workspace, home and connector state data. Existing data keeps its recorded location. |
| `option:env.storage.workspace` | Working files storage. |
| `option:env.storage.home` | Agent home storage. |
| `option:env.storage.state` | Recorded file-state storage. |
| `setting:env.data_sources` | **Workspace and home** — Where each data root lives: made by Orch or an existing directory allowed by this machine's environment.roots. Existing directories keep their owner and mode, have no volume size limit and are never removed by a purge. |
| `option:env.data_sources.workspace` | Working files storage. |
| `option:env.data_sources.workspace.kind` | The declared source or content kind. |
| `option:env.data_sources.workspace.path` | The path on the machine that owns this setting. |
| `option:env.data_sources.home` | Agent home storage. |
| `option:env.data_sources.home.kind` | The declared source or content kind. |
| `option:env.data_sources.home.path` | The path on the machine that owns this setting. |
| `setting:env.after_up` | **Commands after start** — Named argv commands run inside a managed environment after every start; null removes an inherited name and a failure keeps the start closed. |
| `option:env.after_up.*.command` | Executable and arguments, run directly without a shell. |
| `option:env.after_up.*.timeout_secs` | Command timeout in seconds. |
| `setting:env.mounts` | **Mounts** — The mounts attached beside the workspace, each as the mount says, off, read-only, or read-only with the environment's own changes kept apart. |
| `setting:env.resources` | **Limits** — The limits of an environment's unit in place of its template's: processor, memory, processes and disk. |
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
| `setting:network.allowed_hosts` | **Allowed hosts** — Exact other host names a restricted unit may reach; lists intersect and an empty list allows only Orch's own transport. |
| `setting:mcp.enabled` | **MCP servers** — Which MCP servers are switched on, by server; a lower scope may switch an inherited one off. |
| `setting:exec.version_overrides` | **Any agent version** — Machine program paths and versions that an environment manager allows with any agent version. A changed path or version needs approval again. |
| `option:exec.version_overrides.*.path` | The path on the machine that owns this setting. |
| `option:exec.version_overrides.*.version` | The exact selected program version. |
| `option:exec.version_overrides.*.by` | The recorded author, or the declared sort order for navigation. |
| `option:exec.version_overrides.*.at` | When the decision was recorded. |
| `setting:exec.offers` | **Exec offers** — How an agent running with its provider may run tools in this environment, in the order tried. |
| `setting:exec.orch_tools` | **Orch's own tools** — Whether the agents that work in this environment are offered Orch's own tools. |
| `setting:orch_tools.enabled` | **Orch tools** — Which of Orch's own tools an agent is offered. |
| `setting:orch_tools.owner_reads` | **Personal read tools** — Off as shipped. Let the thread use its owner’s current read access, including when shared. Its audience banner shows who can see or act and warns that personal reads are visible to them. Sharing earlier results still requires informed confirmation. |
| `setting:orch_tools.calls_per_minute` | **Tool calls a minute** — How many calls of Orch's tools one thread's agent may make in a minute. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:orch_tools.search_query_chars` | **Shared search query budget** — The query character budget advertised to this thread's agent and used by shared search. Defaults to 1000; increase it for longer queries. |
| `setting:orch_tools.search_results` | **Shared search result budget** — The result budget advertised to this thread's agent and used by shared search. Defaults to 200; byte and time budgets still produce a continuation. |
| `setting:orch_tools.search_context` | **Shared search context budget** — The surrounding-line budget advertised to this thread's agent and used by shared-search snippets. Defaults to 5; increase it for wider context. |
| `setting:threads.offline_deadline` | **Offline turn deadline** — End the active turn after this many offline seconds; none keeps waiting. Each outage retains the setting it began with. |
| `setting:threads.background_wake` | **Background wake** — Wake when background tasks or subagents finish. Off keeps Codex results for the next input. Claude Code background tasks are not offered and commands run in the foreground. Changes apply to the next session. |
| `setting:threads.agent_access` | **Agent access** — The initial reach and area levels shown when creating a thread. The creator confirms what they may give. Applies to new threads, without rewriting existing grants. |
| `option:threads.agent_access.where` | The area to which the agent contribution applies. |
| `option:threads.agent_access.parts` | Rights on individual parts of the chosen area. |
| `option:threads.agent_access.parts.chat` | Rights on the discussion. |
| `option:threads.agent_access.parts.documents` | Rights on documents. |
| `option:threads.agent_access.parts.timeline` | Rights on thread history. |
| `option:threads.agent_access.parts.thread_fork` | Rights to fork threads. |
| `option:threads.agent_access.parts.thread_archive` | Rights to archive threads. |
| `setting:threads.let_viewers_in` | **Let thread viewers in** — The initial choice to let a new thread’s viewers use its environment. Confirmed when the thread is created; changing this default grants no access by itself. |
| `setting:broadcast.chat_messages_per_hour` | **Agent chat messages per hour** — Successful posts across all chats from one origin in a rolling hour; zero forbids posting. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:sharing.snapshot_max_bytes` | **Largest copy** — The largest copy that may be shared with an agent in one request, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:export.max_bytes` | **Largest archive** — The largest archive one export of what lies here may come to, in bytes; one that would pass it is not begun, and says so with its size. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:export.max_items` | **Most things in an archive** — How many conversations, chats and documents one export of what lies here may hold. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:export.keep_hours` | **Archives are kept** — For how many hours a finished archive is kept for its person to fetch before it is removed. |
| `setting:import.max_bytes` | **Import size limit** — The expanded bytes allowed in one import. When unset, the place's export size limit applies. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:import.max_items` | **Import item limit** — The objects allowed in one import. When unset, the place's export item limit applies. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:workspace.ignore` | **Ignored paths** — Patterns, written as in a gitignore file, for what the index of a workspace leaves out. |
| `setting:workspace.index` | **Index** — Whether the whole tree of a workspace is indexed for use while it is offline, or only listed while its environment is online. |
| `setting:source.repository_url` | **Repository** — The repository prepared only when the thread's directory is missing; an existing directory keeps its own files. |
| `setting:source.branch` | **Source branch** — The branch to prepare; unset, use the repository's advertised default branch. |
| `setting:source.secret_name` | **Source secret** — The name of an existing HTTPS Git reading secret; its value is sealed only to the preparation command. |
| `setting:workspace.record_days` | **Keep turn files** — Days to keep complete turn and restore undo files in the environment; zero disables recording. |
| `setting:workspace.record_max_bytes` | **Largest recorded directory** — The included file and link bytes eligible for whole-state recording; larger directories use per-file undo. |
| `setting:broadcast.delivery` | **Announcement delivery** — How an announcement reaches a thread whose turn is running: it interrupts, steers, or waits for the next turn. |
| `setting:broadcast.fallback` | **When steering is impossible** — What is done instead when an announcement is to steer an agent that cannot be steered. |
| `setting:broadcast.wake_idle` | **Wake idle threads** — Whether an announcement starts a turn on a thread that is idle. |
| `setting:broadcast.agent_options` | **Agents' choices** — What an agent may choose for one announcement in place of the values set here. A lower scope may remove allowed choices; it cannot add one excluded above. |
| `setting:terminal.enabled` | **Shells** — Whether interactive shells may be opened. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:terminal.record` | **Keep the last output** — Whether the last of what a shell printed is kept in its environment's history when the shell closes. |
| `setting:terminal.detached_minutes` | **Detached time** — For how many minutes a shell keeps running with nobody attached before it is closed. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:terminal.max_per_person` | **Shells per person** — How many shells one person may have open at once in one environment. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:terminal.max_per_environment` | **Shells per environment** — How many shells may be open at once in one environment, everybody's together. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:browser.enabled` | **Browsers** — Whether the environments here show their browsers, start one when asked, and offer their agents the tool for them. Off, an agent may still run a browser as it runs any program. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:ports.enabled` | **Ports** — Whether current door holders may use owned TCP and UDP ports through Orch. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:ports.max_open` | **Wider ports** — The distinct port numbers with wider or pending audiences; zero leaves own use available. |
| `setting:ports.link_max_secs` | **Link lifetime** — The longest link lifetime from creation, including later expiry changes. |
| `setting:requests.expiry_hours` | **Pending agent requests** — The longest pending boundary decision an agent requests; an earlier requested port-link lapse also ends it. |
| `setting:browser.preset` | **Browser preset** — How a browser is started where nobody says: headless, with no window and no display, or with a window on a display that Orch opens for it. |
| `setting:browser.args` | **Browser arguments** — Arguments added to those a browser is started with; the ones Orch gives itself, the profile's directory and the address it is driven at, cannot be set here. |
| `setting:browser.viewport` | **Browser window size** — The size of a new browser's window, written width x height, from 320x240 to 3840x2160. |
| `setting:browser.idle_minutes` | **Browser idle close** — Close a browser that Orch started once it has been idle this many minutes: nobody watched or drove it and no agent's tool named it; 0 never does. |
| `setting:browser.max_per_environment` | **Browsers per environment** — How many browsers that Orch started may run at once in one environment. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:vnc.enabled` | **Displays** — Whether the environments here show their displays, open one when asked, and offer their agents the tool for them. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:vnc.geometry` | **Display size** — The size of a display that Orch opens, written width x height, from 320x240 to 3840x2160. |
| `setting:vnc.max_per_environment` | **Displays per environment** — How many displays that Orch opened may run at once in one environment. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:env.idle_warning_minutes` | **Idle warning** — How many minutes before an idle stop its warning is sent; 0 sends none. |
| `setting:env.removed_keep_days` | **Removed files** — Days to keep owned files after unit removal, bounded by the system retention window; 0 makes them eligible for immediate removal. |
| `setting:env.unused_remove_days` | **Remove when unused** — Schedule the removal of an environment made from a template once no thread has been in it for this many days; 0 never does. |
| `setting:env.lifetime` | **Lifetime** — Keep this environment, remove it after idle time, or also after its creating thread is archived. Only owned files are deleted. |
| `setting:env.temporary_idle_hours` | **Temporary idle time** — Hours without activity before a temporary environment and its owned files are removed. Its owner gets an hour to keep or export it. |
| `setting:env.recycle_idle_minutes` | **Recycle idle time** — The idle time, in minutes, that the Recycle dialog starts from; 0 applies no minimum idle time. |
| `setting:hosts.full_percent` | **Full at** — The share of a host's memory, declared as needed by what runs there, at which the host counts as full. |
| `setting:hosts.when_full` | **When a host is full** — Whether making or moving an environment onto a full host only warns, or is refused beforehand. |
| `setting:runtime.idle_stop_minutes` | **Idle stop** — Stop a managed provider's runtime after this many minutes without a session or a lease; 0 never stops it. |
| `setting:runtime.template_id` | **Template** — The provider template that a managed provider's runtime is made from. |
| `setting:runtime.host_id` | **Host** — The host that a managed provider's runtime is made on; unset, the one with the most room. |
| `setting:runtime.resources` | **Limits** — Limits over the template's values: processor, memory, processes and disk. The driver reports which limits it enforces. |
| `option:runtime.resources.cpu_millis` | CPU allowance in thousandths of a CPU. |
| `option:runtime.resources.memory_mb` | Memory allowance in MiB. |
| `option:runtime.resources.memory_swap_mb` | Combined memory and swap allowance in MiB. |
| `option:runtime.resources.blkio_weight` | Relative block-I/O scheduling weight. |
| `option:runtime.resources.pids` | Maximum process count. |
| `option:runtime.resources.disk_gb` | Disk allowance in GiB. |
| `setting:runtime.storage` | **Storage** — Named machine storages for new provider home and connector state data. Existing data keeps its recorded location. Provider runtimes have no workspace. |
| `option:runtime.storage.workspace` | Working files storage. |
| `option:runtime.storage.home` | Agent home storage. |
| `option:runtime.storage.state` | Recorded file-state storage. |
| `setting:runtime.home` | **Agent home** — A home made by Orch or an existing directory allowed by this machine. Leave unset to keep the profile’s choice or the agent program’s default and its existing sign-in. Existing directories keep their owner, permissions and files; deleting the runtime never deletes them. |
| `option:runtime.home.kind` | The declared source or content kind. |
| `option:runtime.home.path` | The path on the machine that owns this setting. |
| `setting:runtime.needs` | **Needs** — The processor, memory and disk a managed provider runtime declares it needs for placement. |
| `option:runtime.needs.memory_mb` | Memory allowance in MiB. |
| `option:runtime.needs.cpu_millis` | CPU allowance in thousandths of a CPU. |
| `option:runtime.needs.disk_gb` | Disk allowance in GiB. |
| `setting:runtime.after_up` | **Commands after start** — Named argv commands run in the provider runtime after each start or attachment, where its machine permits them. Only its owners can change them on the provider. |
| `option:runtime.after_up.*.command` | Executable and arguments, run directly without a shell. |
| `option:runtime.after_up.*.timeout_secs` | Command timeout in seconds. |
| `setting:runtime.variables` | **Variables** — Runtime variables override the template and agent profile when its runtime next starts or attaches. |
| `setting:runtime.secret_names` | **Secrets** — Names of provider secrets supplied over the profile's variables. Values are sealed to the runtime's machine, written separately and never returned. |
| `setting:runtime.update` | **Updates** — Whether a runtime that is behind waits for somebody to update it, or is updated whenever it starts. |
| `setting:runtime.renew_days` | **Renewal** — Renew a sign-in when fewer than this many days remain, or this many days after its last renewal when its expiry is unknown; 0 switches renewal off. |
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
| `setting:providers.expiry_notice_days` | **Expiry notice** — How many days before a token that a provider runs on lapses its managers are told; 0 tells them only once it has lapsed. |
| `setting:providers.subscription_harnesses` | **Subscriptions allowed** — The agent programs whose subscriptions may be added as providers here. |
| `setting:providers.subscription_sharing` | **Subscriptions shared** — The agent programs whose subscriptions may be used by people other than their owner. |
| `setting:resources.created_by_members` | **Resources members may add** — The kinds of resource that any member may make in an org as their own. |
| `setting:skills.max_bytes` | **Skill size** — The most bytes the files of one version of a skill hold together. |
| `setting:skills.max_files` | **Skill files** — The most files one version of a skill holds. |
| `setting:notify.subagents` | **Sub-agent notices** — Whether the people who follow a thread are told when one of its sub-agents finishes, fails or is stopped, as they are told when a turn ends. |
| `setting:message.body_max_chars` | **Message length** — How many characters an administrator message may contain; the default can be changed at the system or organization. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:notify.operations` | **Operation notices** — Whether Notify starts ticked in an operation: when work is blocked, always, or never. The person confirms the choice; recipients keep their own notification choices. |
| `setting:broadcast.rate_per_hour` | **Announcements an hour** — How many announcements one thread's agent may send in a rolling hour; 0 allows none. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:capability.recreate_grace_secs` | **Recreate grace** — How many seconds running turns may finish before a recreate interrupts them; 0 interrupts them immediately. |
| `setting:access.request_expiry_days` | **Access request expiry** — After how many days a request for access that nobody decided lapses. |
| `setting:access.max_pending_requests` | **Pending requests** — How many requests for access one person may have waiting. |
| `setting:access.quick_grant_durations` | **Quick access durations** — Durations offered for giving access in one click: a positive number of days followed by d, or always. A lower scope may remove allowed choices; it cannot add one excluded above. |
| `setting:nodes.new_color` | **New node colour** — Whether a new place starts with its kind's icon colour or one of the twelve picked at random. |
| `setting:tags.max_per_place` | **Tags on a place** — How many tags one place carries at most; 0 allows none. A lower scope can only reduce this limit; it cannot remove a limit set above. |
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
| `setting:auto.title` | **Titles** — How automatic titles are written: no title, the first input, the agent’s title, or a summary. |
| `setting:auto.tags` | **Tags** — Whether the summarizer sets a place's tags. |
| `setting:summary.handoff_input_share` | **Summary input share** — How much of the summary model's context window a hand-off, a fork or a goal check may fill with the conversation it sends: the newest turns first, older ones as their one-line index. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:auto.summarizer` | **Summarizer** — Which summarizer is used, and its configuration. |
| `option:auto.summarizer.kind` | The declared source or content kind. |
| `setting:auto.triggers` | **Triggers** — When automatic titles, tags and summaries are written again: after a turn, at a milestone or at intervals, and how often at most. |
| `option:auto.triggers.on` | Events that trigger this automation. |
| `option:auto.triggers.min_interval_secs` | Minimum gap between automatic runs in seconds. |
| `option:auto.triggers.interval_secs` | Scheduled interval in seconds. |
| `setting:capability.policy` | **Capability policy** — For each capability, whether a request for it is allowed, asked about or denied, and who decides. |
| `option:capability.policy.*.decision` | Whether the capability is allowed or requires a decision. |
| `option:capability.policy.*.approver_scope` | Which area supplies the approvers. |
| `setting:sharing.allow_external` | **Owners share outside** — Whether the owners of a thing here may themselves name somebody from outside its root org on its page. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:sharing.allow_copy_out` | **Copies to other orgs** — Whether things here may be duplicated or moved into another org. On by default. A lower scope can switch this off; it cannot undo a refusal set above. Exporting and creating at the destination still need their own permissions. |
| `setting:sharing.external_max_days` | **Outside entries end** — The longest an entry that names somebody from outside may last after it is written or renewed. |
| `setting:sharing.allow_holders` | **Holders share** — Whether somebody who holds a thing here without owning it or managing its page may share it, at most at their own level: name others on its page, give a role in a place, share it with agents. When off, sharing is with a thing's owners and whoever manages its page. A lower scope can switch this off; it cannot undo a refusal set above. |
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
| `setting:usage.catalogue_refresh_days` | **Catalogue refresh** — Every how many days the system fetches the rate catalogue by itself; 0 only when an admin asks. |
| `setting:chat.enabled` | **Chats** — Whether people may post in the chats of the nodes here. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:chat.direct` | **Direct chats** — Whether the members of a root org may start direct and group chats with each other. A lower scope can only choose a stricter option. Strictness order: `members` → `off` (strictest last). |
| `setting:chat.group_max_members` | **Group size** — How many people one group chat may have. |
| `setting:chat.group_mentions` | **Role and group mentions** — Who may name role and group audiences among current readers: chat writers, or its current managers for a group chat. A lower scope can only choose a stricter option. Strictness order: `writers` → `managers` (strictest last). |
| `setting:notifications.person_rate_per_hour` | **Person notifications per hour** — How many people one person may explicitly notify in one root org in a rolling hour; 0 disables the limit. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:notifications.person_note_chars` | **Person notification note length** — How many characters an optional notification note may have; 0 allows no note. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:references.preview_bytes` | **File preview size** — Bytes a reference card reads for a text excerpt. |
| `setting:references.preview_lines` | **File preview context** — Lines shown before and after a referenced line; 0 shows only the referenced line. |
| `setting:files.preview_source_bytes` | **Preview source size** — The largest original decoded for a static file preview. |
| `setting:files.preview_pixels` | **Preview image pixels** — The largest image decoded for a static preview, checked before allocating pixels. |
| `setting:files.preview_edge` | **Preview dimensions** — The longest edge of a static file thumbnail, in pixels. |
| `setting:files.preview_bytes` | **Preview size** — The most encoded bytes in one static thumbnail or text excerpt. |
| `setting:files.preview_cache_bytes` | **Preview cache** — Bytes kept for derived previews; older previews are discarded first, and zero disables caching. |
| `setting:files.viewer_bytes` | **Media viewer size** — The largest original image, PDF, audio or video file read into a browser viewer. |
| `setting:files.native_image_bytes` | **Native image size** — The largest image sent natively to a model; larger images are delivered by path, and zero always uses paths. |
| `setting:docs.enabled` | **Documents** — Whether new documents may be made beside the nodes here. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:docs.templates` | **Starting documents** — Current readable ordinary documents offered when creating one here; a template grants no source access or creation permission. |
| `setting:docs.max_bytes` | **Document size** — The most text one document here may hold, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:docs.max_blocks` | **Document blocks** — The most blocks one document here may have. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:docs.versions_kept` | **Versions kept** — How many versions of one document here are kept at most. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:docs.history_max_bytes` | **History size** — How much the kept versions of one document here may take together, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:docs.file_max_bytes` | **File size** — The largest one file a document here may hold, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:docs.files_max_bytes` | **Files together** — The most the files of one document here may come to together, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:documents.links.shown` | **Links tabs** — The tabs of a document's Links section that its readers see. A document's owner can switch tabs off for one document; a lock here decides for the whole place. |
| `setting:documents.links.page_size` | **Links page size** — Rows shown in a Links list before Show more. |
| `setting:documents.links.occurrences` | **Mentions per source** — Mentions listed under one source in Referenced by before Show more. |
| `setting:documents.links.line_chars` | **Mention line length** — Characters of the line shown around each mention in Referenced by. |
| `setting:documents.links.mentions_counted` | **Mentions counted** — Mentions of the document counted in one source before its count reads “or more”. |
| `setting:documents.links.access_checks` | **Links access checks** — Sources whose reading access is checked for one page of Referenced by. A very popular document continues with a cursor. |
| `setting:documents.links.graph_nodes` | **Graph nodes** — The most nodes the Links graph draws. |
| `setting:chat.attachment_max_bytes` | **Attachment size** — The largest file attached to a chat message, in bytes. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:search.word_recent_candidates` | **Recent word sample** — Recent rows sampled before probing the text index. |
| `setting:search.word_candidates` | **Word candidates** — Candidates per indexed field before exact access and ranking. |
| `setting:search.word_probe_candidates` | **Word index probe** — Indexed matches sampled before switching to a recency scan. |
| `setting:search.word_tuning` | **Word retrieval** — Estimated rows below which word search uses native scoped reads. Missing field limits use Word candidates. Windows per request defaults to one; Show more continues. |
| `option:search.word_tuning.scope_candidates` | Rows inspected inside the authorized search scope. |
| `option:search.word_tuning.request_windows` | Search request window limits. |
| `option:search.word_tuning.tiers` | Rates or limits by the declared tier. |
| `option:search.word_tuning.tiers.title` | Title text or its ranking weight. |
| `option:search.word_tuning.tiers.description` | Description text or its ranking weight. |
| `option:search.word_tuning.tiers.heading` | Heading text or its ranking weight. |
| `option:search.word_tuning.tiers.body` | Body text or its ranking weight. |
| `setting:search.page_size` | **Page size** — Results shown before Show more. |
| `setting:search.reorder_window_secs` | **Reorder window** — How long new results may reorder the first screen. |
| `setting:search.meaning_pause_ms` | **Typing pause** — How long typing pauses before an embeddings request, in milliseconds. |
| `setting:search.cursor_entries` | **Saved searches** — Continuations kept for each reader. |
| `setting:search.cursor_ttl_secs` | **Search lifetime** — How long a search's continuation stays available. |
| `setting:search.vector_cache_entries` | **Query cache size** — Query vectors cached per org; zero disables caching. |
| `setting:search.vector_cache_secs` | **Vector cache** — How long an org reuses a query vector for the same model. |
| `setting:search.semantic.enabled` | **Semantic search** — Also find results with a similar meaning, using the embeddings below. A lower scope can switch this off; it cannot undo a refusal set above. |
| `setting:related.weights` | **Related weights** — Weights from zero to one for meaning, place and references, as decimal text. Zero leaves a signal out. |
| `option:related.weights.meaning` | Weight of similarity in meaning. |
| `option:related.weights.place` | Weight of proximity in the place tree. |
| `option:related.weights.references` | Weight of explicit references. |
| `setting:related.place_scores` | **Related places** — Scores from zero to one for the same folder, topic, project or org. |
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
| `setting:search.semantic.max_disk_share` | **Disk limit** — Index size limit as a share of the indexed content's size; indexing pauses at the limit. Empty: no limit. A lower scope can only reduce this limit; it cannot remove a limit set above. |
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
| `setting:retention.delete_archived_days` | **Delete archived nodes** — Delete a node this many days after it was archived; 0 never does. |
| `setting:retention.agent_history_idle_days` | **Agent history kept** — Remove a thread's agent history after this many days without a turn; 0 never does. |
| `setting:retention.inbox_days` | **Inbox kept** — For how many days a notification stays in a person's Inbox, read or not, before it is removed. |
| `setting:redaction.mask_secrets` | **Mask secrets** — Whether the values of the secrets a thread was given are masked in stored text. A lower scope can require this protection; it cannot switch off protection required above. |
| `setting:redaction.patterns` | **Patterns** — Further patterns to mask in stored text, each with a name. |
| `option:redaction.patterns.*.name` | A readable name for the declared item. |
| `option:redaction.patterns.*.regex` | Regular expression used by the rule. |
| `setting:diagnostics.retention_days` | **Diagnostics kept** — For how many days diagnostics are kept. |
| `setting:orgs.personal_enabled` | **Create personal organizations** — Give new accounts a personal organization. Disabling this keeps existing organizations; people can still join teams and receive document shares. |
| `setting:orgs.personal_closed` | **Personal orgs closed** — Whether a person's own org is created taking no member but its person. |
| `setting:orgs.personal_flat` | **Personal orgs flat** — Whether a person's own org is created holding no org inside it. |
| `setting:orgs.created_by` | **Who may make an org** — Who may make a team org: anybody signed in, system admins only, or somebody a system admin invited to make one, each invitation used once. Personal organization creation is a separate policy. |
| `setting:navigation.order` | **Order of the tree** — The order in which the org's tree is shown to whoever follows the org's own: as somebody arranged it, the newest first, the most recently changed first, or by name. An owner or admin of the org saves it from the order they see. |
| `option:navigation.order.by` | The recorded author, or the declared sort order for navigation. |
| `option:navigation.order.custom` | Explicit per-parent navigation order. |
| `setting:registration.mode` | **Registration** — Who may register: nobody, only somebody with an invitation, or anybody who reaches the system. |
| `setting:org.terms` | **Terms for people who join** — Markdown terms for joining this org or accepting a guest invitation to its places or chats. Root terms come first, followed by each nearer org’s terms. Empty text adds no terms; existing members are not asked to accept later edits. |
| `setting:invitations.forms` | **Invitation forms** — The forms of invitation that may be made for an org of this tree: a link to share, a personal link for one address, or both. A lower scope may remove allowed choices; it cannot add one excluded above. |
| `setting:invitations.default_hours` | **Invitation life** — For how many hours an invitation is good when its inviter says nothing. |
| `setting:invitations.max_hours` | **Longest invitation** — The most hours an invitation may be good for. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:invitations.max_uses` | **Uses of a shared link** — How many people one shared link may bring in at most. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:mail.shared` | **Shared mail server** — Whether a root org that has no mail server of its own sends its mail through the system's. |
| `setting:api.idempotency_window` | **Creation replay window** — How many hours a creation key retains its accepted identity; 0 disables replay. This default can be increased. |
| `setting:release.check_days` | **Look for a release** — Days between read-only checks of the operator's configured release source; 0 is off. A check sends no installation statistics and installs nothing. |
| `setting:cleanup.release_keep_count` | **Keep unused releases** — Unused releases kept per payload after an update; 0 keeps all. Current and previous releases are always kept. |
| `setting:cleanup.unused_keep_days` | **Clean unused caches** — Days to keep images and tool versions that nothing uses; 0 switches automatic cleanup off. Data follows its own retention dates. |
| `setting:backup.warn_days` | **Database backup warning** — Warn when the last successful database-only backup is older than this many days. A failed attempt does not make it newer. |
| `setting:registration.email_verification` | **Email verification** — Whether a person has to confirm their address before they sign in. |
| `setting:auth.local_login` | **Password sign-in** — Whether people other than system admins may sign in with a password or a mailed code. |
| `setting:auth.email_code` | **Code by mail** — What a code sent to a person's confirmed address may be when they sign in: a way in by itself, a second step after the password, both or neither. A lower scope may remove allowed choices; it cannot add one excluded above. |
| `setting:auth.second_step` | **Second step** — Whether the people of this org need a second proof set up and given before they work here. A lower scope can only choose a stricter option. Strictness order: `optional` → `required` (strictest last). |
| `setting:tokens.max_days` | **Longest token life** — The longest total life of an API token that is admitted here: from when it was made to when it ends. |
| `setting:system.banner` | **Banner** — A message shown to everybody on the system, with its level and when it ends. |
| `option:system.banner.text` | Text carried by this rule. |
| `option:system.banner.level` | The requested access level. |
| `option:system.banner.until` | When the rule ceases to apply. |
| `setting:outbound.allowed_private_hosts` | **Private hosts** — The hosts on private addresses that a channel or a destination of an org or a person may reach. |
| `setting:storage.max_blob_bytes` | **Largest file** — The largest single file that Orch stores, in bytes. |
| `setting:storage.env_cache_quota_bytes` | **File cache** — How many bytes of cached file contents are kept for one environment. A lower scope can only reduce this limit; it cannot remove a limit set above. |
| `setting:agents.page_fetch` | **Page fetch** — Unset keeps the program default. Off disables the capability; Native uses a declared page reader; Service uses the explicitly selected fetch backend. It is independent of Web search. |
| `setting:agents.image_generation` | **Image generation** — Unset keeps Program default. Native uses a supported native image tool, Off disables it, and Service uses the explicitly selected backend with native generation off. A default is not a lock; no paid service is selected implicitly. |
| `setting:gateways.tunnel_strategy` | **Tunnel gateway** — New pairing plans use the lowest combined fresh round trip, or stay on the executor’s gateway. Existing tunnels retain their frozen placement. |
| `setting:threads.agent_schedules` | **Agent schedules** — Let the agent schedule visible future inputs on this thread. Off by default; current permissions and provider allowances apply to every run. |
| `setting:agents.image_generation_backend` | **Image generation backend** — Select an exact image route and enabled provider offering. The choice is inherited through the thread and respects higher locks. Unset selects no paid service; current provider Use and limits still apply when it runs. |
| `option:agents.image_generation_backend.route` | The declared image route, such as image_generation.service-api for an Images API or image_generation.service-model for a hosted model tool. An unavailable route stays unavailable; it never chooses another provider implicitly. |
| `option:agents.image_generation_backend.offering` | The exact provider and model or service used by this route. Omit only when that route needs no offering; selection alone grants no access. |
| `option:agents.image_generation_backend.offering.provider` | The provider account for this image call, independently of the conversation’s provider. The driver needs its current Use permission and the offering must fit the applicable allowed lists. |
| `option:agents.image_generation_backend.offering.offering` | The provider-local name of the enabled image service or hosted-tool generation model required by the route. Choose the exact offering rather than an account’s default or a price-based fallback. |
| `option:agents.image_generation_backend.config` | Additional references understood by the selected route. Leave empty unless that route documents them; never put provider credentials here. This object cannot grant Use, bypass limits or override current source authority. |
| `setting:agents.web_search_backend` | **Web search backend** — Choose an enabled provider and native-search model, or that provider's search and fetch services. Unset offers no paid service automatically. |
| `setting:threads.agent_schedule_hours` | **Agent schedule duration** — Default lifetime in hours for recurring agent schedules. An explicit end may be later; this is a default, not a cap. |
| `setting:threads.agent_schedule_runs` | **Agent schedule runs** — Default occurrence count for recurring agent schedules. The agent may choose another positive count; this is a default, not a cap. |
| `setting:summary.targets` | **Summary targets** — Refresh events and input/output defaults by target, off until enabled under the chosen summarizer and shared usage caps. Help and search results use written or existing summaries. |
| `setting:summary.follow_interval_secs` | **Summary follow interval** — Free reads while a summary is eligible or running; future work waits until its due time. Defaults to 5 seconds. |
| `setting:summary.read_batch_size` | **Summary read batch** — Distinct source targets in one stored search-summary window; further targets continue. Defaults to 100. |
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
| `setting:ask.models` | **Ask models** — Allowed API model identities for Ask. Children only narrow; parent order remains. The first usable entry is the default. Unset everywhere or empty blocks Ask. |
| `setting:ask.paid_by` | **Who pays for Ask** — The person asking, or the writer of this setting. Usage is always attributed to the person asking. |
| `setting:ask.retention_days` | **Ask history kept** — Days after the last message before permanent deletion, within the root organization's minimum and maximum. |
| `setting:ask.retention_min_days` | **Shortest Ask retention** — The minimum history retention chosen by administrators of the conversation's root organization. A lower scope can only raise this minimum. |
| `setting:ask.retention_max_days` | **Longest Ask retention** — The maximum history retention allowed by administrators of the conversation's root organization. |
| `setting:ask.max_steps` | **Ask steps per turn** — The model steps allowed for one question before it asks the person to continue. |
| `setting:ask.max_output_tokens` | **Ask answer tokens** — The most output tokens per Ask model request, also reserved before fitting conversation context. The lowest system or organization value applies. |
| `setting:ask.context_tokens` | **Ask context tokens** — Unset uses the chosen model's context window. When set, the smaller value applies to Ask only; output and tool-result reserves must still fit. The lowest system or organization value applies. |
| `setting:ask.max_tools` | **Ask tools per turn** — The read operations allowed for one question before it asks the person to narrow the question. |
| `setting:ask.tool_bytes` | **Ask source size** — The most bytes from one read result supplied to the model, with a canonical link to the full source. |
| `setting:ask.turn_tool_bytes` | **Ask sources per turn** — The total read-result bytes supplied while answering one question. |
| `setting:providers.model_groups_max` | **Model groups** — The maximum number of named groups per model. The default is 8 and can be raised; 0 allows no new group memberships. Existing memberships are kept. |
| `option:usage.rates.*.web_fetch_per_call` | Credits per successful page-fetch call. |
