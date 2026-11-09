---
group: work
screens: /workbench, /org/$slug, /project/$projectId, /topic/$topicId, /thread/$threadId, /doc/$documentId, /chat/$chatId, /search, /resources, /inbox, /everything, /help, /settings/interface
keywords: Ask, question, answer, sources, citation, organization, root org, history, retention, private, model, effort, model policy, floating window, Side thread
summary: Ask read-only questions about the page in view and the work you can read in one organization.
---

# Ask about your work

Ask is your personal, read-only conversation about the page in view and the work you may read in one organization. Answers cite their sources.

## Open Ask

Choose **Ask about this page** above Help in the left rail. On a phone, use the same button beside the finder. **Shift+F1** opens it too; Help's shortcut editor lets you change that binding.

Ask starts docked on the right. **Pop out** puts the same conversation and draft in a floating window. Drag its title to move it and its lower-right handle to resize it. Focus either handle and use the arrow keys for the same actions; Shift moves farther. **Maximize**, **Restore size** and **Dock** change its placement. Your window size and position are kept for your account on this device.

## Write a question

Type a question and press **Enter** to send. **Shift+Enter** adds a line. Confirming text in an input method does not send the question.

Use the paperclip, paste or drop files to attach them. The usual attachment chips show progress and let you remove or retry a file. Sending waits for unfinished uploads. Type **@**, or choose **Mention a resource**, to find a readable document, thread, chat or resource in this conversation's organization. Choosing a reference adds it to the draft; it sends nothing by itself.

The model chip lists the usable entries in this root organization's **Ask models** setting (`ask.models`). **Default · model** follows the first usable entry in that ordered list. Choose another entry and your reasoning effort through the same model-and-effort control used by threads. Effort follows the model’s default until you choose it. A model that declares no effort has no effort rows. Other declared choices stay within the provider's constraints. Your personal choice, including its parameters, is kept with the conversation.

Ask models have their own policy list in **Settings → Automation**. At system scope, a person who may edit it can add or remove enabled API models and move them up or down. Entries store the provider and model only. At an organization, the list comes from above: an editor can remove entries to narrow it, but cannot add or reorder them; the order from above is retained. Effort and other parameters stay with your personal choice in the chip. Subscription-only providers are not Ask models. Current organization policy and usage caps apply to every turn.

When the list is unset, empty or has no usable entry, including when model policy excludes every entry, the panel shows the reason and has no message composer. A link to the setting appears only when you may edit it. Otherwise, the explanation remains visible so you can ask an organization admin to configure a usable model.

While an answer runs, **Stop** ends that turn. You can continue with another question. An uncertain send keeps your draft; retrying the unchanged question uses the same input identity. Docking or popping out keeps the draft and pending attachments in place. Drafts save after a pause in typing through the same mechanism as normal threads; the small draft mark shows saving or failure.

If sending, stopping or preparing a draft fails, **Try again** repeats that action. Retrying a failed Send keeps its original question and request identity, even if you have since edited the draft; that later edit stays unsent. Retrying draft preparation never sends the question.

## Read answers and sources

The answer grows as it arrives. Reading earlier messages stops the view from pulling you back to the bottom. **Load earlier messages** reads older history.

Sources appear as the usual link chips. Hover, focus or click one to inspect its current readable source; **Open** and **Open beside** lead to that source. A reference grants no access. A source you can no longer read does not reveal its current details.

Hover or focus an answer for **Copy answer** and **Retry answer**; the same controls are available on touch. Retry asks again from the preceding question under your current settings and access. If its send is uncertain, retrying that answer again keeps the same request identity and original context. After acknowledgement, another explicit Retry starts a new answer.


## Longer conversations

When the conversation approaches the chosen model’s context window, Ask summarizes its oldest messages with that same model. **Earlier messages summarized** is a visible, expandable message. The originals stay in history; later questions send that summary and the recent messages. A changed or withdrawn source is checked again before a summary is reused.

Summarizing counts as your Ask usage, through the conversation’s current payer and usage caps. It does not choose a separate summary model.

Administrators can set `ask.max_output_tokens` (4096 by default) and the optional `ask.context_tokens` policy at system or organization scope; the lowest value applies. Unset context uses the model’s own window; a configured value can reduce that window for Ask without changing ordinary threads. Fitting reserves the answer’s output tokens and the turn’s tool-result allowance (`ask.turn_tool_bytes`), converted with the chosen model’s measured token estimate. There is no item-count cutoff. If the model’s window has not been recorded, Ask uses the existing conservative handoff estimate; record its context size or reduce the reserves if they do not fit.

If the reserves leave no room, the reason names `ask.context_tokens` and the reserves to change. If even a summary cannot fit, Ask says the conversation is too long for the selected model: start a new conversation or **Keep as thread**. No history disappears silently.

## One organization per conversation

The **In** chip names the conversation's fixed root organization, including its departments and projects. That boundary never changes. Ask reads only material you may read inside it. A resource shared in from another organization is available only at the shared level, without access to the other organization's unrelated work.

Ask which providers or agent profiles are shared into a readable topic or thread. Those results show the work shares and their model or effort limits for that place. Managing a resource elsewhere does not expand those shares. Its private home, runtime, usage and unrelated work stay outside the answer.

The page in view supplies context. Opening a page of another root organization starts or resumes that organization's conversation. On a page outside every organization, such as Inbox or Help, Ask uses the organization selected in the rail, then your personal organization if none is selected. To ask about a source in another root organization, open a page there and ask in that conversation.

**History** lists your conversations by organization. Resuming one keeps its original boundary. If its organization differs from the page in view, that unrelated page is not passed into the conversation. Before a resumed turn, Ask rechecks earlier tool results, rereads the page when it is in scope, and resolves the current model and payer. Your own past answers remain in your history even when a source is no longer available.

Conversations normally live in your personal organization. If personal organizations are disabled, they live in an unlisted place in their root organization. Their root organization still governs access, inspection and retention. Losing access to it closes those conversations for you.

## Privacy, cost and retention

Ask reads as you and makes no changes. It can search, open readable sources, read their history and consult the bundled user guide. It does not lend access to another person or instruct a working thread.

The panel states that the conversation's root-organization admins can inspect and export it. An admin inspection is read-only: it cannot send, retry, stop or keep someone else's conversation. Turn on the existing **As admin** switch, open **History**, choose **Inspect this organization’s conversations**, then open a row. **Export conversation** downloads the permitted conversation as JSON.

You normally pay for questions you ask, unless the effective Ask setting selects its setting owner's provider use. The panel shows the effective payer. Your organization's applicable per-person and shared usage caps still apply.

By default, a conversation is deleted for good **30 days after its last message**. The effective period is held within the minimum and maximum set by its root organization's admins; the panel shows that period and expiry. There is no second recovery window after expiry.

Before expiry, choose **Keep as thread** to create a Private permanent thread in the same root organization. Review the destination topic, or use the authorized default. The original conversation becomes kept; the new thread follows ordinary thread access and retention. Changing its agent later uses the normal thread controls.

## Start a permanent question thread

In a topic, open **New thread**, or **Change** beneath its starter, then choose **Start read-only questions**. Review the model choice and the new thread's access: **Private** or **Use this topic’s default access**. You can include the first question. This program needs no environment or agent profile.

A permanent question thread keeps its normal timeline, files, saved draft and **Stop** control. Its model chooser shows the saved preference. Choose another allowed API model for the next question, or **Default** to reset that preference when the next question is accepted. Each sender's current permissions, model access and usage limits apply.

## Installation addresses

Links copied on another declared address of this Orch installation open on the address you are already using. **Copy link** uses the configured public address when one is supplied. The source's block, turn, message or file-line selection stays attached to the link.

Operators declare other browser origins with `[system] alternate_urls` beside `public_url`. An alternate IP address and its port must be declared explicitly. Another website with the same path, a similar-looking host or an undeclared address stays an external link; recognizing a link never shares the target or sends your credentials to that website.

## Ask, Help and Side thread

**Help** contains the written guide bundled with this build and asks no model. **Ask** uses a model to answer from that guide and your readable work. **Side thread** starts a separate aside with a working thread's agent and its verified read-only mode. Use it for a question about that agent's current work; see [following a running thread](running-thread-views.md).
