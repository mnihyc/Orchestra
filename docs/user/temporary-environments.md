---
title: Temporary environments
screens: /resources/environments, /resources/environments/$id, /resources/templates/$id, /resources/hosts/$id, /thread/$threadId, /inbox
keywords: temporary, disposable, ephemeral, environment, idle, keep, export, download, archive, retry, failed setup, 临时, 一次性, 环境, 空闲, 保留, 导出, 归档, 重试
---

# Temporary environments

## Create a temporary environment

For a quick experiment, switch on **Temporary** in **Add environment**.
**Belongs to** starts at the place you are viewing. Choose a template and host,
or open **Add environment** on either resource's page to start with it
selected. The environment belongs to you and takes its settings from that place.

In **New thread**, choose **New** under **Where it runs**, then switch on **Temporary**.
That environment follows the first thread you make in it. It is not recorded as
the topic's lasting default, so a later thread does not unexpectedly inherit an
environment scheduled for deletion.

An unavailable agent is disabled with its reason. **Create thread** checks the
whole setup before making anything. If the setup is refused, no environment or
thread is made, and your message stays in the dialog. Starting from a project
also creates its topic only when that setup is accepted.

## If setup fails

The thread explains what could not be prepared and keeps your first message in
the queue. Choose **Retry** to try starting the same environment again, or
**Run somewhere else** to review a switch to another environment. You can still
choose another environment while a retry is waiting. You do not need to send
the message again; it waits until the thread can run.

## When it is deleted

The environment page shows **Temporary; deletes …** on every tab. A running
turn or a job using its files keeps it alive. After activity stops, its idle
time starts again. The place's **Temporary idle time** setting starts at
24 hours and can be changed in that place's Settings. The environment's own
setting can override it where permitted.

An environment made for a thread is also deleted when that creating thread is
archived or deleted. Another thread's archive does not trigger this. Archiving
starts a one-hour warning period; the files do not disappear immediately.

## Keep or export

An hour before removal, its owner gets an Inbox notice with **Keep** and
**Export files**. If Orch was offline when a deadline passed, the warning still
gives the owner a full hour. The time shown on the page is that actual deadline.

**Keep** makes it lasting and cancels automatic removal. You can do this from
the page or the notice until removal begins. A failed request says what
happened and can be retried.

**Export files** copies the whole workspace, including files outside a thread's
starting directory, into a download. It needs the machine to be online. The
copy runs as a background job; closing its window is safe. Download the ZIP
from that window or the completion notice in Inbox. This private copy remains
available to the person who exported it after the environment is deleted.
An export in progress holds cleanup until it finishes and leaves another
warning opportunity. An export error is a reason to keep the environment and
retry, not a promise that a copy exists.

## What is deleted

Automatic removal deletes the environment and the workspace and home that
Orch made for it, including their owned volumes or directories. It does not
keep them in the ordinary deleted file retention window. Existing directories you
chose to use, shared mounts and other environments' files are kept. Export
copies remain separate from the deleted environment.

Cleanup retries through the ordinary job queue if its machine is offline or
refuses removal. A replacement directory is never guessed to be the old owned
directory; reconnect the machine and resolve its reported error instead.
