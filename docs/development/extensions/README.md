# Extend Orch

An extension implements a contract and declares its capabilities. Callers use that declaration, so a new implementation fits the same forms, permissions and lifecycle as the existing ones. Rust implementations live under [extensions/](../../../extensions/README.md). Local programs in any language use the JSON-lines [plugin transport](plugins.md).

To write one: read the guide for its kind, implement the contract, check it against the reusable suite of its kind with `orchctl plugin check`, and register it. A capability needs evidence of its effect, not only a flag.

## Which kinds exist, and what implements them

| Kind | What it does | Guide | Built in |
|---|---|---|---|
| Agent adapter | Drives an agent program | [Agent adapters](adapters.md) | Codex, Claude Code, DeepSeek Harness (ACP), and a scripted `fake` adapter |
| Driver | Makes environments on a host | [Drivers](drivers.md) | Docker; host (Linux processes); Kubernetes, built only with the node's `kubernetes` feature |
| Executor, model protocol | Lets an agent reach an environment; speaks to a model service | [Executors and model protocols](executors-and-models.md) | Executors: Codex, Orch. Protocols: OpenAI Responses, Chat Completions, Anthropic Messages |
| Translator | Translates between model protocols | [Executors and model protocols](executors-and-models.md) | None: Orch is a plain relay. The interface is for external implementations |
| Provider source | Reports a provider's facts, models and resets, beside the credential | [Provider sources](providers/README.md) | Examples only: [Acme examples](../../../examples/plugins/provider/README.md) |
| Summarizer | Writes titles, tags, summaries and handoffs | [Summarizers and notifiers](summarizers-and-notifiers.md) | Model |
| Notifier | Delivers a notification outside the application | [Summarizers and notifiers](summarizers-and-notifiers.md) | Email, web push, Telegram, webhook |
| Blob store, identity | Keeps content; signs people in | | Local directory; OpenID Connect. Built in only: no plugin transport |

The [Python examples](../../../examples/plugins/README.md) show a summarizer, a file notifier, a Linux process driver and a provider source without third-party dependencies. Their transport and behavior checks run through the same bridges that hosts use. The [component inventory](../components.md) shows where each implementation lives.
