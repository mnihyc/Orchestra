# Usage at three depths, caps, rates and passing a limit

Usage shows scoped figures, series, breakdowns and ledger records with CSV, caps and their current effect, and rate cards. The same components supply a person's allowances, provider Usage and each place's overview board. System rates and catalogue changes reuse the rate form and guarded settings APIs. `GET /usage/catalogue` returns `200` with `null` when none has been loaded; the rate card, catalogue dialog and system settings show the ordinary empty state. An existing catalogue with no matching model has an empty family list; actual HTTP failures remain failures.

Usage is one screen with three doors: one's own, a place's and a provider's. It is counted in tokens
and in credits at the rates a team writes; Orch records usage and bills nobody. The prompt somebody
meets at a limit is `../../ui/LimitPrompt.tsx`.

- `Report.tsx`: The shared Page/header/tabs for Usage, Performance and Requests at every width; span and removable filters, one Access action and one named guide, and the scoped figures.
- `navigation.tsx`: The same three section choices as real sidebar links, preserving scope, dates and filters. A visible Navigation slot owns them; closing it restores the report's tabs without replacing its active view.
- `ScopePicker.tsx`: Me, visible orgs, departments and projects in the header, reusing authorized names and one opened org's cached tree; never eager trees for every org.
- `PerformanceView.tsx`: The Performance view's shared frame, with the same scope, span and filters as Usage.
- `Performance.tsx`: Median figures with missing-measurement reasons, a sortable model comparison and error-code filters.
- `PerformancePlaces.tsx`: Seven-day performance at open model choices and providers, and the degraded-only status part. The open ring or Usage panel owns provider-health loading; the status part only observes its cache.
- `performance.ts`: Shared display units, confidence floors and like-for-like performance comparisons.
- `Requests.tsx`: A scoped live model-request log with historical cursor pages and a pause that keeps one's place.
- `outcomes.ts`: Typed contract outcome selectors shared by addresses, controls and request filtering.
- `requests.ts`: Pause and resume of the live request log, with stable rows while arrivals wait.
- `Chart.tsx`: The series over the span, by the hour, day, week or month: the measure and split; dragging sets the span. Its readable data uses the shared Table and ordinary disclosure button, with a content-sized empty state.
- `Plan.tsx`: Provider plan windows, ordered by the binding window and duration, with independent current-window and bucketed-history views.
- `PlanChart.tsx`: A fixed percentage axis, measured values, a dashed projection, limit and now rules, bucketed usage and audit-linked early resets.
- `ForecastLine.tsx`: The same observed-pace sentence below every plan meter and period cap, with stale and insufficient-reading states.
- `choices.ts`: Separate device choices for the usage screen and provider Usage, retaining each chart while explicit addresses take precedence.
- `Breakdowns.tsx`: The same total divided by person, model, provider, agent, kind of environment and project, one table each; a row filters the whole screen.
- `Board.tsx`: The usage board of a place: turns, tokens and credits in a period, over time, by person and divided one other way, for whom its row names; Open usage.
- `Records.tsx`: The ledger rows behind what is on screen, each followed to its turn, with the rate its credits were counted at and where that rate came from.
- `Limits.tsx`: Limits card: caps on tokens, on credits and on turns at once with their meters, who is waiting, how firmly each holds, how often each was passed, changes in place.
- `CapPeople.tsx`: Shared person chips in cap subjects, setter attribution and per-person shares, including the limit prompt.
- `AddCap.tsx`: Sentence-shaped cap form: who, how much of what (tokens, credits or turns at once), on which models or family and which agent, where; with its effects stated.
- `PassLimit.tsx`: Passing a limit, from the composer, a row that waits and a blocked thread: Proceed anyway, Pass mine and wait, Change a limit…, and for everybody else who can let them through.
- `RateCard.tsx`: The rate card of a place: the credits a million tokens of each kind cost for a model, from which day; what the shipped catalogue estimates where nobody entered a rate, marked as an estimate; and the models in use that have no rate.
- `Export.tsx`: What is on screen as CSV: the series, or the records.
- `Amounts.tsx`: How an amount of credits is written where it stands among others: a sum with its estimate and what it leaves out, and the credits of one record with the mark of where its rate came from.
- `Catalogue.tsx`: The catalogue of estimates, read-only: its families with their vendor, their rates, the page they were read from and the day, and a field that says which family a model's name would match.
- `Figures.tsx`: The three figures of a span (turns, tokens, credits) each against the span before, what the credits are made of, and the tokens divided the way a rate divides them.
- `Mine.tsx`: One's own usage this month in three figures with a small chart that leads to the usage screen, and the share of each allowance that covers one.
- `Period.tsx`: Compact named ranges including the previous Monday–Sunday week and shared custom date fields. UTC rollup days are labelled once, independently of the reader's timestamp zone.
- `RateForm.tsx`: The form of one row of the rate card: the model's pattern, the prices for a million tokens, the speeds that multiply them and the steps for large prompts, each fault beside its field.
- `ThreadUsage.tsx`: What bounds a thread and what it used: the tightest cap as a percentage for its chip, the caps that cover it as the rows of the Limits card with how firmly each holds there, what the thread used so far, and the way to its usage.
- `capForm.ts`: What a cap that is being made or changed holds while it is edited, the rules it must keep before it is sent, and what it becomes on the wire.
- `figures.ts`: How the figures of usage are written and compared: credits with the digits that mean something, what is an estimate and what had no rate, the change against the span before, and the share of the tokens that a rate divides them into.
- `keys.ts`: What a row of a table and a stack of the chart are about: the member of a dimension that a key names, the words for it, the value that narrows the screen to it, and the key that is every other member together.
- `rates.ts`: The rate card of a place as a form holds it and as the API says it: a row of the card in texts, what is wrong with a row, and the card with a row put in or taken out.
- `saveFile.ts`: Hands a file the page holds to the person: a name for it, and a click on a link to it that the browser takes as a save, and the names a usage file is given.
- `context/fill.ts`: What fills a context and what a turn's tokens are made of, as parts of a bar: shares, tones, colours with patterns, and the words of each.
- `context/PartsBar.tsx`: A bar of parts, each with its colour and pattern and its words, which the keyboard reaches part by part.
- `context/ContextRing.tsx`: How full a thread's context is, at a glance: a ring beside the message controls, coloured by how near the conversation is to being shortened, with a tick where it is; one click opens what fills it.
- `context/ContextPanel.tsx`: What fills a thread's context: a bar of its parts, then free space and the reserve kept for shortening, the same as a table, and when the conversation was last shortened.
- `context/TurnCost.tsx`: What a turn's tokens were made of, in its footer: a thin bar of fresh input, cache read, cache write, output, reasoning and sub-agents with the numbers and the cache hit rate under the pointer, and one click to the turn's details.
- `context/ThreadComposition.tsx`: What a thread's turns used over time: each turn's tokens by part as a column, the context's fill as a line with its shortenings marked, the cache hit rate, the sub-agents' share, the same as a table, and an export.
- `search.ts`: What the address of a usage screen says and what it comes to: the scope asked about, the span, what the chart shows and how it is split, which table is open and what the whole screen is filtered by.
- `series.ts`: Amounts as stacks, token kinds with their existing looks, and non-additive performance lines with gaps, plus the colour each member keeps.
- `words.ts`: A cap in words, the same words wherever a cap is shown: whom it covers, what it allows, of which models and with which agent, at which speed, where it was set, and when it starts again.

The report's empty chart names the selected metric: ten turns with no cost say 0 credits, not that nothing was used. Unrated tokens retain the unknown-cost explanation; positive totals without buckets say there are no chart readings. Only an empty span uses the whole-span empty sentence.
- `CapDialog.tsx`: The dialog that makes a cap or changes one: the sentence with its gaps as controls (whom it covers, the amount, its unit, the period, the models), followed by visible choices for agents, speed, warning and what happens at the limit.
- `SystemRates.tsx`: The system's own pattern rate card and guarded catalogue replacement, using the same rate form.
- `Visibility.tsx`: One ordinary Access button to the place's permission page; no audience sentence or eager permission-holder read.

The header supplies scope and dates across all views. By thread stays visible in the shared wrapping
tabs where access permits it. Records and requests lead with the short thread name, with differing
place context once. A turn counts once within each queried group regardless of whether a meter reported
tokens; multiple requests, meters, days or models do not multiply the span's distinct-turn total.
Tokens and credits still use only the counted meter. Standalone summaries add no turn.

Analytics carries `figures_before` alongside the performance comparison, so comparison needs no second
HTTP request. Requests mounts no hidden figures query; history loads once and native deliveries repair
only demonstrated cursor gaps. Date ranges stay the explicitly opened report's snapshot until changed.

- `ReportFrame.tsx`: Shared report heading, scope/filter bar and body slots; the usage report uses this same frame.
