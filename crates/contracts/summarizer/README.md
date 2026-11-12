# orch-summarizer-contract

Summarizer contract for titles, tags, summary text and handoffs.

Summarizers turn recent activity into text through the `Summarizer` trait. Each descriptor publishes its name, configuration schema and supported outputs; `auto.summarizer` selects the kind and its configuration. Activity is bounded and redacted before the producer sees it. Results conform to the requested size, titles stay within 80 characters, and tags come from the offered choices. Malformed output is discarded with its actual usage still counted. The engine selects the destination: written descriptions are never automation targets. Failure backoff doubles up to one hour, while a skipped call is no failure. Tests also use producers that need no model.

| Module | Planned responsibility |
|--------|------------------------|
| [summarizer](src/summarizer.rs) | Summarize operation and typed failure boundary. |
| [descriptor](src/descriptor.rs) | Configuration schema and supported fields. |
| [activity](src/activity.rs) | Redacted input activity and previous values. |
| [result](src/result.rs) | Text, tag and usage results. |
