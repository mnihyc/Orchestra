---
title: Follow progress and arrange status items
screens: /workbench, /thread/$threadId, /resources/environments, /resources/environments/$id, /resources/hosts/$id, /resources/providers/$id, /resources/mounts/$id, /settings/exports, /admin/$section
keywords: progress, counting, copying, stalled, stopping, cancel, rate, time left, status items, speed, elapsed, layout, migration
---

# Follow progress and arrange status items

## Read progress

A plan can count files and bytes in the background. Counting shows what it
has found so far. A dash is an unknown total; a completed empty location is
zero. When the count is required, confirmation says **Waiting for count…**.
Other counts can continue while you review the plan.

A running copy shows its phase, files, bytes, rate and elapsed time. A time
left marked `~` is an estimate. A rate marked `≈` is estimated too. A job
queued behind another change says so. Leave the progress view and return
without starting a second copy.

## Stop or retry

Use **Stop counting** for a plan measurement, or the job's cancel action for
the operation itself. **Stopping…** means the machines are still ending their
work. A delayed stop names the machine still being awaited. Cancellation is
complete only after that work has stopped.

A stalled line keeps the last figures and says when they last changed. Check
the named machine and reason. A failed count or stopped optional check offers
Retry; retrying a measurement does not confirm the plan. A job failure names
its phase. Read its outcome before repeating a change that may have committed.

## Arrange status items

The status bar, thread header and live line can show speed, elapsed time,
tokens, cost, context, usage and resource load. Only reported values and
explicit estimates appear. A provider limit and a model's context are separate
facts; opening their card explains each one.

Open an item's context menu to hide it or Move to another place. Choose
Customize, or Layout → Status items, for the common editor. Select a place,
check the items to show, and drag their handles or use Alt+Up / Alt+Down to
order them. More status items holds values that do not fit.

These choices belong to your layout on this device. They do not change the
agent's work. Reset restores the status defaults in all three places; Reset layout restores them
with the rest of the frame.

## During a server update

The reconnecting banner can show the database migration, completed steps,
reported rows and elapsed time. Unknown row counts remain unknown. A failed
database update points to the server's update log; after success the page
reconnects. Operators should follow the upgrade procedure and keep the log.
