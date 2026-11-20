---
group: settings
title: Speed, first output and errors
screens: /usage, /thread/$threadId, /resources/providers/$id
keywords: performance, requests, speed, latency, first token, first output, errors, tokens per second, thinking
summary: Compare request performance, find timing in a turn and understand unavailable measurements.
---

# Speed, first output and errors

Open **Usage**, then **Performance** to compare models, or **Requests** to follow single requests. Your scope, dates and filters stay in place between the three views. The page of a provider also shows its last seven days. Each request links to the turn that made it.

Usage is in your account menu. Choose **Me**, an organization, a department or a project in the header, then a range.

- **Last week** means the previous Monday through Sunday. These report days use UTC, shown in the date range. The times of requests use your time zone.
- **By thread** is a tab you see where you may read reports of threads.
- **Turns** counts distinct turns in the matching records, including turns with no report of tokens. Several requests of one turn do not add turns.

## What the figures mean

**First output** is the time from asking the model until its first output arrives, including thinking
before any visible words. **Speed** is streamed output tokens per second while that output is being
generated. Streamed thinking counts where the agent program reports it. Hidden reasoning is subtracted
where its count is known. Time spent running tools or waiting for a person does not count towards speed.
A stream that omits a separate hidden-thinking count keeps speed unmeasured.
**Request time** runs from asking the model to its final output, before a tool runs.

The figures show medians. Half the measured requests fall at or below the median. The **Models** table
also shows the 90th percentile, which helps show slower requests. Percentiles are read from fixed
logarithmic buckets, estimated at the bucket's geometric middle, with about 4.5 percent resolution;
a real zero remains zero.

**Over time** draws Speed, First output and Errors as lines, one per group. A bucket without
measurements leaves a gap. Each chart keeps its own **Interval**. This device remembers your view
choices and named period. Choices in the address take precedence; filter chips and custom dates
stay in the address alone.

**Error rate** is provider-side failures divided by requests with a known outcome, with the failure count
beside it. Failures of the agent program, of the session and of the environment of a tool stay in **Requests** without blaming the provider. A cancellation by a person shows as **Cancelled** and never counts as an error. The row of each error shows its code and the explanation of the provider. Filters can show all failures or one code.

## Why a figure is not measured

A dash means that no value was measured. The question mark beside a figure explains why.

- Speed needs at least half a second of generation and 32 streamed output tokens. Short replies
  still have request time and first output where their boundaries were reported.
- Some agent programs hide thinking without reporting its separate count of tokens. Those requests have times for the first output and for the request, but no measured speed of streaming.
- Some sub-agents report tokens without request times.
- An agent program or a version that reports no request boundaries can report counts while its times stay unmeasured. Earlier usage records may also have no measurements of requests.

These are missing observations. They never become zero in a figure or a percentile. Picker hints appear
only after a choice has at least 20 measured requests here over the last seven days.

## Find the time in a turn

Open the details in the footer of a turn. **Model time** adds its timed main-stream requests. **Tool time** is the turn's elapsed
time less that measured model time. It includes sub-agent work and unmeasured intervals, with a note
when either is present. If no main-stream request has a time, both figures remain unmeasured. **First output** comes from the first
request; **Speed** is the median of eligible requests. **Requests of this turn** opens the filtered log.

While a turn is generating, its live status can also show an estimated output count and speed when
the program has not reported output tokens. The estimate uses the current model's data and is marked
**≈**; without that data it is omitted. A large reopened message can also lack a running count;
its estimate stays unavailable until the program reports usage. Reported token counts take precedence.

## Follow requests

Requests arrive newest first. **Pause** keeps the rows in place as arrivals wait under a count.
**Resume**, or the new-row count, shows them. Use **Earlier requests** to page back, and **Export**
to save the filtered rows as CSV. Running requests are marked until their outcome arrives.

You see the same scope of statistics as in Usage. A place's summary can include other people's totals
while its request log shows only the records you may read. Access is checked again for live deliveries.

A status-bar part appears only when the provider of the thread in view has at least ten requests and
at least 20 percent errors in fifteen minutes, or its median first output exceeds three times its own
seven-day median, with at least ten measured first-output samples in both windows. It opens that provider's failed requests and disappears when the condition clears.

## Web actions

A web step says whether the program searched the web, opened a page or looked for text within a page.
Its details link the reported page; an address the program did not report stays unavailable.
Links open only when you choose them. The step's original call remains in its source view.
These work-step counts are separate from the provider's measured search usage.

## Returned images

Images returned by a tool appear beside that tool's step. Open an image to use the ordinary viewer;
the tool's text remains in its details. If a part has no usable image bytes or cannot be saved,
**The returned image could not be retained** marks that part. This does not turn a successful text
reply into a failed turn or claim that the provider's plan has run out.

In a step's original call, **Full input** and **Full output** open retained text through the same
file card and viewer. A copy action on a shortened excerpt says **Copy shown input** or
**Copy shown output**; use the full file to inspect or download everything that was retained.

## Built-in programs

### Codex

Figures use the request boundaries and token counts reported by the program
and its checked build.

### Claude Code

Supported streams report a separate thinking count. Sub-agents report tokens
without request times.

### DeepSeek Harness

Figures use the observations the program and its checked build provide. A
missing count or request boundary leaves the affected figure unmeasured.
