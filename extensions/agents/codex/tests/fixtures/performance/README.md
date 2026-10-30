# Recorded request boundaries

Captured on Codex 0.162.1 using a plan sign-in and an API key through OpenRouter. The tool case contains two requests, including a short burst whose speed is deliberately unmeasured. The high-effort answer supplies both the long-answer and hidden-reasoning cases. The API-key case streams reasoning summaries.

Each line preserves the event and its arrival timestamp, rounded to milliseconds from the capture's monotonic stopwatch. Identifiers are fixed references, account notifications and thinking signatures are removed, and home paths are anonymized. Expected values are literal differences between these stamps; they are independent of the adapter's implementation. Speed requires at least 500 ms and 32 streamed tokens. Reasoning is removed only when it was hidden.

The schema fixture covers the request-boundary notification surface and its referenced types. Native conformance compares that surface with the pinned app server's generated JSON schema before a harness upgrade is accepted.

`retry.jsonl` is a constructed protocol case: a normalized 429 failure followed by a fresh attempt. It contains fixed ids and no account state; the observer test asserts the two outcomes and the restarted timing.

`web-search.jsonl` is a constructed provider-side search case on the pinned event surface. One token update covers the text before and after search, so search does not open a second request. The supplied Codex captures contain no web search; a native capture remains an integrator validation case.
