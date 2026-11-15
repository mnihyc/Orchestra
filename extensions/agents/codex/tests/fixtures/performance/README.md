# Recorded request boundaries

Captured on Codex 0.162.1 using a plan sign-in and an API key through OpenRouter. The tool case contains two requests, including a short burst whose speed is deliberately unmeasured. The high-effort answer supplies both the long-answer and hidden-reasoning cases. The API-key case streams reasoning summaries.

Each line preserves the event and its arrival timestamp, rounded to milliseconds from the capture's monotonic stopwatch. Identifiers are fixed references, account notifications and thinking signatures are removed, and home paths are anonymized. Expected values are literal differences between these stamps; they are independent of the adapter's implementation. Speed requires at least 500 ms and 32 streamed tokens. Reasoning is removed only when it was hidden.

The schema fixture covers the request-boundary notification surface and its referenced types. Native conformance compares that surface with the pinned app server's generated JSON schema before a harness upgrade is accepted.

`retry.jsonl` is a constructed protocol case: a normalized 429 failure followed by a fresh attempt. It contains fixed ids and no account state; the observer test asserts the two outcomes and the restarted timing.

`web-search.jsonl` is a constructed provider-side search case on the pinned event surface. One token update covers the text before and after search, so search does not open a second request. The supplied Codex captures contain no web search; a native capture remains an integrator validation case.

## Hosted web search remains unproved

There is no real hosted-search rollout in these fixtures. A model-side
`web_search_call` must not be treated as a standalone `function_call_output`, or
declared successful from its title, sources, analytics or the turn's success.
The action closes at its own reported end; an unmatched retained result leaves
its usage unknown and adds an unverified notice.

To validate a hosted decoder, capture a fresh conversation on the exact supported
native build, using the normal native search mode and a harmless public question.
Record the build, route (direct sign-in or forwarded), model and search mode.
Retain these two synchronized streams, with their original event order:

- App-server notifications from the turn request through `turn/completed`, including
  every `webSearch` start/end, `turnId`, action, token update and reroute.
- Rollout records for that same session and turn: `session_meta`, `turn_context`,
  all `web_search_call` response items, and any corresponding
  `function_call_output` records, including their absence.

Replace identifiers consistently across both streams. Remove account, credential,
plan-limit and private-path information without changing status, action, model
association or result structure. Keep a note of the exact schema and capture
command. Do not relabel a constructed case as a recording.

A case built from that capture must prove the exact session/turn/item association,
the real action and terminal state, immediate display completion while the turn
continues, and only the usage quantity the captured structure actually supports.
It must also cover an unrelated turn, a missing model, replay without re-billing,
and no duplicate count alongside a forwarded meter. Recorded failure or cancelled
searches must remain distinct from successful searches with no results. Until
those observations exist, hosted usage is explicitly unproved.
