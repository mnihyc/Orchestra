# Extension implementations

These are implementation homes behind neutral contracts. Applications select
built-ins; external peers use plugin
bridges in their hosts. Core policy operates on capability declarations.

| Kind | Host | Implementation homes |
|------|------|----------------------|
| Agent adapter | Connector agent role | [Codex](agents/codex/README.md), [Claude Code](agents/claude-code/README.md), [fake](agents/fake/README.md) |
| Driver | Node | [Docker](drivers/docker/README.md), [host](drivers/host/README.md) |
| Executor | Connector environment role through envhost | [Codex](executors/codex/README.md), [Orch](executors/orch/README.md), whose tools also stand in for a harness's own; which of them an environment offers is that environment's own list |
| Model protocol | Connector provider role | [OpenAI Responses](protocols/openai-responses/README.md), [Chat Completions](protocols/openai-chat/README.md), [Anthropic Messages](protocols/anthropic-messages/README.md) and [Embeddings](protocols/openai-embeddings/README.md), all plain relays |
| Translator | Connector provider role | [Seam only](translators/README.md): external implementations, none built in |
| Provider source | Connector beside the credential | Facts, models and reset sources, and keys-only local presentation; [implementer pages](../docs/development/extensions/providers/README.md) and [Acme examples](../examples/plugins/provider/README.md) for programs and bearer-protected addresses |
| Summarizer | API engine | [Model](summarizers/model/README.md) |
| Notifier | Worker | [Email](notifiers/email/README.md), [push](notifiers/push/README.md), [Telegram](notifiers/telegram/README.md), [webhook](notifiers/webhook/README.md): to a person's address, their devices, a chat they linked with a bot, or a place somebody configured |
| Blob store | API, built-in only in v0.1 | [Local](blobs/local/README.md) |
| Identity | API, built-in only in v0.1 | [OIDC](identity/oidc/README.md) |

Inbox entries are state: the engine writes them in the transaction of the
event, and no notifier is involved. A notifier declares whom it reaches, who
may set a channel of its kind up, the fields one is made from and the texts it
ships, so an installed one appears wherever channels and destinations are
added. A channel is the system's, a root org's or a person's own; what goes
through it to a person is that person's route, off until they switch it on. External registration names driver, adapter,
executor, translator, provider, summarizer and notifier kinds. Wire protocols are built
in; an upstream in another format is reached through a translator.
