---
group: providers
screens: [/resources/providers, /resources/providers/$id, /thread/$threadId, /usage]
keywords: [web search, native search, grounding, search fees, public web, cached, live, DeepSeek]
summary: Turn on the search a provider's model has built in, set its price, and read what a search cost.
---

# Provider web search

A provider can make the web search that its model has built in available to agents, summaries and goal checks. The managers of the provider decide which search modes are on, and which mode a new use starts with. Offering support does not turn it on.

Built-in web search finds public information on the web. The tool that Orch gives agents to search documents, chats and threads they may read is a different capability.

## Find the controls

1. Open [Providers](/resources/providers) and choose the provider.
2. On **Models**, open the model and its options. A manager of the provider turns on the modes it supports and chooses the default.
3. Open **Parameters** of a thread. The thread chooses its mode there, within the limits of the provider.
4. [Usage](/usage) shows the searches reported and the charge set, beside the other usage of the request.

A Codex web action closes when the program reports the action’s end, even while
the answer continues. When a hosted search returns search results, the action
shows **Completed** and keeps those results. Its billable search quantity stays
unknown unless the program provides separate usage evidence.

If the action has only source links or an end event and Orch cannot find its
result, the thread shows an unverified notice. A retained result that reports
failure marks the action failed.

Reading native result evidence has a separate short wait so a slow history file
does not hold up other sessions. **Native result wait**
(`agents.native_result_wait_ms`) starts at 2000 milliseconds in system settings;
program profiles can choose their own wait within the engine's interrupt window.
An expired read adds an unverified notice and keeps the action's reported result.

Other sessions and their silence timers continue during that read. Later input for
the waiting session keeps its arrival order. **Native result buffer**
(`agents.native_result_buffer_bytes`) defaults to 8 MiB for that pending input;
profiles can choose another size. If it fills, Orch ends the optional evidence
read as unverified and processes all retained input. No input is dropped and no
manual action is needed. The system default applies when the program starts again.

**Web action wait** (`agents.web_action_wait_secs`) closes a native action that
has not reported its end, with a notice explaining the wait. It starts at 120
seconds, can be changed in the place's settings, and applies when the session
opens again. This closes the action's row while the turn continues; a later result
can still update that row. An action closed this way supplies no usage count.


- **Disabled** takes the hosted search tool away.
- **Cached** uses indexed content without live retrieval, where the protocol supports it.
- **Live** lets the model search when it needs information.

A default stays a choice. A provider option that is fixed shows its source.

## Try it on a model

For a provider model that offers **Live** search, turn on **Live** without changing the ceiling of another model. In a thread that uses the model, choose **Live** and ask the agent to check the publisher's current release announcement. Look at its source links and at the search usage of the request. A search setting allows the tool; it does not prove that the model used it. If the vendor reports no count, **Usage** keeps it unknown. On the next use, choose **Disabled** when no web lookup is needed.

Search support depends on the endpoint and model. Sharing a wire format does not imply that every model supports every tool. DeepSeek documents hosted search for its Pro model through Responses; a model that ignores search is not proof that it ran. Google's OpenAI compatibility documentation lists grounding tools for its Images endpoint, without documenting Chat grounding. Gemini native grounding is available through Google Gemini Interactions and a native Gemini endpoint, such as `https://generativelanguage.googleapis.com/v1beta`. Its credential uses `x-goog-api-key`. Requests supply the admitted conversation with `store=false`; they retain signed native steps without using hidden remote history. Enable Live on the model and configure `search_types` with `web_search` or `image_search`. Existing function tools stay in the request. Only a reported `grounding_tool_count` supplies a search quantity; missing counts stay unknown.

Search fees can be additional to model tokens. Usage records keep a vendor-reported search count when available. A visible WebSearch tool call can contain several backend searches, so its presence is not a billable count. A missing count is unknown. A matching vendor-reported total can replace a catalogue estimate. An explicit provider or rate-card price in Orch credits stays authoritative; the reported total remains beside it for comparison.

Native web search finds public web information. Orch's `search_shared` tool searches documents, chats and threads the agent may read. They are separate capabilities.

For upstream details, see [OpenAI web search](https://developers.openai.com/api/docs/guides/tools-web-search), [Anthropic web search](https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool), [DeepSeek Responses](https://api-docs.deepseek.com/guides/responses_api/) and [Gemini grounding](https://ai.google.dev/gemini-api/docs/google-search).

## Configure the hosted tool

Turn on the modes on the model of the provider first. The request defaults of an endpoint can then configure the hosted search tool, under the name `web_search`:

- **Responses** supports the size of the search context, filters by domain, an approximate location, and live access through the mode chosen.
- **Messages** supports a limit on searches, allowed or blocked domains, and an approximate location.

These options configure the tool and keep the other tools of the request. For a Responses endpoint, a request default can read:

```json
{"web_search":{"mode":"live","search_context_size":"low","filters":{"allowed_domains":["example.com"]}}}
```

For Messages, use its own options:

```json
{"web_search":{"mode":"live","max_uses":3,"allowed_domains":["example.com"]}}
```

An allow list and a block list cannot be combined. A **Disabled** choice for one use overrides a default. The defaults of an endpoint configure a tool that an agent already selected; they do not turn on a tool that the agent program left out. Summaries and goal checks take their chosen settings through the same limits of the provider's model.

## Set a search price

The rate of the model and the rate card accept **Credits per web search**. Enter the price in the same credit unit as the token rates. In US dollars, the documented basic fee for a search is `0.01` for OpenAI and for Anthropic, and `0.00075` for a successful search with DeepSeek. The fee for a search does not get the multiplier of the service tier that applies to tokens. Search tokens can still carry the token charge of the model.

Counts and prices are recorded with each request. A request with search turned on and a missing count or price has a total charge that is unknown. A cumulative report that is repeated is not an additional use. These are estimates from the settings; they are not an invoice.

A search engine that was chosen on purpose can have a fee for each request and an extra fee for results that are charged. Orch keeps those declared terms with the real request. The number of results asked for and the links of citations that are shown are not measured as charged results. If a count that is needed is missing, the full estimate stays unknown. When the engine is chosen automatically, or is the built-in one, the price depends on the provider unless a total is reported. A matching total from the vendor replaces only catalogue estimates; a price in credits that you set for the provider or on the rate card stays in force.

**What counts as a search.**

Search fees can come on top of the tokens of the model. A **WebSearch** tool call that you see can hold several searches in the background, so seeing one does not give the number that is billed. A missing count is unknown. A vendor's total that matches can replace a catalogue estimate, and a price in credits that you set stays in force, with the reported total beside it for comparison.

- **With a direct sign-in,** the quantities of built-in search are reported by model at the end of a turn. Orch records the increase once, deducts quantities already recorded on real requests, and applies the unit price you set apart from token prices and speeds. The first cumulative total of a conversation that resumes sets the starting point when its earlier total is unknown, and missing counts stay unknown. Connections that are forwarded keep the meter of the forwarder.
- **With Codex,** a standalone web action proves usage only through a successful result that was kept. Search actions count as web searches. Opening a page or finding text counts as a page fetch. An empty search result can still be a successful call. The end event of the program and the counter of its analytics do not prove success. Attempts that failed or did not match are shown without being counted or charged, and missing evidence stays unknown. Orch reads results with the verified build of the program and does not guess from the text of messages. A replay or an adoption shows historical results and never bills them again.
- **The web action failed** (`native_web_action_failed`) means the result that was kept reported failure. **The web action could not be verified** (`native_web_action_unproven`) means the result, the original owner, the model or the supported format could not be matched. The action keeps its query, its page and its raw source, and a text turn that was otherwise completed stays completed. Open **Full output** under the raw view of the action to read its result and the results reported.
- **In Usage → Requests,** **Built-in capability usage** names these totals and links to their original turn. They add no model request, no token charge, no attempt and no delay. A zero that was reported stays zero, and a share of tokens of the model that was not reported says **Not measured**. A unit price that is unknown stays unpriced.

## Capability routes

Open **Parameters** of a thread and read **Capabilities**. Each row comes from the same declared support and current settings that the server uses: a tool of the model, a tool of the program, a service of a provider that was chosen, or a session of another program. A row that is unavailable states the current reason, such as a model that is turned off, a mode that is not supported, access that is missing, or a service that was not chosen. If the program does not report its default, the row says **Program default**.

A provider and the quote for each call appear only when you may read them. The quote keeps its declared currency and unit, and a missing price is unknown. A preview of a route reserves no usage and allows no later call: the current access, the settings of the provider and the usage limits are checked when a call runs. Choosing the backend of a provider does not turn on a paid service. Keep the capability at **Program default** to keep the current behavior of the program, or choose the service on purpose where one is offered.

For example, open **Parameters** on a thread whose search uses the service of a provider. The **Web search** row names that provider and the price it declared for each search. If access is taken away, the same row becomes unavailable with the reason. Orch never picks another provider quietly. A capability that a later extension supplies appears in the same list from its declaration.

Managed search and page fetch use the call rules of the place. A retry that is configured keeps the original deadline of the tool. A fallback that is configured checks access, the limits of the offer and the caps again for each candidate named. A candidate that cannot be used is skipped, and one that fails for another cause continues the same chain. A recovery that succeeds records its reason and its rule in the thread and does not change the backend configured. Each attempt records only the usage it really measured. A wait for a person, or a reset beyond the deadline of the tool, returns a paused result. Use the recovery action of the thread, or wait for the reset reported, before the next call of the agent.

**The thread's view of Claude Code.** The **Parameters** view of a thread shows the access facts of the chosen program beside each capability. The forwarded path of Claude Code leaves out its beta options for the host only, the keep-alive of its cache, its catalogue of hosted models, and the citation and mode options of its built-in WebSearch. This does not turn off ordinary forwarded generation, and it does not change the cache lifetime chosen for the thread. In the **Separated** arrangement, its built-in WebFetch reads pages from the container of the provider, and the thread shows that place. These are explanations of the current route. They are not extra permissions or tools.

## Page fetch

**Page fetch** is independent of **Web search**.

- **Unset** keeps the page reader that the program has now.
- **Off** turns this capability off.
- **Built in** uses the page reader that the chosen program or model declares.
- **Service** uses the fetch service you chose and turns off its declared built-in counterpart from the next session.

Turning on managed **Web search** does not turn on managed page fetching.

At the organization, **Web search backend** chooses the provider and model, or a search service and a fetch service of that provider. The fetch service of an API backend is optional. A backend without fetch support stays unavailable for page fetching, and Orch does not imitate it by returning snippets of search results. A manager of the provider turns on each service in the **Settings** of the provider and can set its groups and its exact price in credits. When the call runs, the agent needs current **Use**, every policy of offers that applies, and room under the limits.

For example, leave **Web search** on its chosen mode, open the settings of the place, set **Page fetch** to **Service** and save. Start a new session to get the changed set of tools. A page that is read shows its real address and the content returned, in the same timeline card as the pages that built-in tools open. **Usage** records the purpose of page fetch, the real backend and the pages charged that were reported. It does not count the links of search results as pages. Turning **Page fetch** to **Off** prevents another paid fetch even if an earlier session still lists the old tool.

## Codex endpoint support

Some Codex models make a separate search request of their own. Such models need an endpoint that declares this support, and a model with search turned on. **Cached**, **Indexed** and **Live** appear only when the current resolution offers them. An endpoint without the tool that is needed can use a search service you chose. Keeping **Program default** does not choose a paid service.

A standalone search that succeeds counts as one search operation. Opening a page, or finding text in a page, is a separate page operation. Several queries or source links do not create extra counts. Failed or unproven replies stay uncharged for these quantities. A reply that only searched supplies no measure of generated tokens.

The usual **Web search** control of Codex can offer **Service** when the main model has no built-in search and a managed backend is available. Opening the control, or leaving it unset, does not choose a paid service. Choosing **Service** turns built-in search off and applies when the session opens again. The key `web_search` in the advanced settings of an agent profile stays an option of the program itself: use **Disabled**, **Cached**, **Indexed** or **Live** within the limits of the provider, and choose a managed service through the usual control of the thread.

## Codex model details

Forwarded Codex sessions take their image input, their context window and their speeds that are turned on from the entry of the model in the provider. The window of the model itself and a larger context opened by its manager remain separate. Concurrent sessions keep their own model details. A change is accepted where a session opens, and work already running keeps its settings.

A model that takes text only receives an attached image as a file, with its path in the message, and its tool for viewing images stays off. A model whose name resembles a bundled Codex model gets a request dialect of its own only when the endpoint declares that support.
