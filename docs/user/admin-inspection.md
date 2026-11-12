---
group: operations
screens: [/admin, /admin/$section, /settings/security, /settings/tokens, /storage, /resources/environments/$id, /resources/machines/$id]
keywords: [admin, sessions, tokens, revoke, uploads, storage, snapshots, imports, exports, audit]
summary: Inspect stored work, sessions and tokens through audited administrative views.
---

# Inspect stored data, sessions and tokens

With admin mode on, system admins can inspect administered stored objects through their native views. Owner, age, size, attachment and activity facts make orphaned data visible. An administrative opening or revocation is marked and audited; secret credential values are not displayed by an inventory.

## Where

Open [Management](/admin). Users gives the selected person's Sessions and Tokens; the system Tokens list collects API tokens across people. Storage lists uploads, snapshots, exports, import reports and machine storage entries. Its Open action leads to the permitted native view, recorded export manifest or read-only import report. Inspecting someone else’s report offers no Undo. Partial and expired uploads have no Open action.

[Uploads](/storage?view=uploads) in personal settings lists your own unattached uploads. Delete cancels an unfinished upload or releases your completed custody; attached files are kept.

For your own browser sessions use [Security](/settings/security); for your API tokens use [Tokens](/settings/tokens). Environment Files lists its saved file snapshots. A machine's Storage tab reports the parts of its data directory, including credential storage by name and size.

## Example

An upload is no longer attached to a message. Find its Storage row, check who uploaded it, its age and size, then Open it using the indicated admin access. Before revoking a person's old token, inspect its owner, targets, allowed operations, last use and expiry. Revoke the specific token and check the resulting activity; removing one token does not mean every browser session was ended.

## What to expect

A session row identifies the browser, network address, creation, last activity and expiry. Sign out ends the selected session. Sign out all ends every session of that person, including your current browser when you inspect yourself; you must sign in again. A token list shows scope and activity, never its recoverable secret. Partial machine uploads retain their actual state rather than looking complete.

An import report or export manifest is different from the package download. Downloading the package still uses the owner's ordinary access or an audited administrator opening. Unknown sizes remain unknown. Review the current state before deleting data. Read [storage cleanup](storage-cleanup.md), [import and export](import-and-export.md), and [account security](account-and-profile.md).
