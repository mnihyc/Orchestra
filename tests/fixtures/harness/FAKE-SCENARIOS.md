# Scripted upgrade scenarios

The fake program uses its existing parser, session files, interruptible waits,
item emitter and lifecycle methods. A typed tool does not execute a real command
or fetch a page; it simulates the same native event the engine handles. Existing
run/edit/MCP/subagent/plan directives retain their established behavior.

Lifecycle/idle/background controls are actual adapter methods. A script cannot
claim to be idle or open a second session just by injecting an event into its
active turn. `observe_idle` rejects working turns, stamps the real session clock,
keeps its native item and emits only a passive completed marker.

| Shared scenario | Fake control |
|---|---|
| `start` | Real `start` and `open_session(New)` |
| `resume` | Real `open_session(Resume)` with the retained native ref |
| `fork` | Real `open_session(Fork)` and its actual native history |
| `tool_command` | `run <command> => <code> <output>` |
| `tool_file_change` | `edit <path> +<n> -<n>` |
| `tool_read` | `tool` with `file_read` payload |
| `tool_search` | `tool` with `file_search` payload |
| `tool_web_search` | `tool` with `web_search` payload |
| `tool_web_fetch` | `tool` with `web_fetch` payload |
| `tool_mcp` | Existing `mcp <server> <tool> <json>` and the actual session MCP list |
| `tool_subagent` | Existing `sub <name>: ...` |
| `tool_plan` | Existing `plan first | second` |
| `approval_command` | Existing `ask command`, or typed `request` Command |
| `approval_file_change` | Typed `request` FileChange |
| `approval_permissions` | Typed `request` Permissions |
| `approval_plan` | Typed `request` Plan |
| `approval_mcp` | Typed `request` McpElicitation |
| `approval_unknown` | `request` with unknown native kind; generic card and actual offered answers/cancellation |
| `approval_checkpoint` | Typed Command `request`, real `resolve_approval` and subsequent command directive |
| `approval_prompt_user_rule` | Typed `request` with native scoped options/rules; real response |
| `approval_network` | Permissions `request` with exact network names, real response |
| `user_input` | Existing `ask input`, or typed UserInput `request` |
| `interrupt` | Real `interrupt` during existing interruptible `sleep`/request/tool wait |
| `steer` | Real `steer` during a running turn, existing input state reporting |
| `compaction_automatic` | `diverged automatic_compaction` (the managed automatic act stays suppressed) |
| `compaction_manual` | Existing `compact <ms>` phase pair |
| `compaction_marker` | Existing `compacted` |
| `compaction_idle` | Real `observe_idle` with `change=compaction`, no turn start or nil turn |
| `usage_limit` | Existing `exhaust` / `limit` with real facts/reset time |
| `rate_limit` | `fault` with `rate_limited` and actual retry metadata |
| `overloaded` | `fault` with `overloaded` |
| `sign_in_expired` | Existing `expire`/native credential replacement methods |
| `sign_in_revoked` | `fault` with `provider_unauthorized`, or actual logout/credential APIs |
| `context_overflow` | `fault` with `context_overflow` |
| `broken_stream` | Existing `cut`, or `fault` with `stream_interrupted`/incomplete |
| `process_exit` | Existing `crash` / actual `stop` |
| `background_end` | Existing `background`, real `end_background_command` |
| `background_wake` | The existing background native-action/continuation response after real End |
| `subagents` | Existing `sub` with actual started/update/completed native records |
| `mcp_startup` | `observe` with `change=mcp_startup` and actual status |
| `mcp_failure` | `observe` failed MCP startup emits native `mcp_server_failed`; ordinary `mcp` retains its actual failure |
| `mcp_elicitation` | Typed McpElicitation `request`, real form/response/cancellation |
| `hooks` | `observe` with `change=hook`, native subject and status |
| `model_reroute` | `reroute` closes the old request and records the new actual model without failing work |
| `settings_program` | `observe` with actual previous/reported/requested values |
| `settings_server` | Same `observe`, server subject and future/default scope |
| `session_close` | Real `close_session` |
| `session_unload` | Real `close_session` unloads the live actor while its native file remains resumable |
| `automatic_divergence` | `diverged <exact managed pin>`; arbitrary future pins need no new parser branch |
| `tolerance_unknown_fields` | Typed JSON directives with extra native fields; no crash, stable known values |
| `tolerance_unknown_event` | `unknown <native event>` with raw disabled; shared counted marker assertion |
| `tolerance_unknown_method` | `unknown <native method>` and actual actor request/reply controls where applicable |
| `tolerance_unknown_enum` | Unknown Request kind becomes Other; other malformed neutral input becomes an explicit error, never a crash |
| `tolerance_optional_fields` | Reordered JSON objects and absent optional TurnError/request fields |
| `tolerance_malformed` | Malformed directive text becomes `fake.script` error before effects; `fault` retains safe native malformed-frame text |

`tool` takes the existing neutral ToolPayload JSON. `request` takes a native
HarnessRequest with the real options; an unknown kind becomes Other, and the
reserved cancellation is consumed by its actual request wait. `observe` takes
`{change,subject?,value,previous?,expected?,next_turn?}`. Measurements default to
Unsupported. Program, actual turn and observation time come from the runner,
not input identity hints. A different known requested value emits the same
nonterminal divergence error as a pinned behavior arriving anyway.

`fault` takes the existing TurnError JSON with code/message/retryable and its
optional native status/retry/reset/limit/partial fields. Use `diverged` and
`reroute` for nonterminal observations; the parser rejects those as terminal
faults. The reroute changes only subsequent model-request evidence, never the
old request or the user's requested model. All JSON is parsed before any script
line can act, so malformed controls cannot execute an earlier command first.

Deciding shared controls are `orch-adapter-fake --test upgrade_compatibility`,
with existing `directives`, `harness`, `sessions` and `requests` suites retained.
