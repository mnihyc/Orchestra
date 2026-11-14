---
group: providers
title: Choose and read provider details
screens: /thread/$threadId, /resources/providers, /resources/providers/$id, /resources/machines, /resources/templates
keywords: how do I choose provider details, facts source, models source, SourceChoice, balances, spend, reset, Not read yet, None reported, Does not report, Unencrypted, providers.allow_http_upstreams, Last program error, interrupted
summary: Read provider observations with their source, missing-data state and reset options.
---

# Choose and read provider details

The provider's credential, its model access, details, models and runtime
are separate choices. A source can report windows, a remaining balance,
spending and resets for a key as well as a native sign-in. The card says
**Details from <source>** and shows only what that source and the
credential's keeper actually know. Its money is the vendor's unit;
**Orch counted** is the separate token/price-table ledger.

## Choose a source

**Add provider** starts with the interface: an address and its API format,
or a program's own sign-in, followed by the credential and the machine that
reaches it. Examples fill those same fields. Keys are added afterwards in
**Sign-in and keys**. Creating a provider does not wait for its runtime to
answer; its page shows the saved setup while the live connection brings its
observations.

For an API endpoint, **Advanced → Request defaults** accepts additional body options as JSON.
The request's explicit values win, and Orch keeps ownership of the model, prompt, tools,
output budget and credentials. Fixed and secret headers and query parameters keep their
existing fields. A custom option is configuration, not proof that the upstream supports it.

A summarizer has the same kind of options in `auto.summarizer.params`. Its endpoint defaults
apply first, its model's configured defaults next, and the summarizer's explicit options last.
For OpenAI Responses, enable the model's speed as ID `priority`, label **Fast**, and choose
that ID for its default, request options, rates and caps. Request ID `fast` is the documented
alias; usage keeps the actual reported `priority` separately from what was requested.
Anthropic Messages uses `speed: "fast"`, with any required beta header in the endpoint's
headers. Compatible Gemini, DeepSeek and other endpoints use their documented fields;
choosing a format does not grant a model a speed it does not offer.

In a provider's Settings, open **Where its details come from**. Facts
and Models have their own choices. **Default** explains the actual
reader: the native adapter for a sign-in, the known OpenRouter or
DeepSeek reader for those endpoints when they take a key (each reads
with it), otherwise no optional facts; models normally come from the
endpoints' own lists.

**Program** chooses a program already registered on the keeping machine
or carried by its trusted runtime template. Its fields are its own
declarations. Secret fields show **Set; replace**, never their value.
Changing this choice installs no program and never exports a native
sign-in. A managed template must carry the program and its interpreter.

**Address** chooses the reporting service's URL and authorization:
None, Bearer token or Header named … . The authorization value is
write-only. Use, for example, `https://plan.acme.example/provider` for
the made-up Acme source. **Send this provider's key to this address**
starts off and asks you to confirm the actual host before sending a
key. It is unavailable for a native sign-in. The service token is
independent of that provider key.

An `http://` address is refused unless **Allow unencrypted upstreams**
(`providers.allow_http_upstreams`) allows it here, then marked
**Unencrypted**. For example, an internal Acme service may be
`http://plan.internal:41870/provider` only with that policy enabled.
Calls leave the keeping machine, so its network allowlist or local
`provider.upstreams` must allow that host too. Orch follows no redirect.

**Nothing** keeps only the credential keeper's facts. **Manual only**
for models lets a manager enter routing names without discovery. Save
reads the source's declaration and first result. Its supported members
or safe error appear beside the choice. The Add provider wizard has
these same choices visible, with the same defaults. **Add a source program…**
uses the keeping machine's Programs registration inline; the returned
declaration supplies its fields. See [Build your own provider source](../development/extensions/providers/README.md).

The provider page has six sections: Overview, Models, Sign-in and keys,
Usage, Settings and Activity. Facts are a compact label/value list with their
read times. Unread members share one **Not read yet** line and **Refresh**.
Unsupported members share one **Not supplied** line below the reported facts;
the heading names their source. A missing admin key keeps its own explanation,
and an unread fact is never a guessed zero. Lifetime and recent use figures
name what they count, such as turns.
Delete stays in the page menu, and
is also available if its stored details cannot be shown. Its ordinary access
and in-use checks still apply.

## Request options

On **Sign-in and keys → Endpoints → Advanced**, **Allowed request options** sets
JSON Schema rules by interface. On **Models**, use a group's **Options…**
to set the same rules for every model in that existing group. A model's
own **Edit options…** keeps its individual rules.

The model's rule supplies a field's form first, then the last group in
its saved group order, then earlier groups, then the endpoint. Every
layer still limits the final request. For example, an endpoint allowing
temperature from 0 to 1, a group limiting it to 0.5 and a model starting
at 0.25 produce a 0.25 default; a request for 0.75 is refused. A model's
more permissive form cannot remove the group's limit. This applies to
forwarded program calls and Orch's own model calls. Changes are used
for the next turn or call; a current lease keeps its accepted snapshot.

`providers.model_groups_max` sets how many groups a model may join at
this home. It starts at 8 and can be raised. Reducing it leaves existing
memberships intact; models may still be edited or removed from groups.
Setting it to 0 permits no new memberships. The field uses this setting
in both **Add model** and the model's editor.

For a shared setup, put the endpoint's common rules in Advanced, add
**frontier** to the relevant models, then open **frontier → Options…**
and set that group's rules. Leave a model's own rules empty to use the
shared form. Saving one group preserves the others; malformed JSON or
a stale revision keeps the draft open with the error.

## Thread options

Open a thread's **Parameters → Request options**. A forwarded API thread shows
the fields its provider opened, grouped by interface. Choose an enum from its
list; a fixed value is read-only. Program sign-ins keep their program's native
controls. An undeclared field is not a thread choice.

Each interface's values use the existing **agent parameters** setting, with the
same source, lock and revision. Reset that row to follow the place's inherited
choice again; other parameters and interfaces keep their choices. A refused
change keeps the draft and shows the error. The permission to manage the thread
and any lock above still decide whether the controls may be changed.

Choices apply on the **next turn**. A running turn keeps its lease's values,
including after a connection or runtime restart. The actual outgoing interface
uses its choices after translation; the final request must still fit every
endpoint, group and model rule. For example, choosing a verbosity from the
provider's list can replace the program's value, but cannot override a fixed
region or enable an option the provider has closed. Switching the model may
require resetting choices that its new entry does not open.

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

In **Models**, **Reported values** opens a small read-only card on a saved model
or a suggestion. Each value says **From the vendor** or **From catalogue**.
Only an actual supplied vendor value receives the first label. A catalogue
mapping remains a proposal; an unknown or older source says **Source not confirmed**.
The card keeps the model observation's own time, with **Time not reported** when
none was supplied. A newer balance or plan read does not date an old model report.

**Vendor differs** names fields whose reported value differs from the saved
proposal or choice. The card shows the reported value; the saved entry and your
overrides stay in effect until you edit them. Reading or refreshing never enables
a model, a speed, search or a paid service. A reported maximum context describes
what is available; it is separate from the context a manager opened to threads.
Prices retain the source's reported decimal precision. Missing values are not zero.

A reported **Family** can offer **Add … group…** beside the model. It opens the
existing model editor with that membership proposed, keeping its other groups.
Review, edit or remove it, then choose **Save**; Cancel changes nothing. Only a
vendor-supplied family that already fits the group-name rules is proposed—no
group is guessed from a model's name. A new report never changes membership.
The configured group allowance still applies, including zero; remove another
membership or the proposal when needed. An open draft keeps the family you
chose even if a later observation names another one.

## Read the four states

| The card says | What it means |
|----------------|---------------|
| Not supplied by this source | The source neither declared nor ever told that member. |
| Not read yet | It declared support, but no successful observation exists. |
| None reported | It explicitly reported null or an empty list. |
| A value and time | It told that value at that time. Zero is a value only when actually reported. |

For example, **12.50 USD left** is a reported balance. A later read
that omits balances leaves that amount with its old time; a read that
explicitly reports no balances clears it. New windows do not date old
money as new. A source error retains old values, their times and a
safe **Could not refresh · Retry** line. A failed details query does
not turn a working model credential into an authorization failure.

**Refresh** reads facts and model offers now. New models are suggestions
for a manager to add. Adding one switches on what it offers (its efforts, its
context sizes, speeds and access programs), as the organization's **Switched on
when added** setting says: everything it offers, only efforts and context, or
only its window and efforts. A model with no window of its own takes the largest
context its source reports, and the model editor marks the window as required
where neither the catalogue nor the endpoint reports one. Orch also
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

In Settings, Runtime shows each agent profile's current status. **Last program
error** keeps the latest unexpected exit and its time, with the program's
reason when available. It remains visible after the profile is Online again.
A running turn is marked interrupted, and its conversation records the reason.
The next turn uses the thread's history-recovery choice.

## Plan forecasts

Each window's meter shows a forecast once enough recent readings exist. **Over
time** beside Windows opens the provider's Usage tab at Plan. Its solid line is
measured use; the dashed line continues at the observed pace. History shows
usage added per time bucket and marks early resets. See [Plan windows and
forecasts](settings-and-usage.md#plan-windows-and-forecasts).

Usage stays visible with **Usage details unavailable for this view** when
its details are private. A readable source with no plan or history says
**Plan and history not supplied by this source**. A source that needs an
admin key keeps that specific reason instead.

## Use a reset

A facts source may list resets and declare that it can consume them.
Only a current manager in a browser gets **Use a reset**. The dialog
shows the exact reset, current count, remaining count and reported
effects. Cancel is the default; choose **Use this reset** explicitly.
No model is used and Orch's own caps stay the same. A lost reply keeps
the same request identity so a repeated check spends once.

A limit reported by the source can hold a key provider's work too.
A successful reset or later admitting observation releases that vendor
hold under the existing work permissions and caps.

## Build your own

The [provider source overview](../development/extensions/providers/README.md)
and individual [facts](../development/extensions/providers/facts.md),
[models](../development/extensions/providers/models.md),
[resets](../development/extensions/providers/resets.md) and
[key presentation](../development/extensions/providers/credential.md)
pages ship in Help's **Build your own** section. They contain generated
shapes/bounds/errors and the exact Python program/address examples used
by the conformance suite. A provider manager selects existing code;
the machine or trusted template's writer registers it.

## Request option rules

On Models, open a model's editor and choose a Request format. Documented fields have compact rule controls: No explicit rule, Constrain or Block. Choose allowed values without selecting a default; a default is a separate choice. Identifier rules can fix provider-authored text or strip a value. These rules narrow the existing endpoint/model limits; a documented tier is not enabled merely by appearing in the catalogue.

Raw JSON keeps custom fields, native agent rules and unfamiliar schema keywords. A malformed draft stays visible until corrected. A custom parent schema remains raw rather than being replaced by a compact control. No explicit property rule may still be blocked by a closed parent schema or another layer.

Endpoints expose the same schema controls when their format declares Request options. Fixed public headers use the existing header map; secret headers keep their separate control. Advanced retains other names and values. Body defaults remain separate from schema rules.

The catalogue follows the request formats, including [Responses](https://developers.openai.com/api/reference/resources/responses/methods/create), [Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create), [Messages](https://platform.claude.com/docs/en/api/messages/create) and configured endpoint extensions. A field's presence in documentation does not establish model support.


## Service backends

In an API provider's **Settings → Services**, Orch lists services declared for its saved endpoints. Discovery leaves every service disabled. **Edit service** lets a provider manager enable one, keep its declared groups or choose service groups, and save an optional price in Orch credits. Saving these choices starts no search, page read or model request and chooses no thread backend. A service unavailable on its runtime keeps its cause beside the control.

Catalogue prices are estimates in their original currency. An explicit saved price in Orch credits is what Orch counts; a vendor's reported total stays beside it for comparison. Clearing the custom price restores the catalogue estimate. Editing preserves other services and future configuration fields. A stale provider revision is refused so another manager's change is not overwritten.

A thread uses a paid backend only after its capability is explicitly set to **Service** and its backend names an enabled offering. Provider Use, service/model allowlists and purpose caps apply to the original person or native work source on every request. A provider's model list and service list remain independent. Unset keeps the program's default and selects no paid fallback.
