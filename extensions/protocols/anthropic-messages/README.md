# orch-protocol-anthropic-messages

Anthropic Messages wire-protocol implementation.

The relay of the Messages format of Anthropic: it lets through `POST /v1/messages`,
`POST /v1/messages/count_tokens` and `GET /v1/models` and nothing else, presents the key (a bearer, as the vendor
documents) and the endpoint's headers and query as its options say, refuses in the vendor's own error document, and
reads usage from the prompt of `message_start` and the cumulative figures of `message_delta`, or from a plain answer,
as it passes by. It also turns an endpoint's list of models into suggestions (the one number such a list gives for the context is the largest the endpoint takes, put among what
the model offers and not among what a manager opened), writes the list a lease answers, and
issues the key as it is. Each rule has a unit test, requests are relayed through the scripted model server of
`orch-testkit` (cut streams among them), and one real request, made only with `ORCH_REAL_API_CHECK=1`, checks the
usage observer against a real stream.

A plain relay, like `../openai-responses/`: a request passes through in the
format it arrived in, with nothing rewritten but the credential. Which
harness speaks this format is a matter of agent adapters, not of this package.

| Module | Planned responsibility |
|--------|------------------------|
| [requests](src/requests.rs) | Allowed paths and explicit model extraction. |
| [authorization](src/authorization.rs) | The key presented as the endpoint's options say, and shaped denials. |
| [usage](src/usage.rs) | Streaming usage observation. |
| [models](src/models.rs) | Discovery as suggestions only: each model with the name that is sent and, where the endpoint's list has it, the name that is shown. |
| [credentials](src/credentials.rs) | API-key credential form. |
| [decimal](src/decimal.rs) | Exact decimal text from the numbers that bodies carry. |
