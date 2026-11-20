---
group: work
screens: /workbench, /org/$slug, /project/$projectId, /topic/$topicId, /thread/$threadId, /doc/$documentId, /chat/$chatId, /search, /resources, /inbox, /everything, /help, /settings/interface
keywords: Ask, question, answer, sources, citation, organization, root org, history, retention, private, model, effort, model policy, floating window, Side thread
summary: Ask read-only questions about the page in view and the work you can read in one organization.
---

# Ask about your work

**Ask** is your personal, read-only conversation about the page in view and about the work you may read in one organization. Its answers cite their sources. It reads as you and changes nothing.

## Open Ask

Choose **Ask about this page** above **Help** in the left rail. On a phone, use the same button beside the finder. **Shift+F1** opens it too, and the shortcut editor in **Help** lets you change that key.

Ask starts docked on the right. **Pop out** puts the same conversation and draft in a floating window.

- Drag its title to move it, and its lower right corner to resize it. Focus either handle and use the arrow keys for the same actions; **Shift** moves farther.
- **Maximize**, **Restore size** and **Dock** change its placement.
- Its size and position are kept for your account on this device.

## Write a question

Type a question and press **Enter** to send it. **Shift+Enter** adds a line. Confirming text in an input method does not send the question.

- Use the paperclip, or paste or drop files, to attach them. The chips show progress and let you remove or retry a file. Sending waits for uploads that are not finished.
- Type **@**, or choose **Mention a resource**, to find a document, thread, chat or resource that you can read in the organization of this conversation. Choosing one adds it to the draft. It sends nothing by itself.
- The model chip lists the entries you can use in the **Ask models** setting of this root organization. **Default · model** follows the first entry of that ordered list that you can use. Choose another entry, and your reasoning effort, through the same control for model and effort that threads use. Effort follows the default of the model until you choose one, and a model that declares no effort has no effort rows. Other choices stay within the limits of the provider. Your own choice, with its parameters, is kept with the conversation.

**Ask models** has its own list in **Settings → Automation**. At the level of the system, a person who may edit it can add or remove API models that are turned on and move them up or down. Entries hold only the provider and the model. At an organization the list comes from above: an editor can remove entries to narrow it but cannot add or reorder them, and the order from above stays. Effort and other parameters stay with your own choice in the chip. Providers that only have a subscription are not Ask models. The current policy of the organization and the limits on usage apply to every turn.

If the list is not set, is empty, or has no entry you can use (the model policy can exclude every entry), the panel shows the reason and has no composer. A link to the setting appears only if you may edit it. Otherwise the explanation stays visible, so that you can ask an administrator of the organization to set up a model that can be used.

A provider's **Use** entry can allow **Ask**, **Other work**, or both. To share a provider only for questions, open that entry's options and leave only **Ask** selected under **Uses**. Its model and effort restrictions still apply together. Other work includes ordinary threads, summaries and services. The asking person and the configured payer both need an entry that permits Ask; choosing a payer grants no access.

When the list is unset, empty or has no usable entry, including when model policy excludes every entry, the panel shows the reason and has no message composer. A link to the setting appears only when you may edit it. Otherwise, the explanation remains visible so you can ask an organization admin to configure a usable model.

If sending, stopping or preparing a draft fails, **Try again** repeats that action. Retrying a **Send** that failed keeps its original question and its request identity, even if you have edited the draft since; the later edit stays unsent. Retrying the preparation of a draft never sends the question.

## Read answers and sources

The answer grows as it arrives. Reading earlier messages stops the view from pulling you back to the bottom. **Load earlier messages** reads older history.

- **Sources** appear as the usual link chips. Hover, focus or click one to look at its source as you can read it now. **Open** and **Open beside** lead to that source. A reference gives no access. A source you can no longer read does not reveal its current details.
- Hover or focus an answer for **Copy answer** and **Retry answer**. The same controls are there on touch. **Retry answer** asks again from the question before it, under your current settings and access. If its send is uncertain, retrying that answer again keeps the same request identity and the original context. After it was acknowledged, another **Retry** on purpose starts a new answer.

## Longer conversations

When the conversation comes close to the context window of the model you chose, Ask summarizes its oldest messages with that same model. **Earlier messages summarized** is a visible message that you can expand. The originals stay in the history, and later questions send that summary and the recent messages. A source that changed or was withdrawn is checked again before a summary is used again.

Summarizing counts as your Ask usage, through the payer of the conversation and the usage limits that apply now. It does not choose a separate model for summaries.

What bounds a conversation is the chosen model’s context window, counted in tokens with that model’s measured estimate. The answer allowance is `ask.max_output_tokens` (4096 by default). Administrators can set it and the optional `ask.context_tokens` at system or organization scope; the lowest value applies. Unset context uses the model’s own window; a configured value can reduce that window for Ask without changing ordinary threads. Ask reserves the smaller of the configured answer allowance and half the known window, with a minimum of one answer token. Instructions and tools also need input room, so a very small context can leave no room for any question. There is no item-count cutoff and no limit in bytes.

When the history does not fit, the oldest messages are summarized as above. Completed reads are fitted before the page in view. A source that cannot fit is shortened in tokens, ends in a line that says so and where the whole source is, and the answer goes on from there; open that link, or ask for a narrower part, to read more. Every later step checks the current source again and fits it to the available tokens, including content added since its first reading.

A model whose window has not been recorded is not given a small one: Ask sends the conversation as it is. If the provider refuses the combined input and answer allowance, Ask retries with a smaller answer allowance first. A refusal even with the smallest answer establishes a smaller window and triggers a summary of the oldest messages; record the model’s context size to avoid extra requests. If even a summary cannot fit, Ask says the conversation is too long for the selected model: start a new conversation or **Keep as thread**. If the question and what it read are too long by themselves, no summary of earlier messages helps, and Ask says so: ask a narrower question or start with fewer sources. No history disappears silently.

The numbers inside the read tools are settings too, each with today’s value as its default: `ask.docs_hits` (5 guide pages per search), `ask.docs_excerpt_tokens` (1,000 tokens of each), `ask.max_operations` (20 operations per catalogue search) and `ask.max_links` (50 links kept from one read). The step and tool counts per question are `ask.max_steps` (16) and `ask.max_tools` (32).

When Ask cannot take a question, the message says why in its own words: the model or provider is not allowed or not ready, a usage limit was reached, or the organization cannot be opened as you. Ask answers as you are in an organization, whatever the **As admin** switch says; an administrator who is not a member of an organization can inspect its conversations but not ask in it.

## One organization per conversation

The **In** chip names the root organization of the conversation, including its departments and projects. That boundary never changes. Ask reads only what you may read inside it. A resource that was shared in from another organization is available only at the level it was shared at, with no access to the unrelated work of that organization.

You can ask which providers or agent profiles are shared into a topic or thread that you can read. The answer shows what the work shares, with the limits on model and effort for that place. Managing a resource elsewhere does not widen those shares. Its private home, its runtime, its usage and unrelated work stay outside the answer.

The page in view gives context. Opening a page of another root organization starts, or resumes, the conversation of that organization. On a page outside every organization, such as the Inbox or **Help**, Ask uses the organization selected in the rail, then your personal organization if none is selected. To ask about a source in another root organization, open a page there and ask in that conversation.

**History** lists your conversations by organization. Resuming one keeps its original boundary. If its organization is not that of the page in view, the page is not passed into the conversation. Before a resumed turn, Ask checks earlier tool results again, reads the page again when it is in scope, and finds the model and the payer that apply now. Your own past answers stay in your history even when a source is no longer available.

Conversations normally live in your personal organization. If personal organizations are turned off, they live in an unlisted place in their root organization. The root organization still governs access, inspection and retention. If you lose access to it, those conversations close for you.

## Privacy, cost and retention

Ask reads as you and makes no change. It can search, open sources you can read, read their history, and consult the user guide that comes with this build. It does not lend your access to another person, and it does not give instructions to a thread that is working.

The admins of a conversation's root organization can inspect and export it. An admin inspection is read-only: it cannot send, retry, stop or keep someone else's conversation. Turn on the existing **As admin** switch, open **History**, choose **Inspect this organization’s conversations**, then open a row. **Export conversation** downloads the permitted conversation as JSON.

You normally pay for questions you ask, unless the effective Ask setting selects its setting owner's provider use. The panel shows the effective payer. Your organization's applicable per-person and shared usage caps still apply.

By default, a conversation is deleted for good **30 days after its last message**. The effective period is held within the minimum and maximum set by its root organization's admins; the panel shows that period and expiry. There is no second recovery window after expiry.

Before expiry, choose **Keep as thread** to create a Private permanent thread in an authorized destination topic, or use the authorized default. The review names the organization the new conversation will search and be governed in. A copy into another organization takes that destination’s knowledge, tools, model choices, policies and billing from its first turn; the source’s fixed root stays unchanged. Cross-organization copies still require the source’s copy and Export controls. The original temporary conversation becomes kept; the new thread follows ordinary thread access and retention. Changing its agent later uses the normal thread controls.

Unavailable **Export conversation** and **Keep as thread** controls stay
focusable and explain their own state on hover or keyboard focus. No
conversation yet, a conversation being checked, a failed read and a refused
action are distinct. Keep waits for a running answer and is unavailable during
admin inspection; permitted Export remains available in either state.

## Start a permanent question thread

In a topic, open **New thread**, or **Change** beneath its starter, then choose **Start read-only questions**. Review the choice of model and the access of the new thread: **Private** or **Use this topic’s default access**. You can include the first question. This program needs no environment and no agent profile.

A permanent question thread keeps its normal timeline, files, saved draft and **Stop**. Its model chooser shows the saved preference. Choose another API model that is allowed for the next question, or **Default** to reset that preference when the next question is accepted. The permissions, the access to the model and the limits on usage of each sender apply as they stand now.

## Installation addresses

A link copied on another address that this Orch installation declares opens on the address you are already using. **Copy link** uses the public address when one is set. The selection of a block, a turn, a message or a line of a file stays attached to the link.

Administrators declare other origins of the browser, beside the public address, in the setting for alternate URLs. An alternate IP address and its port must be declared explicitly. Another website with the same path, a host that looks alike, or an address that is not declared stays an external link. Recognizing a link never shares its target and never sends your credentials to that website.

## Ask, Help and Side thread

**Help** holds the written guide that comes with this build and asks no model. **Ask** uses a model to answer from that guide and from the work you can read. **Side thread** starts a separate side thread with the agent of a working thread, in its read-only mode that was verified. Use it for a question about the current work of that agent. See [Follow a running thread](running-thread-views.md).
