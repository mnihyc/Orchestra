# Read the resource library's counts and set paste delivery

Know what the **Giving** and **Used** figures in the resource lists count, and where they come from, and set how pasted text is delivered. At the end you can explain a count to a person who asks, and pasted text goes to agents the way you chose.

People read the lists as described in [Resource library](../user/resource-library.md). Changing a list's view changes no permission and admits no work.

## Before you start

You need to be a system administrator to change the paste setting. Reading the counts needs nothing.

## What the counts mean

The lists return current giving and reported use with their ordinary page read. The same facts also serve resource pages and the consequences shown in plans.

- **Recorded since** marks a partial history. **Not recorded** means there is no reliable source.
- Counts include only what an agent program reports. Offering or loading a skill does not prove that it was used. A tool error that came back can count as a call; a refusal made locally, and the replay of a closed turn, cannot.
- Turns of providers, profiles and environments use the usage ledger and the immutable execution segments. MCP calls and skill invocations keep stable identities inside the turn Orch started that reported them. When the turn closes, its replay receipts and temporary server mapping are removed in the same transaction. The counts and the people captured stay. A resumed or imported history that is already closed cannot create a receipt or add another use.
- A successful creation of an environment or a provider keeps one membership for each template or host. Creating the same resource again updates its last use. Moving it to another host or template starts a membership there. A failed attempt adds no creation. These memberships outlast the removal of the original runtime.
- Summaries are read from indexes by subject. All-time rows keep lifetime totals, and UTC day rows serve the thirty-day window; older day rows are dropped as new use is recorded, and the lifetime row stays. The people captured keep one membership for each subject and version with their latest use, so daily counts of distinct people are never added together.
- Private names are filtered when the reader opens the list or a count.

## Set paste delivery

**Pasted text in the message** decides how much pasted text goes into the message and how much goes as a file. Its shipped share is 25%.

1. Open the setting at the place you manage.
2. Choose **Always as a file** (0) or **Always in the message** (100), or a share in between. Each paste can still be chosen on its own.
3. Automatic compaction by the agent program stays on. When a provider refuses the whole window, the failed turn shows **Send as a file**.

## If it does not work

| You see | What to do |
|---|---|
| A count looks too low | Check **Recorded since**: the history is partial. A skill only offered, or a call refused locally, is not counted |
| A person cannot see a name in a list | Private names are filtered for each reader |
