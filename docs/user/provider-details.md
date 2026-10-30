---
title: Choose and read provider details
screens: /thread/$threadId, /resources/providers, /resources/providers/$id, /resources/machines, /resources/templates
keywords: how do I choose provider details, facts source, models source, SourceChoice, balances, spend, reset, Not read yet, None reported, Does not report, Unencrypted, providers.allow_http_upstreams, Last program error, interrupted
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
read times. Unread members share one **Not read yet** line and **Refresh**;
an unread fact is never a guessed zero. Delete stays in the page menu, and
is also available if its stored details cannot be shown. Its ordinary access
and in-use checks still apply.

## Read the four states

| The card says | What it means |
|----------------|---------------|
| Does not report | The source neither declared nor ever told that member. |
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
for a manager to enable; nothing is silently switched on. Orch also
reads a minute after a vendor-reported reset. Its compact provider
caption uses the binding windows, else a first reported balance, else
a first spend row. Without permitted facts it cannot invent room.

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
