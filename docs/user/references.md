---
title: References and access
screens: /thread/$threadId, /doc/$documentId, /chat/$chatId, /place-chat/$nodeId, /org/$slug/messages, /search, /shared, /documents/$nodeId, /document-folder/$folderId, /inbox, /approvals
keywords: reference, preview, chip, copy reference, link, block, turn, message, file, line, description, read_shared, edit_document, locked, deleted, request access, quick grant, expiry, history
---

# References and access

## Preview and open

A reference names a place, thread, document, chat or file. Hover or focus its
chip to preview it. Click or press Enter to keep the card; Escape closes it.
The card shows the permitted title, place, description and last change. A
reference to a part also shows that block, turn, message or text line.

**Open** goes to that part. **Open beside** keeps your current work in view.
**Copy reference** copies words an agent can follow. Opening or copying grants
no access. An unreadable reference keeps its shared name without revealing
current metadata; a deleted target says **Deleted**. Its Open actions are disabled;
**Copy reference** keeps the stored address and labels it **Deleted**.

## Locked references

A grey reference with a lock is a real reference you cannot currently open. It
keeps the name that was shared, which may be older than the current name.
References made before names were kept show their kind. A deleted target reads
**Deleted**.

The locked card shows no current description, place, owner or file details. If
access can be requested, its form starts with the least access needed. Add an
optional reason and press **Send**. Its owners will be asked without revealing
their names in the card. A waiting request can be taken back.

Only one request by you may wait on the same thing. The usual pending-request
limit and request expiry apply. People outside the org can ask only when the
thing may be shared with them.

A direct chat belongs to its two participants: its locked card has no request form.
Ask a participant to forward or quote what they choose to share. A group chat offers
**Ask to join**. A grant adds you as a member for the chosen duration. **Always** is
ordinary permanent membership. A timed member is marked **Until** in the member list;
expiry ends membership and greys the reference again.

## Giving access

People who may decide see the same decision on the reference card and in Inbox
or Approvals. Pick a duration to give the requested access to that person on
this one thing. The shipped choices are one day, seven days, thirty days and
**Always**. The place can change these choices. **Other…** opens the full decision
for fewer permissions, narrower options or another end date. **Deny** accepts an
optional note.

The decision is recorded and the requester is told. When a timed grant ends,
access is checked again and the reference greys if no other access remains.
See [Who can see and do what](who-can-see-and-do-what.md#requesting-access)
for how requests are routed and settled.

## Write a reference

Give an agent a path from its shared topic, a path starting at `/`, or `id:`
followed by the thing's identifier. Add `#` and the part number to name a block,
turn or message. Put it in a Markdown link or backquotes. For example,
`[Design](</Web/Design notes#3>)` names part 3 of that document. Angle brackets
keep a path containing spaces or parentheses together.

In messages, script addresses stay as text. Web links can include `https://` or `http://`.

Prefer **Copy reference** when handing work between people and agents. Its
identifier survives a rename. Document block references keep the block's
identity when surrounding blocks move. A copied part number describes the
part at the time of copying; use the live chip when reading the current work.

An agent's reading tools use the same names and part numbers. Reading a shared
document at brief depth includes its outline. Reading and searching return
text; editing and messaging return their structured result. Edits and messages
also report which references resolved and which stayed as ordinary text.

## Files and lines

The Files row menu, code view and attachment preview offer **Copy reference**.
For text, it can name the cursor line or the first selected line. An environment
file reference includes its environment and relative path; an attachment keeps
the identity of the document or chat file. Copy the offered reference instead
of guessing a path on another machine.

A file card shows its owner, path and size where known, and a short text excerpt
when available. Opening uses the existing file viewer and the current file
permissions. A preview is a glance, not a copy of the whole file.

## Describe a document

Write a description below a document's title to help people and agents choose
what to read. Its first line appears in shared listings and previews. With no
description, Orch uses the first line of the first paragraph.

Agents may describe a document through `edit_document`; a configured automation
can maintain it where the place permits. A description helps discovery and
does not replace the document or expand its sharing.

## Sharing with an agent

Copy reference writes the same form an agent can follow, such as
`[Rollout plan](id:019a0000-0000-7000-8000-000000000001#3)`. Agents can also
write shared paths in Markdown links or backquotes. An agent without access
reads the shared name followed by **(no access)** and can tell its person.
Agents do not request access themselves.

An agent's **Where you are** message gives its own conversation path. It can
read earlier turns, search that path, or download the whole conversation for
its own tools, including turns from before its context was shortened.
