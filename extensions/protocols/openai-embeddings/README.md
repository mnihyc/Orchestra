# Embeddings protocol

The built-in implementation is `OpenAiEmbeddings` in
[`../openai-responses/src/embeddings.rs`](../openai-responses/src/embeddings.rs). It shares OpenAI authorization,
discovery and bounded usage observation without a second package. Providers name this format as
`openai-embeddings` for an endpoint that answers `POST {base}/embeddings` with `{ model, input: [...],
dimensions? }`: OpenAI's own, or a local server such as llama.cpp's. No agent speaks it; Orch uses it to turn
documents, conversations and chats into vectors for search by meaning, through `provider.embed` in the provider's
runtime.
