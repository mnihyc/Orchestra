---
group: providers
screens: [/resources/providers, /resources/providers/$id, /thread/$threadId, /usage]
keywords: [web search, native search, grounding, search fees, public web, cached, live, DeepSeek]
summary: Enable a provider’s supported native search modes and set their price.
---

# Provider web search

The thread's Parameters view shows the selected program's access facts beside
each capability. Claude Code's forwarded path omits its host-only beta options,
cache keepalive, hosted model catalogue and native WebSearch citation/mode
options. This does not disable ordinary forwarded generation or change the
thread's chosen cache lifetime. In the Separated arrangement, its native
WebFetch reads pages from the provider runtime; the thread shows that location.
These are explanations of the current route, not additional permissions or tools.

For direct sign-ins, native search quantities are reported by model at turn
boundaries. Orch records the increase once, deducts quantities already recorded
on real requests, and applies the configured unit price separately from token
prices and processing speeds. A resumed conversation's first cumulative total
establishes a baseline when its earlier total is unknown; missing counts stay
unknown. Forwarded connections keep the forwarder's meter.

For Codex standalone web actions, only a successful retained native result proves
usage. Search actions count as web searches; opening a page or finding text counts
as page fetches. An empty search result can still be a successful call. The
program's end event and analytics counter alone do not prove success. Failed or
unmatched attempts are shown without counting or charging them; missing evidence
stays unknown. The decoder uses the verified native build, not guesses from its
message text. Replay and adoption display historical results without billing them
again.

**The web action failed** (`native_web_action_failed`) means the retained result
reported failure. **The web action could not be verified**
(`native_web_action_unproven`) means the result, original owner, model or supported
format could not be matched. The action keeps its reported query, page and raw
source, and an otherwise completed text turn stays completed. Open **Full output**
under the action's raw view to inspect its native result and reported results.

A Codex web action closes when the program reports that action's end, even while
the answer continues. Missing usage evidence adds an unverified notice and leaves
the reported result intact. A retained result that proves failure marks the action
failed. Usage remains unknown until the result is proved.

Hosted Codex searches whose retained result cannot be matched keep their reported
completion and show the unverified notice. Their native search quantity remains
unknown. Source links and a successful answer do not establish that quantity.

Reading native result evidence has a separate short wait so a slow history file
does not hold up other sessions. **Native result wait**
(`agents.native_result_wait_ms`) starts at 2000 milliseconds in system settings;
program profiles can choose their own wait within the engine's interrupt window.
An expired read adds an unverified notice and keeps the action's reported result.

**Web action wait** (`agents.web_action_wait_secs`) closes a native action that
has not reported its end, with a notice explaining the wait. It starts at 120
seconds, can be changed in the place's settings, and applies when the session
opens again. This closes the action's row while the turn continues; a later result
can still update that row. An action closed this way supplies no usage count.


In **Usage → Requests**, **Native capability usage** identifies these aggregate
quantities and links to their original turn. They add no model request, token
charge, attempt or latency. A reported zero stays zero; a model's unreported
token share says **Not measured**. An unknown unit price stays unpriced.

A provider can make its model's native web search available to agents, summaries and goal checks. Provider managers decide which search modes are available and which mode starts a new use. Offering support does not switch it on.

On the provider's Models page, enable the supported modes and select the default. A thread then chooses its mode in Parameters, within the provider's limits. **Disabled** removes the hosted search tool. **Cached** uses indexed content without live retrieval where the protocol supports it. **Live** allows the model to search when it needs information. A default remains a choice; a fixed provider option shows its source.

Endpoint request defaults can configure the native search tool through `options.params.web_search`. Responses supports search context size, domain filters, approximate location and live-access control through the selected mode. Messages supports a search limit, allowed or blocked domains and approximate location. Those options configure the hosted tool while keeping the request's existing tools.

Search support depends on the endpoint and model. Sharing a wire format does not imply that every model supports every tool. DeepSeek documents hosted search for its Pro model through Responses; a model that ignores search is not proof that it ran. Google's OpenAI compatibility documentation lists grounding tools for its Images endpoint, without documenting Chat grounding. Google Search grounding needs its native protocol or a translator that declares it.

Search fees can be additional to model tokens. Usage records keep a vendor-reported search count when available. A visible WebSearch tool call can contain several backend searches, so its presence is not a billable count. A missing count is unknown. A matching vendor-reported total can replace a catalogue estimate. An explicit provider or rate-card price in Orch credits stays authoritative; the reported total remains beside it for comparison.

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

An explicitly selected search engine can have a fee per request and an additional fee for charged results. Orch keeps those declared terms with the actual request. Requested result limits and displayed citation links are not measured charged results. If a required count is missing, the complete estimate remains unknown. Automatic and native engine choices keep a provider-dependent price unless a total is reported. Matching reported totals replace only catalogue estimates; explicit provider and rate-card credit prices keep their authority.

## Where

Open [Providers](/resources/providers), select the provider, then its Models and options. A provider manager enables the supported modes and sets their default there. Open a thread's Parameters to choose among the offered modes for that use. [Usage](/usage) shows the reported searches and configured charge alongside other request usage.

## Example

For a provider model that offers Live search, enable Live without changing another model's ceiling. In a thread using that model, select Live and ask the agent to check the publisher's current release announcement. Inspect its source links and the request's actual search usage. A search setting is permission to use the tool; it does not prove the model used it. If the vendor does not report a count, Usage keeps it unknown. Choose Disabled on the next use when no web lookup is needed.

## Capability routes

Open a thread's Parameters view and read **Capabilities**. Each row comes from the same declared support and current settings used by the server: a model tool, the program's tool, a chosen provider service or a delegated session. An unavailable row states the current reason, such as a disabled model, an unsupported mode, missing access or a service that has not been chosen. If the program does not report its default, the row says **Program default**.

A provider and its per-call quote appear only when you may read them. The quote keeps its declared currency and unit; a missing price is unknown. A route preview does not reserve usage or authorize a later call: current access, provider settings and usage caps are checked when a call runs. Choosing a provider backend alone does not turn on a paid service. Keep the capability at Program default to retain the program's current behavior, or explicitly choose the service where one is offered.

For example, open Parameters on a thread whose search uses a provider service. The Web search row names that provider and its declared price per search. If access is removed, the same row becomes unavailable with the reason. Orch does not silently pick another provider. A capability supplied by a later extension appears in the same list from its declaration.

Managed search and page fetch use the place's existing call rules. A configured retry keeps the original tool deadline. A configured fallback checks access, offering limits and caps again for each explicitly named candidate; an unusable candidate is skipped, and a candidate that fails differently continues the same chain. Successful recovery records its reason and rule in the thread without changing the configured backend. Each attempt records only its actual measured usage. A wait for a person or a reset beyond the tool deadline returns a paused result; use the thread's existing recovery action, or wait for the reported reset, before the agent's next call.


## Page fetch

**Page fetch** (`agents.page_fetch`) is independent of Web search. Unset keeps the program's current page reader. **Off** disables this capability. **Native** uses the selected program or model's declared page reader. **Service** uses the explicitly chosen fetch offering and switches off its declared native counterpart for the next session. Enabling managed Web search does not enable managed page fetching.

At the organization, **Web search backend** selects the provider and model, or separate search and fetch services on that provider. An API backend's fetch service is optional; a backend without fetch support stays unavailable for page fetching. Orch does not emulate it by returning search snippets. A provider manager enables each service in the provider's Settings and may set its groups and exact credit price. The agent needs current Use, all applying offering policies and cap room when the call runs.

For example, leave Web search on its selected mode, open the place's settings and set Page fetch to Service, then save. Start a new session to use its changed tool set. A page read shows its actual address and returned content in the same timeline card as native page opens. Usage records the page-fetch purpose, actual backend and reported charged pages; it does not count search result links as pages. Turning Page fetch Off prevents another paid fetch even if an earlier session still advertises the old tool.

## Codex endpoint support

Some Codex models use a separate native search request. Those models need an endpoint that declares this support as well as a model with search enabled. Cached, Indexed and Live appear only when the current resolution offers them. An endpoint without the required native tool can use an explicitly chosen search service. Keeping Program default does not choose a paid service.

A successful standalone search is counted as one search operation; opening a page or finding text in a page is a separate page operation. Multiple queries or source links do not create extra operation counts. Failed or unproven replies remain uncharged for those quantities. A search-only reply supplies no generation token measurement.

Codex's ordinary Web search control can offer Service when the primary model has no native search, if a managed backend is available. Opening the control or leaving it unset does not select a paid service. Choosing Service disables native search and applies when the session opens again. The literal `web_search` key in an advanced agent profile remains a native SDK option: use Disabled, Cached, Indexed or Live within the provider's limits, and choose a managed service through the ordinary thread control.


## Codex model details

Forwarded Codex sessions take their image input, context window and enabled speeds from the provider's model entry. The model's own window and a larger context opened by its manager remain separate. Concurrent sessions retain their own model details; changes are accepted through the existing session-opening boundary while earlier running work keeps its settings.

A text-only model receives an attached image as a file, with its path in the message. Its native image-view tool stays off. Model names resembling a bundled Codex model acquire a native request dialect only when the endpoint declares that support.
