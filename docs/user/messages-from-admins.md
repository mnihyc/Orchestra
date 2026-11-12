---
group: operations
screens: [/inbox, /org/$slug/settings/$section, /admin/$section]
keywords: [admin messages, members, system, Inbox, terms changed, Markdown, banner]
summary: Read administrative messages in Inbox and follow their sender, audience and history.
---

# Read and send messages from admins

Admins can send an authored message to an organization's members or to everyone active on the system. Each recipient gets an Inbox item that opens the message's title, Markdown body, sender and time. These messages are separate from announcements that tell agents about work.

## Where

Read a message from [Inbox](/inbox). Organization member managers send through **Settings → Members → Message all members…**. System admins use **Message everyone…** in [Management](/admin) with admin mode on. The sent list beside the composer records earlier messages and their recipient counts.

Write a title and body, check Preview, and read the recipient count before Send. For a system message, **Also show as the banner until** optionally puts it in the existing system banner until the chosen date.

## Example

Studio changes its terms. Open the terms field's **Tell the members…** link, review the prefilled title, and write what changed and when it applies. Check **Sends to N members**, then send once. Members of Studio and its suborganizations receive the message, including the sender. A guest invited only to a topic does not.

## What to expect

The delivery job reports progress. Inbox delivery always applies; email and other channels follow each person's [notification choices](/settings/notifications). A system message reaches active people across the system. Sending is recorded in audit history with its scope and recipient count.

The count before Send is a current preview. The confirmation and sent list show the actual recipient count.

If a background read cannot connect, the last readable message or sent list stays visible and dimmed. Choose **Try again** to read it again. When current access ends, its content leaves the view.

Read [organization terms](organization-terms.md) for the separate joining requirement, and [notices and announcements](notices-and-announcements.md) for agent announcements and notices about one thing.
