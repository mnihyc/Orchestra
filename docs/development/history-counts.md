# History plan measurements

A history plan measures decoded JSON payload bytes. Database compression does not reduce that measure.
The same measure applies to current items, retained revisions and operation-owned history cuts.

`thread_history_turn_counts` holds exact item counts and decoded payload sizes by continuation, actual
turn (or loose history), stream, kind and before-import flag. The item producer maintains it for every
insert, relevant update and deletion. Changes to pin or quote metadata which leave those measures
unchanged do not churn the counters. Position bounds support an exact partial-edge calculation.

`thread_history_count_slices` and `thread_history_slice_counts` retain per-turn metadata for an immutable
prefix at its actual sequence fence. A span captures that metadata once. Whole groups copy their
maintained totals; only a bisected group or an older revision needs individual metadata resolution.
A retained byte total never changes to the current source's size after a source edit.

Operation-owned cuts keep their item and metadata totals once at capture in
`thread_history_cut_counts` and `thread_history_cut_row_counts`. Main-stream content counts, decoded
payload bytes, all reserved item positions (including markers), and metadata row counts are separate
measures. A later source edit, deletion, copied body or proved native alias does not change them.

The existing thread schema migration creates and fills these derived tables. Existing payload content
is not rewritten. Source selection, ordinary authorization, immutable membership and retained revision
checks remain with the existing history readers. Counts do not authorize reading the measured content.
