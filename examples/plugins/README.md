# External extensions

Programs that implement an extension point of Orch from outside its code, over the plugin transport: one line of
JSON at a time on standard input and output. They are written against the same contracts as the built-in
implementations, and they show negotiation and observable behavior, not copies of hosts. Each is plain Python 3,
with nothing to install.

| Example | Kind | What it shows |
|---------|------|---------------|
| [notifier](notifier/README.md) | notifier | The smallest useful notifier: it appends what it is asked to deliver to a file. |
| [summarizer](summarizer/README.md) | summarizer | A summarizer with no model: it answers from three fixed rules. |
| [driver](driver/README.md) | driver | Linux process groups, persistent volumes and reconciliation by the system label. |
| [translator](translator/README.md) | translator | Plain-text Messages to Responses requests and streamed text answers with usage and declared losses; example only. |
| [provider](provider/README.md) | provider | A made-up Acme plan with windows, balance, spend, one reset, models and local key presentation; program and bearer-protected address bindings. The [implementer pages](../../docs/development/extensions/providers/README.md) embed these files exactly. |
| [native subscription providers](provider/README.md) | provider | Codex and Claude Code observations through their own unmodified harnesses, with the same sign-in, complete endpoint descriptions and recorded-answer equivalence tests. |

The tests of the `orch-plugin` package run the notifier and the summarizer as real programs, through the traits of
their contracts.
The `orch-conformance` plugin tests also run the translator through its suite and both provider examples through
the provider suite, using the real program bridge and connector address client.
