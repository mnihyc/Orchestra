# Executors and model protocols

An [`Executor`](../../../crates/contracts/executor/README.md) starts a service
in an environment and returns its loopback endpoint. Its descriptor names its
protocol, available tool sets, native sandbox modes and attribution variables.
Keep all state under the supplied data directory and bind a port assigned for
that instance. Two environments on one host must have distinct state, listeners
and credentials; stopping one must leave the other running.

An environment lists its executor offers in order. A harness declares which
offers it can use, and a session takes the first compatible one. Orch's own
server tools are independent of that list. The built-in executors currently
offer Linux operation; macOS and Windows are withheld until their actual
native containment, process and reconnect checks run.

Use `orch_conformance::executor::run` with two fresh instances and an
`executor::Fixture` of distinct environments. The reusable suite checks startup,
declarations and independent listeners. Add real protocol calls, file
containment, attribution, shell cleanup and reconnect tests for your executor;
a reachable TCP listener alone cannot prove those effects.

A [`WireProtocol`](../../../crates/contracts/model/README.md) admits exact
model requests, extracts the routing name, presents credentials according to
endpoint options and reads usage while bytes pass through unchanged. Keep the
allowed request table narrow. Harness credentials are replaced at the provider
side; provider endpoints and secret headers are not returned to the browser.
Responses, Chat Completions, Messages and Embeddings have built-in implementations. Use the protocol
suite with literal format examples and expected counts, including byte-by-byte
stream boundaries, and the controlled model server for actual forwarding.

Chat Completions uses `openai-chat` with an endpoint base such as `https://model.example/v1`;
the relay appends `/chat/completions`. It preserves tools, custom request fields, JSON replies
and SSE frames, including `[DONE]`, while replacing only authorization as configured. The agent's
descriptor must declare that format; adding an endpoint never makes an incompatible agent support it.
The pinned Codex adapter uses Responses. The [OpenAI configuration reference](https://developers.openai.com/codex/config-reference/)
documents `wire_api` as Responses only, so the provider form directs Codex users to that format.

Embeddings uses `openai-embeddings` with the same kind of base; the relay appends `/embeddings`. No agent
speaks it: Orch itself sends batches of texts there (`provider.embed`, from the provider's runtime) to search
by meaning, asking for `encoding_format: "float"` and, where a length is set, `dimensions`. A protocol that can
do this implements `embedding_request` and `embedding_vectors`; the others keep the defaults, which decline.

A `ProtocolTranslator` converts a request to a different upstream format and
streams the response back. Its declaration states preserved, approximated and
dropped meaning. A built-in wire protocol presents the upstream credential when
it implements the target; otherwise the translator presents it and marks the
request authorized. The translator suite compares a supplied request/response
example and every response byte boundary. There is no built-in translator;
external translators use the same trait through their bridge.
