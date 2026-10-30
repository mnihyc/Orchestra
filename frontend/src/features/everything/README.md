# Everything

`/everything` is a main-area page for every signed-in person. Management opens the same browser in
system scope. Each branch and name search uses the API's keyset pages and served access facts.

Tree and Recent share the Navigation controls. Recent crosses levels using personal last-opened or
thread/document activity, with calendar groups, location and time/editor columns. Navigation links
carry the beginning and visibility choices; validated list state is retained by `view.ts` through the
device view-choice store. Date, kind and archive/done filters remain server predicates.

Recent keeps its optional columns separately from Tree. Default columns fit by giving Where the
remaining space after Name's intrinsic width; its path truncates from the start and keeps the complete tooltip. Explicit extra
columns can widen the table inside its named scroller, whose scrollbar stays visible on overflow.
Recent has no hierarchy gutter. Narrow cards show name/kind/state, nearest place with the chosen
time, then By when known; only explicitly chosen extra columns retain labelled fields. Tree keeps
its ordinary detailed cards.

Select mode adds row ticks and All for the visible eligible rows. Share uses the existing multi-item
permission review and its live checks; merely drawing or selecting rows reads no detail pages. A new
filter/order clears selection. Private/path-only rows and rows without Share cannot be selected.

| File | Responsibility |
| --- | --- |
| `Everything.tsx` | One name field and the list, labelled filters behind an initially closed button at every width, column choices per person, sorting and expansion controls. |
| `EverythingTable.tsx` | Lazy tree-grid levels, keyboard navigation, row actions and safe icons. Desktop columns become labelled stacked rows on narrow screens, using the page scroll. |
| `columns.ts` | Optional columns and device-local preferences. The admin column is available only while the session switch is on; hiding it retains the saved preference. |
| `view.ts` | Validated address and per-device listing choices, excluding typed search and selection. |

Participation means explicit ordinary assignments or follows, as served by the shared access Frame.
It is independent of the admin-only marker. Search uses the shared literal matcher, name relevance,
personal visits and activity before the selected column. Created and Active from are separate filters;
activity includes work below a place. Unknown wire kinds keep a generic item icon and label.

The header has no subtitle and the table has no repeated keyboard sentence. Scope, kinds, owner,
participation, admin-only, dates and the secondary sort picker mount only inside the requested Filters
popover. Its count reflects narrowing fields, Escape returns focus, and choices remain active when
closed. Column sorting and the polite sort announcement remain; only the duplicate visible sort prose
is absent. Row ownership shortcuts filter immediately without mounting a hidden person lookup.

Regression sources are `tests/component/everything.test.tsx`, `tests/e2e/everything.spec.ts`, the API's
`tests/org_contents.rs` and `tests/everything_scale.rs`, with the fake route in `tests/doubles/everything.ts`.

Resource lists use this same query with one resource kind, `parent=resources` and `resource_details=true`.
Their state, harness, origin/unit kind, provider view and home predicates run before the page limit.
Only that readable page gets the existing typed resource views. Search also matches its visible owner
and home labels; normal hierarchy search stays name-only. Hosts may sort by declared memory/capacity
or the combined count of starting/running environment and provider units; unknown capacity stands last.
The API's `tests/everything_resources.rs` and `tests/component/resource-inventory.test.tsx` cover that join.
