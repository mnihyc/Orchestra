---
group: work
title: Read and refresh summaries
screens: /thread/$threadId, /topic/$topicId, /doc/$documentId, /chat/$chatId, /search, /help/$article
keywords: summary, summarizer, refresh, catch up, chat title, automatic, summary.targets, auto.summarizer, weekly cap, Ask
summary: Read a resource's summary, refresh it explicitly, or open an editable Ask draft about it.
---

# Read and refresh summaries

A summary is a short paragraph beside a thread, topic, document or chat. Its
status, readable summarizer and last successful update appear together.
Descriptions remain the words somebody deliberately wrote; a summary never
rewrites them.

## Read the status

**Fresh** matches the selected source. **Out of date** keeps an earlier
paragraph after its source changed. **Updating** means a request is queued or
running; its previous readable paragraph stays visible. **Failed** keeps the
last successful paragraph and shows the problem. **No summary yet** means no
paragraph has been produced. **No summarizer** means no usable producer is
selected; an existing readable paragraph may still remain.

Open **Summary details** for the selected model's declared base rates. A
missing rate stays unreported. These rates are quotes; the Usage pages show
what actual calls consumed. The thread's **Parameters** view shows the same
summary, producer, state and time.

In an older message version, Parameters explains that the live summary does
not describe that history. It does not load or refresh the current summary there.

## Refresh and choose defaults

**Refresh summary** requests one update as you. Opening a page or searching
does not request paid summary work. Your refresh belongs to you and obeys your
provider Use, permitted models and applying usage caps. A weekly cap with
purpose **Summary**, subject **Members**, split **Each** bounds each person's
summary use separately.

If a refresh acknowledgement is lost, retrying **Refresh summary** continues
that pending request. After an acknowledgement, **Try again** on a failed
read only reloads the summary; a later deliberate refresh is a new request.

For a personal refresh, an automatic summarizer need not be configured. Orch uses
an allowed API model under your current access, allowlists and summary caps,
without enabling automation. If no model is usable, the box gives its reason.

For automatic work, choose **Summarizer** and edit **Summary targets** in the
place's settings.
Thread, topic, document, chat and chat title have separate automatic choices,
events, pacing and input/output defaults. Automatic refresh is off until
enabled. Automatic summaries use their summarizer setting's writer and count
under shared caps. Size and interval values are defaults, not spending caps;
locks above the place still apply. Call recovery follows the configured call
rules for summaries.

Without an explicit interval, threads use 120 seconds, topics 900 seconds,
documents 300 seconds, and chats and chat titles 300 seconds. Topic changes
aggregate many threads; document edits and chat messages usually arrive in
bursts. An explicitly set shared trigger interval overrides these defaults;
a per-kind interval overrides the shared value. Summary options shows the
effective interval without turning it into a stored document override.

For one document, open **Actions → Summary options**. Save an own choice or
use **Use place defaults** to inherit again. Saving options controls later
events; it does not itself request a summary. An API refresh can name a
permitted model for that one personal request without changing the automatic
default.

**Scheduled** shows the actual minimum-interval or recovery wait and its eligibility time.
The previous paragraph remains readable and personal refresh stays explicit. Free reads
wait until that time; eligible or running work follows **Summary follow interval**
(5 seconds by default, configurable by place). A failed read offers a free retry.

Search reads stored paragraphs in batches; it neither refreshes them nor
chooses a model. Larger result sets continue through further read windows.
Each source keeps its current permissions and inspection state, so another
root's unavailable source does not block a readable root.

If a summary read fails, the hit says it is unavailable for this view.
**Try again** repeats only that free read, without revealing a private cause.

## Choose the input budget

**Input tokens** starts at 32,000 in the model summarizer. A target can
override it. You may raise it within the model's available window after
reserving its output; the instruction and framing also use input space.
This is a planning estimate. Usage still shows what the model actually used.

Recent eligible items come first. Short items stay whole; long items share
the remaining space fairly, keeping useful detail before adding older items.
The prompt reads oldest to newest. Unchanged items keep their cuts between
refreshes; when full, an old chunk leaves together to make room for new items.

**Summary details** reports how many source items were included, shortened,
and omitted. Shortened items are part of the included count. Private and
unreadable material is excluded before this selection. Changing the budget
does not itself request paid work.

## Chat catch-up and title

**Catch up** covers the unread message range captured when you opened the
chat. Refreshing that catch-up keeps the same range; it does not advance your
read position. Another person's unread position does not become yours.

**Chat title** is separate and uses the chat's bounded current text. A written
group name stays in place. Automatic titles do not replace it. Private groups
and restricted descendants do not flow into a topic's shared summary.

Stored content opened through chat inspection uses the normal inspection
audit. It creates no membership, chat draft or read position. A disabled action
keeps its reason beside the existing control.

## Ask and search results

**Ask about this** opens the existing Ask panel with an editable quote and
reference in the resource's organization. It appends to the matching draft;
opening it does not send a question. Review it and use **Send** when ready.

Help summaries are written with each translated article and use no model.
Content search shows a hit's existing readable summary when one is available;
it never generates one for the search. **Ask about these results** quotes the
current visible selection in its order. When several organizations appear,
choose which organization's results to use. One conversation keeps one root
organization. A source that is still loading, unavailable or inspection-only
cannot silently become an ordinary Ask input.
