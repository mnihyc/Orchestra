---
group: providers
title: Choose and read provider details
screens: /thread/$threadId, /resources/providers, /resources/providers/$id, /resources/machines, /resources/templates
keywords: Relay, SDK, provider.relay, how do I choose provider details, facts source, models source, SourceChoice, balances, spend, reset, Not read yet, None reported, Does not report, Unencrypted, providers.allow_http_upstreams, Last program error, interrupted
summary: Choose where a provider's details come from, read them with their source and missing-data states, and use a reset.
---

# Choose and read provider details

Choose **Google Gemini** in Add provider and enter a Gemini Developer API key. Discover models to see the names available to that key. The native Interactions endpoint uses the key header and supports stateless text, function calls and Google Search. The second endpoint is available to clients that speak OpenAI Chat Completions.

For a **Google Antigravity subscription**, install Google’s official `agy` on the machine that owns the workspace and register that executable in an Antigravity agent profile. Once the connector has probed it, choose that profile under **Subscription** in Add provider and choose **Sign in with Google**. Open the Google page and paste its one-time authorization code into Orch. The CLI keeps its Google sign-in in this provider’s isolated home. Discover models and select one of the returned IDs for a thread on that machine. A Gemini API key is a separate API provider. Google documents this personal CLI flow in its [ACP guide](https://antigravity.google/docs/ide/extensions/acp/).

A provider has separate choices for its credential, its model access, its details, its models and its container. A source can report windows, a remaining balance, spending and resets for a key, as well as for a sign-in of an agent program. The card says **Details from <source>** and shows only what that source and the keeper of the credential know. Its money is in the provider's own unit. **Orch counted** is the separate ledger of tokens and price tables.

A delegated capability's helper link has a collapsed **Subscription** entry. Open it to read the selected sign-in's current reported windows, reset times and observation age. This reads cached facts under your own provider access; it starts no program or paid request. Unreadable facts stay hidden and unreported use stays unknown. These account-wide observations do not turn a helper result into an API price or a per-job percentage.

## Choose a source

**Add provider** starts with the interface: an address and its API format, or the sign-in of an agent program. Then you give the credential and the machine that reaches it. The examples fill the same fields. Keys are added afterwards, in **Sign-in and keys**. Creating a provider does not wait for its container to answer. Its page shows the saved setup while the live connection brings in its observations.

**Request defaults.** For an API endpoint, **Advanced → Request defaults** takes more options for the body as JSON.

- The values in the request itself win. Orch keeps control of the model, the prompt, the tools, the output budget and the credentials.
- Fixed headers, secret headers and query parameters keep their own fields.
- A custom option is configuration, not proof that the provider supports it.
- A summarizer has the same kind of options. The defaults of its endpoint apply first, then the defaults set on its model, and last the summarizer's own options.
- For OpenAI Responses, turn on the model's speed with the ID `priority` and the label **Fast**, and choose that ID for its default, its request options, its rates and its limits. The ID `fast` is the documented alias. Usage keeps the `priority` actually reported apart from what was requested.
- Anthropic Messages uses `speed: "fast"`, with the beta header it needs in the headers of the endpoint. Gemini, DeepSeek and other compatible endpoints use their documented fields. Choosing a format does not give a model a speed it does not offer.

Gemini's [OpenAI-compatible API](https://ai.google.dev/gemini-api/docs/openai)
uses **openai-chat**. Its documented Priority and Flex tiers use the IDs `priority`
and `flex`; enable the tiers the model offers and choose its default in the same
model form. Effort uses `reasoning_effort`. Gemini-specific options, such as
`extra_body.google.cached_content`, use the same nested request-option rules,
defaults and per-use form as any other declared option.

DeepSeek's [Chat Completions API](https://api-docs.deepseek.com/api/create-chat-completion/)
also uses **openai-chat**. Its documented effort and `thinking.type` options can
have model defaults and per-use overrides. Its API does not declare an inference
tier selector: a model rule with `service_tier: false` leaves that field out.
There is no need to invent a Fast tier. New documented fields use the existing
request-option schema and its `default` values; endpoint defaults, model defaults
and explicit use choices keep the same precedence for every provider.

In the provider’s **Settings**, open **Where its details come from**. Facts and models each have a source choice.

- **Default** is the reader that fits: the reader of the agent program for a sign-in; the known reader for OpenRouter or DeepSeek when they take a key (each reads with it); otherwise no optional details. Models normally come from the lists of the endpoints.
- **Program** chooses a program already registered on the machine that keeps the credential, or carried by its trusted template. Its fields are its own declarations. A secret field shows **Value set; enter a new one to replace it** and never its value. Choosing a program installs nothing and never exports a sign-in. A managed template must carry the program and its interpreter.
- **Address** chooses the URL of the reporting service and how it is authorized: **None**, **Bearer token** or **Header named …**. The authorization value can be written but never read. For example, `https://plan.acme.example/provider` for the made-up Acme source. **Send this provider's key to this address** starts off and asks you to confirm the host before a key is sent. It is not available for a sign-in. The token of the service is independent of the key of the provider.
- **Nothing** keeps only the details of the keeper of the credential. For models, **Manual only** lets a manager enter names for routing without discovery.

An address that starts with `http://` is refused unless **Allow unencrypted upstreams** allows it here; it is then marked **Unencrypted**. For example, an internal service such as `http://plan.internal:41870/provider` works only with that policy on. The calls leave the machine that keeps the credential, so the allowed hosts of that machine's network, or its own list of upstreams, must allow the host too. Orch follows no redirect. A listed origin permits its path and descendants on complete path boundaries, with the same scheme, host and port. For example, `https://openrouter.ai` permits its `/api/v1` path.

**Save** reads the declaration of the source and its first result. Its supported members, or a safe error, appear beside the choice. The **Add provider** form shows the same choices with the same defaults. **Add a source program…** uses the registration of programs of the keeping machine inline, and the declaration it returns supplies the fields. See [Build your own provider source](../development/extensions/providers/README.md).

The page of a provider has six sections: **Overview**, **Models**, **Sign-in and keys**, **Usage**, **Settings** and **Activity**. The details are a compact list of labels and values with the time each was read. Members not read yet share one **Not read yet** line and **Refresh**. Members the source does not supply share one **Not supplied** line below the reported details, and its heading names the source. A missing admin key keeps its own explanation, and a detail that was never read is never a guessed zero. Figures of lifetime use and of recent use say what they count, such as turns. **Delete** stays in the menu of the page, and it is available even when the saved details cannot be shown. The usual checks of access and of use still apply.

## Request options

On **Sign-in and keys → Endpoints → Advanced**, **Allowed request options** sets rules, in JSON Schema, by interface. On **Models**, use the **Options…** of a group to set the same rules for every model in that group. The **Edit options…** of a model keeps the rules of that model.

The form of a field comes first from the rule of the model, then from the last group in the order saved for the model, then from the earlier groups, then from the endpoint. Every layer still limits the final request. For example, an endpoint allows a temperature from 0 to 1, a group limits it to 0.5 and a model starts at 0.25: the default is 0.25, and a request for 0.75 is refused. A model's more permissive form cannot remove the limit of its group. This holds for the calls an agent program forwards and for the model calls Orch makes itself. A change is used for the next turn or call; a call that is running keeps the settings it started with.

A setting of the home sets how many groups a model may join there. It starts at 8 and can be raised. Lowering it leaves existing memberships alone, and models can still be edited or taken out of groups. Setting it to 0 allows no new memberships. The **Add model** form and the editor of a model use the same setting.

To set up a shared rule, put the common rules of the endpoint in **Advanced**, add **frontier** to the models it concerns, then open **frontier → Options…** and set the rules of that group. Leave the own rules of a model empty to use the shared form. Saving one group keeps the others. Malformed JSON, or a revision that is out of date, keeps the draft open with the error.

## Thread options

Open **Parameters → Request options** of a thread. A thread on a forwarded API shows the fields its provider opened, grouped by interface. Choose an enum from its list. A fixed value is read-only. The sign-ins of agent programs keep the controls of the program. A field that was never declared is not a choice of the thread.

The values of each interface use the **agent parameters** setting, with the same source, lock and revision. Reset that row to follow the choice of the place again. Other parameters and interfaces keep their choices. A change that is refused keeps the draft and shows the error. The permission to manage the thread, and any lock above, still decide whether the controls can be changed.

Choices apply from the **next turn**. A turn that is running keeps the settings it started with, also after a restart of the connection or the provider's container. The request that goes out uses your choices after translation, and it must still fit every rule of the endpoint, the group and the model. For example, a verbosity chosen from the list of the provider can replace the value of the program, but it cannot override a fixed region or turn on an option the provider closed. Switching the model can mean resetting choices that its new entry does not open.

Codex's **Access** choice also works on a forwarded API endpoint when the
interface supports it and a provider manager enabled the program for the model.
Choose it from the composer's **Access** control, which stands beside **Model** and **Effort** where the model has a program switched on. The choice applies to
the next turn and stays with that turn's lease across retries and restarts.
Ordinary access adds no program selector to a Responses request. Programs merely
reported by the provider remain unavailable until a manager enables them.

A speed, an access program or a context size that a manager switched on for the
model, and that the thread cannot use, still has its control, shown off. It
says why in a few words: the agent does not offer it, the credential does not
stay with the provider, the environment or the provider is of the other kind, the
provider's rule for the model leaves it out, or the **Agent features** setting
has its feature off. Where you may lift the reason, the control also offers the
act that does, such as **Switch on in Agent features**.

## Confirmed values

In **Models**, **Reported values** opens a small read-only card on a saved model or a suggestion. Each value says **From the vendor** or **From catalogue**. Only a value the vendor really supplied gets the first label. A catalogue mapping stays a proposal, and a source that is unknown or older says **Source not confirmed**. The card keeps the time of the observation of that model, with **Time not reported** when there was none. A newer balance or plan does not date an old report of a model.

- **Vendor differs** names the fields whose reported value differs from the saved proposal or choice. The card shows the reported value. The saved entry and your overrides stay in force until you edit them.
- Reading or refreshing never turns on a model, a speed, a search or a paid service.
- A reported maximum context describes what is available. It is separate from the context a manager opened to threads.
- Prices keep the decimals the source reported. A missing value is not zero.

A reported **Family** can offer **Add … group…** beside the model. It opens the editor of the model with that membership proposed and its other groups kept. Review it, edit it or remove it, then choose **Save**; **Cancel** changes nothing. Only a family the vendor supplied that already fits the rules for group names is proposed. No group is guessed from the name of a model. A new report never changes membership. The allowance of groups still applies, zero included. Remove another membership or the proposal when you need room. An open draft keeps the family you chose even if a later observation names another.

## Read the four states

| The card says | What it means |
|---|---|
| Not supplied by this source | The source never declared that member and never reported it. |
| Not read yet | It declared support, but there is no successful observation. |
| None reported | It reported null or an empty list on purpose. |
| A value and a time | It reported that value at that time. Zero is a value only when it was reported. |

For example, **12.50 USD left** is a reported balance. A later read that leaves balances out keeps that amount with its old time. A read that reports no balances on purpose clears it. New windows do not date old money as new. A source error keeps the old values, their times and a safe line, **Could not refresh · Retry**. A details query that failed never turns a working credential for models into a failed authorization.

**Refresh** reads facts and model offers now. New models are suggestions for a manager to add.
The organization's **Switched on when added** setting controls their starting choices. **Context, speeds and access** enables the reported context sizes, speeds and access programs; **Efforts and context** enables context sizes; **Only what it states** keeps the model's own window and efforts. Web search, cache lifetimes, reasoning modes, capacity tiers and request options need separate configuration. Larger contexts remain a per-thread choice.
When no window is entered, Orch uses the source's largest reported context if it fits the model's enabled context limit. If neither the catalogue nor the endpoint supplies a usable value, enter a window before saving. Orch also
reads a minute after a vendor-reported reset. Its compact provider
caption uses the binding windows, else a first reported balance, else
a first spend row. Without permitted facts it cannot invent room.

In **Models → Edit options… → Model report**, read the endpoint's reported
input and output windows, capability support, model line and retirement
schedule. A scheduled date is not a statement that a model has retired.
Missing facts stay absent, and a new report does not switch on search,
larger contexts or other paid options for a model that is already saved. Saving the model keeps these facts
separate from the options a manager enabled.

## Program exits

In **Settings**, which a manager of the provider sees, **Agent program processes** shows the current status of each agent profile in use. **Last program error** keeps the latest exit that was not expected and its time, with the reason of the program when there is one. It stays visible after the profile is online again. A turn that was running is marked interrupted, and its conversation records the reason. The next turn uses the choice of the thread for recovering history.

## Plan forecasts

The meter of each window shows a forecast once there are enough recent readings. **Over time**, beside **Windows**, opens the **Usage** tab of the provider at **Plan**. The solid line is measured use, and the dashed line continues at the pace seen. **History** shows the usage added in each interval and marks early resets. See [Plan windows and forecasts](settings-and-usage.md#plan-windows-and-forecasts).

When its details are private, **Usage** stays visible with **Usage details unavailable for this view**. A source you can read that has no plan or history says **Plan and history not supplied by this source**. A source that needs an admin key keeps that reason instead.

## Use a reset

A source of details can list resets and declare that it can use them up. Only a current manager of the provider, in a browser, gets **Use a reset**. The dialog shows the exact reset, how many there are, how many remain and the effects that are reported. **Cancel** is the default; choose **Use this reset** on purpose. No model is used, and the limits of Orch stay as they were. If the reply is lost, the request keeps the same identity, so a repeated check spends the reset once.

A limit the source reports can also hold the work of a provider on a key. A reset that succeeds, or a later observation that lets work in, releases that hold under the usual permissions and limits.

## Build your own

The [provider source overview](../development/extensions/providers/README.md) and the pages on [details](../development/extensions/providers/facts.md), [models](../development/extensions/providers/models.md), [resets](../development/extensions/providers/resets.md) and [how a key is presented](../development/extensions/providers/credential.md) are in the **Build your own** section of Help. They hold the generated shapes, bounds and errors, and the exact Python program and address examples that the conformance checks use. A manager of the provider chooses code that already exists. The person who writes for the machine or the trusted template registers it.

## Request option rules

On **Models**, open the editor of a model and choose a **Request format**. Documented fields have compact controls for the rule: **No explicit rule**, **Constrain** or **Block**.

- Choose the allowed values without choosing a default; a default is a separate choice.
- Rules for identifiers can fix text that the provider wrote, or strip a value.
- These rules narrow the limits of the endpoint and the model. A tier in the documentation is not turned on just because the catalogue lists it.
- **Raw JSON** keeps custom fields, the rules of agent programs and schema keywords it does not know. A draft that is malformed stays visible until you correct it. A custom parent schema stays raw instead of being replaced by a compact control. A property with no explicit rule can still be blocked by a closed parent schema or by another layer.
- Endpoints show the same controls when their format declares request options. Fixed public headers use the header map, and secret headers keep their separate control. **Advanced** keeps other names and values. Defaults for the body stay apart from schema rules.

The catalogue follows the request formats, including [Responses](https://developers.openai.com/api/reference/resources/responses/methods/create), [Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create), [Messages](https://platform.claude.com/docs/en/api/messages/create) and the extensions you configure for an endpoint. A field that is documented does not prove that a model supports it.

## Service backends

In **Settings → Services** of an API provider, Orch lists the services declared for its saved endpoints. Discovery leaves every service turned off. **Edit service** lets a manager of the provider turn one on, keep its declared groups or choose groups of services, and save an optional price in Orch credits. Saving these choices starts no search, no page read and no model request, and chooses no backend for any thread. A service that is unavailable on its runtime keeps its cause beside the control.

Catalogue prices are estimates in their original currency. A price in Orch credits that you saved is what Orch counts, and the total reported by the vendor stays beside it for comparison. Clearing the custom price brings back the catalogue estimate. Editing keeps the other services and later fields of the configuration. A provider revision that is out of date is refused, so that the change of another manager is not overwritten.

A thread uses a paid backend only after its capability is set to **Service** on purpose and its backend names a service that is turned on. The **Use** of the provider, the allowed lists of services and models, and the limits by purpose apply to the original person, or to the program that began the work, on every request. The list of models and the list of services of a provider stay independent. **Unset** keeps the default of the program and chooses no paid fallback.

## Direct model API calls

A key provider’s **Settings → Relay** switch starts off. Open exact enabled models in its model picker, then enable Relay. A separate **Relay** permission grants direct API access with the same complete model, effort and arrangement options as Use; ordinary Can use does not grant it. A standalone SDK call has no attested machine or environment, so a location-restricted grant does not apply. Subscriptions continue through normal threads.

Copy **SDK base** into the SDK’s base URL and authenticate with an Orch API key. For an OpenAI-compatible SDK this is its API key; for a Messages SDK use its bearer/auth-token option. The provider’s model key stays on its connection’s keeper. The base serves `/responses`, `/responses/compact`, `/responses/input_tokens`, `/chat/completions`, `/embeddings`, and `/v1/messages` with `/v1/messages/count_tokens` and `/interactions`; model listings show only opened models the caller may use. These requests do not refresh provider observations.

Calls stream through the existing native forwarder. The same endpoint, group and model option declarations constrain them before an upstream request. Unopened models, undeclared options, exhausted caps, an unavailable keeper or a disabled Relay refuse a call with their actual reason. Relay usage is counted for the caller under **Usage → Relay**, with origin Orch; a cap whose purpose list is empty includes it. The public request never carries the provider’s credential and request or response content is not retained as server files.

System settings `providers.relay_concurrency`, `providers.relay_request_bytes`, `providers.relay_idle_secs` and `providers.relay_retry_ms` control admission, request size, silence and transport retries. `jobs.io_timeout_secs` supplies the call’s overall native work allowance. A disconnected reader ends its command. Replaying an SDK request creates a new call; Orch never silently repeats a possibly paid native call after a runtime crash.


The **Common** request format declares cache and schema-output rules once in `request_options.common`, through the same endpoint, model-group and model layers. SDK callers keep using their interface’s native fields. An explicit thread choice uses `provider_options:common` and wins when a format-specific choice names the same value; every original schema and enabled ceiling still applies. A format that cannot represent a member or retention says which one before model work starts. A nested schema-name default alone does not enable schema output. New wire implementations supply their feature bindings through the same protocol contract.
