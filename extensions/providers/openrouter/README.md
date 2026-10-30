# orch-provider-openrouter

OpenRouter's documented ordinary-key usage and account-credit source. It reads the key (`GET /api/v1/key`) and reports its spending, its limit and each budget that starts over as a window with its next reset (daily at 00:00 UTC, weekly on Monday at 00:00 UTC, monthly on the first at 00:00 UTC, as OpenRouter's limits page says; the binding budget marked), a limit or a budget that never starts over as a balance, the free models' requests of the day as a window counted in requests, and the key's lapse. The account's credits (`GET /api/v1/credits`) are told only where OpenRouter answers them, for a management key; an ordinary key's refusal keeps the key's observation and is no error. The connector supplies the network transport; this crate bounds the whole read to eight seconds and each reply to one MiB. The programs that compose the API and the connector register `source::builtin()` in their table of built-in sources; the cores know it only through that table.

| Module | Description |
| --- | --- |
| `lib.rs` | OpenRouter's ordinary-key spending, credits and lapse, using bounded read-only vendor requests. |
| `source.rs` | OpenRouter's key and account-credit reads through the common facts source interface. |

Unit tests cover the next reset of each interval and the share of a limit; `tests/recorded.rs` replays recorded answers (a key with a weekly limit, budgets with the binding one, a key without a limit and one whose limit never starts over, a management key's credits and an ordinary key's refused credits). The connector program's tests read them through the connector's common reader of facts, and its transport tests cover authorization and exact addresses. No live vendor key is needed by those tests.
