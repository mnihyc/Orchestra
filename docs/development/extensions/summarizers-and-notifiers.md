# Summarizers and notifiers

A [`Summarizer`](../../../crates/contracts/summarizer/README.md) receives an
authorized activity slice, a node reference, previous text, limits and tag
choices. Its descriptor names the fields it can write and a configuration
schema. Titles and descriptions fit their character limit on one line; tags
come from the supplied choices and fit the count limit. A field not declared
returns unsupported. No activity may produce an answer within limits or a skip.
The engine owns the triggers, concurrent human edits, rate limits and ledger.

Run the summarizer suite through the trait or use `orchctl plugin check` on
the [rule-based example](../../../examples/plugins/summarizer/README.md).
It requires no model. A model-backed implementation also needs a controlled
completion test and separately authorized real-model measurements; the reserved
built-in model package does not yet export one.

A [`Notifier`](../../../crates/contracts/notifier/README.md) checks a channel's
configuration and delivers an already rendered message. Its declaration says
whether it reaches a person's email, devices, an account linked through a code,
or a channel destination; its fields, permitted owners, language templates and
body format provide the form and texts. A check sends no notification. The
engine writes Inbox/outbox rows in the event transaction; the worker decides
delivery time and renders; the notifier sends and does not read the database.

For a linking notifier, create a code-bearing link and poll from a cursor.
Codes belong to pending routes and expire; do not poll when none waits. The
worker handles retryable failures and retry-after values, and disables a gone
target or removes its dead device subscription. Keep delivery ids stable across
attempts where the far side supports deduplication. An uncertain remote result
cannot guarantee exactly-once delivery.

Pass the private-host policy through every outbound connection and redirect.
The common contract checks special-purpose addresses and embedded IPv4 forms;
unapproved private destinations are refused at the actual socket address.
Secret configuration values are handed over for one call and do not enter the
descriptor, report or returned channel values.

The notifier conformance fixture supplies two disposable rendered deliveries,
and controlled retry, gone and private-address refusals. Missing far-side
counterparts are reported as unverified. Built-in notifier tests also speak to
controlled SMTP, bot, webhook and push services; those tests prove their actual
transport, signing and encryption behavior. The
[file notifier](../../../examples/plugins/notifier/README.md) provides the
smallest external program and keeps its log separate from JSON transport output.
