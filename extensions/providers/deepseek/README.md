# orch-provider-deepseek

DeepSeek's documented key balance source. It reads `/user/balance` with the provider's key, keeps currency amounts as decimal text, and reports whether the vendor admits ordinary calls. A refused or changed response tells nothing; it never invents a balance or credential proof.

| Module | Description |
| --- | --- |
| `lib.rs` | DeepSeek's documented key balance, retaining the vendor's decimal text and currency parts. |
| `source.rs` | DeepSeek's documented balance response through the common facts source interface. |

Unit tests replay the [documented balance response](https://api-docs.deepseek.com/api/get-user-balance/) and cover zero, unavailable usage, changed response shapes and exact default host selection. The [vendor's first-call documentation](https://api-docs.deepseek.com/) specifies its bases and bearer authorization. The transport is supplied by the connector, with an eight-second deadline, a one-MiB reply bound and no redirects. The programs that compose the API and the connector register `source::builtin()` in their table of built-in sources; the cores know it only through that table.

Not tried with a live DeepSeek key.
