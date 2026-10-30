---
title: Speed, first output and errors
screens: /usage, /thread/$threadId, /resources/providers/$id
keywords: performance, requests, speed, latency, first token, first output, errors, tokens per second, thinking
---

# Speed, first output and errors

Open **Usage**, then **Performance** to compare models, or **Requests** to follow individual requests.
Your scope, dates and filters stay in place between the three views. A provider's page also shows its
last seven days. Each request links to the turn that made it.

Usage is in your account menu. Choose **Me**, an organisation, department or project in the header,
then a range. **Last week** means the previous Monday through Sunday. These report days use UTC,
shown in the date range; request timestamps use your time zone. **By thread** is a visible tab
where you may read thread reports. Turns count distinct turns in the matching records, including
turns with no token report. Multiple requests of one turn do not add turns.

## What the figures mean

**First output** is the time from asking the model until its first output arrives, including thinking
before any visible words. **Speed** is streamed output tokens per second while that output is being
generated. Streamed thinking counts where the harness reports it. Hidden reasoning is subtracted
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
beside it. Harness, session and tool-environment failures remain in Requests without blaming the provider. A person's cancellation is visible as **Cancelled** and never counts as an error.
Each error's row shows its code and the provider's explanation. Filters can show all failures or one code.

## Why a figure is not measured

A dash means that no value was measured. The question mark beside a figure explains why.

- Speed needs at least half a second of generation and 32 streamed output tokens. Short replies
  still have request time and first output where their boundaries were reported.
- Some harnesses hide thinking without reporting its separate token count. Those requests have
  first-output and request times, but cannot have a measured streaming speed.
- Some sub-agents report tokens without request times.
- A harness or version without proven request boundaries can report counts while its times remain
  unmeasured. Earlier usage records may also have no request measurements.

These are missing observations. They never become zero in a figure or a percentile. Picker hints appear
only after a choice has at least 20 measured requests here over the last seven days.

## Find the time in a turn

Open a turn's footer details. **Model time** adds its timed main-stream requests. **Tool time** is the turn's elapsed
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
