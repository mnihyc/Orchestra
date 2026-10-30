# Chat Completions protocol

The built-in pass-through implementation is `OpenAiChat` in
[`../openai-responses/src/chat.rs`](../openai-responses/src/chat.rs). It shares OpenAI
authorization, discovery and bounded usage observation without a second package or a
translation layer. Agents declare this format as `openai-chat`; a relay's availability
does not imply that every agent speaks it. Codex remains limited to its declared Responses format.
