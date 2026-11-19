# orch-summarizer-model

Model-backed title, description and tag generation selected by configuration.

The built-in model producer frames redacted values and activity as material,
calls the host's permission-checked API provider, and returns bounded text or
tags with actual usage. Its tests cover whole prompts, hostile material,
malformed answers and handoff bounds; the engine owns scheduling and writes.

`auto.summarizer.params` supplies an object of options in the selected endpoint's format.
Endpoint request defaults apply first, then the model's configured defaults, then these explicit options.
The same configuration and completion constructor serve every summary field, document summaries,
handoffs and goal verification. Routing, prompt material, tools, output limits and credentials stay
owned by their existing configuration fields.

For an OpenAI Responses model whose manager enabled `priority` with label **Fast**, use
`"params": {"service_tier": "priority"}`. The documented request alias `fast` is also accepted
when priority is enabled; caps use the actual canonical `priority` identifier. Anthropic Messages
uses `"params": {"speed": "fast"}`; its capacity `service_tier` is a separate option. Configure
required beta/version headers through the endpoint's existing headers. These choices never
establish that a model or account offers them. [OpenAI Fast mode](https://developers.openai.com/api/docs/guides/fast-mode),
[Anthropic Fast mode](https://platform.claude.com/docs/en/build-with-claude/fast-mode).

Nested vendor options remain JSON. Use their actual wire shape, including any vendor-specific
namespace; SDK helpers named `extra_body` are not a universal wire wrapper. Requests through
Gemini, DeepSeek or another compatible endpoint use the same path. Custom configuration does not
advertise an unsupported speed. Usage retains the requested and reported tier separately; a
reported downgrade uses its actual price, and an unreported requested tier leaves credits unknown.

| Module | Planned responsibility |
|--------|------------------------|
| [descriptor](src/descriptor.rs) | Configuration schema and supported fields. |
| [prompts](src/prompts.rs) | Configurable prompt defaults. |
| [completion](src/completion.rs) | Provider-complete request and constrained output conversion. |
