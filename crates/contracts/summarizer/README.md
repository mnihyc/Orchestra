# orch-summarizer-contract

Summarizer contract for title, description and tag generation.

The contract of summarizers, which turn the recent activity of a node into the text of one field (a title, a description or tags): the `Summarizer` trait, the descriptor each publishes (its name, the schema of its configuration, the fields it writes) and how `auto.summarizer` splits into the kind and the configuration, the activity it is shown (the latest items, each cut, without tool output, already redacted), and the result with the one rule that holds every result before it is written: one line of at most 200 characters for a description, at most 80 for a title, tags a subset of the offered ones, longer output cut and malformed output discarded with the usage still counted. The back-off after failures doubles up to one hour, and a skip is no failure. A summarizer with no model in the tests stands for an implementation.

| Module | Planned responsibility |
|--------|------------------------|
| [summarizer](src/summarizer.rs) | Summarize operation and typed failure boundary. |
| [descriptor](src/descriptor.rs) | Configuration schema and supported fields. |
| [activity](src/activity.rs) | Redacted input activity and previous values. |
| [result](src/result.rs) | Text, tag and usage results. |
