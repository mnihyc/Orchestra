# orch-protocol-openai-responses

OpenAI Responses wire-protocol implementation.

Codex remote compaction uses `compaction_trigger` inputs on ordinary `POST /responses`.
The separate `/responses/compact` route is not relayed.

The relay of the Responses format of OpenAI: it lets through `POST /responses` and
`GET /models`, and gates `POST /alpha/search` on the endpoint's native binding and the admitted thread resolution. It presents the key and the endpoint's headers and query as its options say, refuses in
the vendor's own error document, and reads the usage of a stream or of a plain answer as it passes by. It also turns an
endpoint's list of models into suggestions (the one number such a list gives for the context is the largest the endpoint takes, put among what
the model offers and not among what a manager opened), writes the list a lease answers, and issues the key as it is. Each rule has
a unit test, requests are relayed through the scripted model server of `orch-testkit` (cut streams among them), and one
real request, made only with `ORCH_REAL_API_CHECK=1`, checks the usage observer against a real stream.

Standalone search uses the same bounded plain-response reader and lease meter.
Only a whole successful HTTP result with the native output shape establishes
a search or page operation. Multiple queries and citations are not additional
operations. Errors, cut or malformed bodies and unknown actions establish no
successful quantity. Search-only replies do not report generation tokens.

`OpenAiChat` serves Chat Completions through the same credential, discovery and bounded usage machinery,
without translating request or reply bytes. It allows only `POST /chat/completions` and the lease's `GET /models`.
Streamed final usage and non-streamed prompt/completion counts follow the same token convention. Connector
relay regressions cover both forms, unchanged tools and custom fields, credential isolation and refusal.

`OpenAiEmbeddings` is the Embeddings format (`openai-embeddings`), which OpenAI and most local servers of embedding
models speak. It allows only `POST /embeddings` and the lease's `GET /models`, and makes Orch's own batches of texts
into vectors (`encoding_format: "float"`, `dimensions` where a length is set), read back by each item's `index`. Unit
tests cover the allowed requests, the batch, the order of the vectors, refused answers and the counted tokens.

| Module | Planned responsibility |
|--------|------------------------|
| [requests](src/requests.rs) | Allowed paths and explicit model extraction. |
| [chat](src/chat.rs) | Chat Completions pass-through with shared OpenAI authorization, discovery and usage observation. |
| [embeddings](src/embeddings.rs) | Embeddings pass-through and the request that turns texts into vectors, with shared OpenAI authorization, discovery and usage. |
| [authorization](src/authorization.rs) | The key presented as the endpoint's options say, and shaped denials. |
| [usage](src/usage.rs) | Streaming usage observation. |
| [models](src/models.rs) | Discovery as suggestions only: each model with the name that is sent and, where the endpoint's list has them, the name that is shown and its rate. |
| [credentials](src/credentials.rs) | API-key credential form. |
| [decimal](src/decimal.rs) | Exact decimal text from the numbers that bodies carry. |
