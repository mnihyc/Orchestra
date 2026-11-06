---
screens: [/resources/providers, /thread/$threadId, /usage]
keywords: [web search, native search, grounding, search fees, public web, cached, live, DeepSeek]
---

# Provider web search

A provider can make its model's native web search available to agents, summaries and goal checks. Provider managers decide which search modes are available and which mode starts a new use. Offering support does not switch it on.

On the provider's Models page, enable the supported modes and select the default. A thread then chooses its mode in Parameters, within the provider's limits. **Disabled** removes the hosted search tool. **Cached** uses indexed content without live retrieval where the protocol supports it. **Live** allows the model to search when it needs information. A default remains a choice; a fixed provider option shows its source.

Endpoint request defaults can configure the native search tool through `options.params.web_search`. Responses supports search context size, domain filters, approximate location and live-access control through the selected mode. Messages supports a search limit, allowed or blocked domains and approximate location. Those options configure the hosted tool while keeping the request's existing tools.

Search support depends on the endpoint and model. Sharing a wire format does not imply that every model supports every tool. DeepSeek documents hosted search for its Pro model through Responses; a model that ignores search is not proof that it ran. Google's OpenAI compatibility documentation lists grounding tools for its Images endpoint, without documenting Chat grounding. Google Search grounding needs its native protocol or a translator that declares it.

Search fees can be additional to model tokens. Usage records keep a vendor-reported search count when available. A visible WebSearch tool call can contain several backend searches, so its presence is not a billable count. A missing count is unknown. Provider prices estimate cost; a charge explicitly reported by the upstream remains authoritative.

Native web search finds public web information. Orch's `search_shared` tool searches documents, chats and threads the agent may read. They are separate capabilities.

For upstream details, see [OpenAI web search](https://developers.openai.com/api/docs/guides/tools-web-search), [Anthropic web search](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool), [DeepSeek Responses](https://api-docs.deepseek.com/guides/responses_api/) and [Gemini grounding](https://ai.google.dev/gemini-api/docs/google-search).

## Configure the hosted tool

Enable modes on the provider's model first. For a Responses endpoint, an example request default is:

```json
{"web_search":{"mode":"live","search_context_size":"low","filters":{"allowed_domains":["example.com"]}}}
```

For Messages, use its own options:

```json
{"web_search":{"mode":"live","max_uses":3,"allowed_domains":["example.com"]}}
```

Allow and block lists cannot be combined. A per-use Disabled choice overrides a default. Endpoint defaults configure a tool already selected by an agent; they do not enable a tool the harness left out. Summaries and goal checks resolve their configured choices through the same provider model limits.

## Set a search price

The model's rate and the rate card accept **Credits per web search** (`web_search_per_call`). Enter the price in the same credit unit as token rates. In US-dollar units, the documented basic search fee is `0.01` for OpenAI and Anthropic, and `0.00075` for a successful DeepSeek search. Search fees do not receive the token rate's service-tier multiplier. Search tokens can still carry their own model token charge.

Counts and prices are recorded with each request. A search-enabled request with a missing count or search price has an unknown complete charge. A repeated cumulative report is not an additional use. These are configured usage estimates, not an invoice.
