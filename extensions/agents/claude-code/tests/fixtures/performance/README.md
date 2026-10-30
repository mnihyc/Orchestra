# Recorded request boundaries

Captured on Claude Code 2.1.296 with partial messages enabled, using a plan sign-in and an API key through OpenRouter. The tool case contains two short requests whose speeds are deliberately unmeasured. The long answer streams text. The OpenRouter run supplies both the API-key and hidden-thinking cases; it streams a thinking signature and reports 199 thinking tokens in message_delta. The shared rule subtracts those hidden tokens and measures 249 streamed tokens over 2,836 ms: 87,800 milli tok/s.

Each line preserves the event and its arrival timestamp, rounded to milliseconds from the capture's monotonic stopwatch. Identifiers are fixed references, thinking signatures are removed, and home paths are anonymized. The ready input is marked by the captured system/init; subsequent inputs by tool-result messages. Expected durations are literal stamp differences, independent of the adapter implementation. The sum of request durations stays within ten percent of the result's duration_api_ms. Complete sub-agent messages carry counts without invented times.

`retry.jsonl` is a constructed protocol case: a normalized 429 failure followed by a fresh attempt. It contains fixed ids and no account state; the observer test asserts the two outcomes and the restarted timing.
