---
group: work
title: Read and refresh summaries
screens: /thread/$threadId, /topic/$topicId, /doc/$documentId, /chat/$chatId, /search, /help/$article
keywords: summary, summarizer, refresh, catch up, chat title, automatic, summary.targets, auto.summarizer, weekly cap, Ask
summary: Read the summary of a resource, refresh it on purpose, or open an Ask draft about it that you can edit.
---

# Read and refresh summaries

A summary is a short paragraph beside a thread, topic, document or chat. Its status, its summarizer that you can read and the time of its last successful update appear together. Descriptions stay the words that somebody wrote on purpose, and a summary never rewrites them.

## Read the status

- **Fresh** matches the source that is selected.
- **Out of date** keeps an earlier paragraph after its source changed.
- **Updating** means that a request is queued or running. The previous paragraph stays visible and readable.
- **Failed** keeps the last paragraph that succeeded and shows the problem.
- **No summary yet** means that no paragraph has been produced.
- **No summarizer** means that no producer you can use is selected. A paragraph that exists and can be read may still remain.

Open **Summary details**, or **Catch-up details** for a chat, to see the declared base rates of the model selected. A rate that is missing stays unreported. These rates are quotes. The **Usage** pages show what the calls really used. The summary of a thread stays above the conversation while **Parameters** is open. In an older version of a message, **Parameters** explains that the live summary does not describe that history. It does not load or refresh the current summary there.

## Refresh and choose defaults

**Refresh summary** asks for one update as you. Opening a page or searching never requests paid summary work. Your refresh belongs to you and obeys your **Use** of the provider, the models you may use and the usage limits that apply. A weekly limit with the purpose **Summary**, the subject **Members** and the split **Each** bounds the summary use of each person separately.

- If the acknowledgement of a refresh is lost, retrying **Refresh summary** continues the request that is pending. After an acknowledgement, **Try again** on a read that failed only reloads the summary. A refresh you do later on purpose is a new request.
- For a refresh of your own, an automatic summarizer does not have to be set up. Orch uses a model of an API that is allowed, under your current access, the lists that are allowed and the summary limits, without turning automation on. If no model can be used, the box gives its reason.

If its source changes before the first model call, Orch reads it again and rebuilds
the prompt without charge. **Summary read repeats** defaults to three repeats;
zero makes no repeat. Content that keeps moving says so. If the source changes
during a model call, its answer is not published as current and no second call is
made automatically. The box keeps any previous paragraph out of date, explains the
change and offers **Summarize again**.

You can refresh a summary without configuring an automatic summarizer. When you leave the model choice to Orch, it selects an allowed API model under your current access, allowlists and summary caps. You can also choose a supported subscription program for that refresh. If no model is usable, the box gives its reason.

Without an interval you set, threads use 120 seconds, topics 900 seconds, documents 300 seconds, and chats and chat titles 300 seconds. Changes in a topic add up many threads. Edits of documents and messages of chats usually come in bursts. A shared interval for triggers that you set overrides these defaults, and an interval for one kind overrides the shared one. **Summary options** shows the interval in force without turning it into an override stored on the document.

### Configure a summarizer

Open **Settings → Automation → Summarizer** and press its pencil to edit this place's choice.
Choose a registered summarizer. **A model** uses an enabled API model or a supported subscription program,
chosen by name; the setting writer needs provider Use for other work. **Answer tokens**
and **Context tokens** show their built-in defaults until you set your own. **Request options**
and **Instructions per field** open when needed. An unusable provider or model is marked
beside its field. Other registered kinds show their declared configuration fields.
Setting up the model form needs no typed kind name, provider identifier or JSON.

**None here** removes this place's choice so inheritance applies. **Edit as JSON** shows
the raw value. The row's **Save** writes its whole draft once; **Discard** restores the
saved value. Another row's switch or a filter keeps this draft without saving it.
Enable automatic **Summary targets** separately; saving a summarizer does not enable
automatic work. Titles and tags use it only when their own automation choice says so.

The confirmed value lives in the server database. Program updates preserve it with that
database. Removing this place's override restores inheritance without erasing the setting
above. Provider Use, model availability and usage caps are checked again when a summary runs.

Each **Summary targets** row has an **Automatic** switch. While enabled, **When**,
**At most every**, **Reads up to** and **Writes up to** show defaults until you set your own.
Its **Save** and **Discard** affect only that row's draft.

A subscription program runs in a private helper owned by the person whose authority runs the request. The selected sign-in carries its usage. Sharing the helper asks you to confirm that its history includes copies of the source material. The prompt inspector checks access to those sources each time you open it.

Without an explicit interval, threads use 120 seconds, topics 900 seconds,
documents 300 seconds, and chats and chat titles 300 seconds. Topic changes
aggregate many threads; document edits and chat messages usually arrive in
bursts. An explicitly set shared trigger interval overrides these defaults;
a per-kind interval overrides the shared value. Summary options shows the
effective interval without turning it into a stored document override.

**Scheduled** shows the real wait for the minimum interval or for recovery, and when it becomes eligible. The previous paragraph stays readable, and a personal refresh stays explicit. Reads that are free wait until that time. Work that is eligible or running follows **Summary follow interval** (5 seconds by default, which a place can change). A read that failed offers a free retry.

For one document, open **Actions → Summary options**. Save an own choice or
use **Use place defaults** to inherit again. Saving options controls later
events; it does not itself request a summary. A refresh request can name an
allowed backend for that one personal request, including a supported subscription
program with its placement and options, without changing the automatic default.

**Scheduled** shows the actual minimum-interval or recovery wait and its eligibility time.
The previous paragraph remains readable and personal refresh stays explicit. Free reads
wait until that time; eligible or running work follows **Summary follow interval**
(5 seconds by default, configurable by place). A failed read offers a free retry.

Search reads existing summaries for its results; it neither refreshes them nor chooses a model.
Larger result sets continue as more results are shown. Each source is checked against your
current access, so an unavailable source in another organization does not prevent you from
asking about readable results.

If a summary read fails, the hit says it is unavailable for this view.
**Try again** repeats only that free read, without revealing a private cause.

## Choose the input budget

**Input tokens** starts at 32,000 in the summarizer that uses a model. A target can override it. You may raise it within the window of the model, after the output is reserved. The instruction and its framing also use space of the input. This is an estimate for planning, and **Usage** still shows what the model really used.

Answer allowances use **tokens**, separately from a target's **output characters**.
The model summarizer starts with `answer_max_output_tokens` at 1,024 for paragraphs,
descriptions, tags and handoffs; `title_max_output_tokens` at 120 for titles;
and `goal_max_output_tokens` at 512 for a goal verdict and its reason. Each is
configurable under **Summarizer**. Target characters still start at 600 for a
paragraph and 80 for a title. They guide the returned text, not the model's
reasoning allowance or actual billed use.

`reasoning_tokens` leaves another 2,000 tokens by default when reasoning is
known or unrecorded. Disabled tuning controls do not imply that an upstream
model does no reasoning. An explicit report without thinking support removes
that reserve; permitted effort choices remain separate. Zero reasoning tokens
leaves no extra room.

Recent eligible items come first. Short items stay whole; long items share
the remaining space fairly, keeping useful detail before adding older items.
The prompt reads oldest to newest. Unchanged items keep their cuts between
refreshes; when full, an old chunk leaves together to make room for new items.

**Summary details** reports how many items of the source were included, shortened and left out. Shortened items are part of the number included. Private material, and material you cannot read, is excluded before this selection. Changing the budget does not itself request paid work.

## Chat catch-up and title

**Catch up** covers the range of unread messages that was captured when you opened the chat. Refreshing that catch-up keeps the same range. It does not advance your position of reading, and the unread position of another person does not become yours.

When the range is empty, the box says there is nothing new and no paid request
is created. An explicitly selected nonempty range can still be summarized
after those messages have been marked read.

**Chat title** is separate and uses the chat's bounded current text. A written
group name stays in place. Automatic titles do not replace it. Private groups
and restricted descendants do not flow into a topic's shared summary.

Use **Ask about catch-up** for the unread messages and **Ask about chat title** for the title. Each opens its own source in the Ask draft, which you can edit. Stored content that is opened through the inspection of chats uses the usual audit of inspection. It creates no membership, no draft of a chat and no position of reading. An action that is turned off keeps its reason beside the control.

## Ask and search results

**Ask about this** opens the Ask panel that exists, with a quote and a reference that you can edit, in the organization of the resource. It adds to the matching draft, and opening it sends no question. Review it and use **Send** when you are ready.

Summaries in Help are written with each translated article and use no model. Search of content shows the summary of a hit when one is available to read. It never generates one for the search. **Ask about these results** quotes the visible selection as it is, in its order. When several organizations appear, choose the organization whose results to use. One conversation keeps one root organization. A source that is still loading, is unavailable or can only be inspected cannot become an ordinary input of Ask quietly.
