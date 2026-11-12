---
group: work
screens: [/thread/$threadId, /resources/environments/$id, /workbench]
keywords: [restore, checkpoint, files, before message, touched files, whole directory, changed since, undo]
summary: Review and restore recorded files to their state before a selected request.
---

# Restore files to before a message

Restore returns recorded files to their state before your selected request. Review its plan first: it can restore the **Whole directory** when that state is available, or **Files the agent changed** when only recorded edits are available. The conversation stays as a record of what happened.

## Where

Open the thread through [navigation](/). Hover or focus your message, or use its touch action row, and choose **Restore files to before this message…**. The plan names the available form and its paths before any files are written. **Review and undo files…** remains available for individual changes.

## Example

An agent changes a configuration in one turn, edits it again in the next, and creates a temporary file. Restore from the request before those turns. In the touched-files form, the plan uses the configuration's first saved pre-image and removes the recorded added file. If you edited the configuration afterwards, the plan marks **Changed since**; review it and explicitly choose overwrite only if you want that later change replaced.

## What to expect

The touched-files form follows this thread's recorded file changes in the same directory, including later turns. Changes made by commands outside the agent's file tools are not covered by that form. Whole-directory state can be unavailable because recording is off or the index has reached its limits; the plan says why and offers the recorded-edit fallback where it exists.

Restore runs as a job and records an undo state for the paths it changes. Use **Undo this restore** to reverse that restore where offered. A conflict is not silently overwritten. Inspect the environment's Files and index state from [Environments](/resources/environments). See [running thread views](running-thread-views.md) for Review and line comments.
