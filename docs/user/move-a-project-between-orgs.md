---
title: Move a project between orgs or deployments
screens: /org/$slug, /project/$projectId, /topic/$topicId, /doc/$documentId, /settings/exports, /settings/imports, /thread/$threadId
keywords: import, export, package, orch package, move, copy, project, org, deployment, archive, zip, undo, conflicts, people, plain files, markdown, 导入, 导出, 包, 移动, 项目, 撤销, 冲突
---

# Move a project between orgs or deployments

An **Orch package** is an export that Orch can read back. Export a place as a package, then
import the file into any place where you may create things: another org, another project,
or another Orch deployment. The import first shows what it would do and writes nothing;
you choose, then it runs as a job you can follow, cancel and undo.

## Export a project as an Orch package

Open **Export…** on an org, project, topic, thread, chat, folder or document. In **Form**
choose **Orch package (can be imported)**. Files and comments are on; Versions is your choice.

The package is one zip. It holds the structure (places and document folders), documents with
their files and, if you chose them, their comments and versions, chats, and threads as
history. Each thing has a record Orch reads back and a Markdown copy people can read. Documents
also have an HTML copy that keeps their layout and links to the files beside them. A manifest lists every part with its checksum, so a damaged or changed
file is noticed on import. Everything the export limits leave out is listed in the manifest
as left out, with the reason.

Who is named in the package follows the same rules as any export: people appear by name and
username; email addresses are written only when a system administrator exports. The package carries
who may see and do what on a place or a document only where you manage its access.

## Import a package

Choose **Import…** in the menu of an org, project, folder or topic, or **Import** in a
documents list. Five steps:

1. **Source**: drop the package or choose its file. Orch recognises it from its manifest.
2. **Destination**: the place, and for a package made of a place, whether it comes back as
   a new place inside the one you chose or into it.
3. **Review**: the dry run. Counts, size against the limits, the objects with the action
   each would take, the people, the conflicts and what is lost. Nothing exists until you
   choose **Import**.
4. **Import**: one live line with the phase (checking, matching, creating, report), items
   and elapsed time. You may close the dialog; the job goes on in the status bar. **Cancel
   import** stops it; what it made stays and the report says so.
5. **Report**: one row per object (created, kept both, replaced, unchanged, skipped,
   failed), with its reason in words, downloadable as JSON or CSV. **Your imports** in
   settings keeps the reports.

If an import fails before it writes any object rows, the report still shows why it failed.

You can bring in only what you could make there by hand. A thing you may not create, or a
kind that cannot lie in that place (a document where documents are switched off, a chat
under a document folder), is shown with its reason and is skipped; the rest still comes.
Each batch checks your rights again, so a change of role during a long import is honoured.

An org in a package comes back as a project; an org's own settings, members and sharing are
not an import's to create.

## What happens to people

Each person named in the package is matched, first match wins: the same account id (a
package from this deployment), a confirmed email address, the username, a choice you make
in the **People** tab, and otherwise an **imported person** label. Matching never creates an
account.

**By accounts** writes what the package says an account wrote as that account. It is offered
only to an Admin of the place. Documents, their blocks and saved versions follow the same people mapping as comments and messages. Ownership stays with the importer. **By me, with original names** makes you the author of
everything and keeps the original name where the thing can show it: "originally by …" in the
report, at the start of a chat message or comment, and with the object. Someone
who is not an Admin of the place gets this form, and it is the safe default.

## Conflicts and re-importing

An object that meets something already there is found two ways.

- **Same source**: the thing was brought in before by an import of the same package or of the
  same place. Default **Replace**: the document gets a new version and keeps its identity,
  so links to it still hold; unchanged things are left alone. Chats and threads are merged:
  messages that came before are not repeated, new ones are added.
- **Same name**: a different thing of the same kind and name lies there. Default **Keep both**:
  the new one takes a free name such as "Notes (2)".

The **Conflicts** tab sets the default for each kind of meeting; a row's own choice
overrides it (**Create**, **Keep both**, **Replace**, **Skip**). Skipping a place or folder
skips what lies in it. Importing the same package twice into the same place changes nothing
the second time except what was edited in the package since.

References between the imported things, mentions, file links and relative links are
rewritten to the new places in one pass. A reference to something outside the package keeps
its label as words.

## What does not come back

An Orch package is a copy of work, not of a running system. These do not come back, and the
**Not imported** tab says so before you start:

- native sessions of agent programs (a thread comes back as history, bound to nothing);
- workspaces and other environment files;
- secrets;
- usage and audit records;
- what people keep for themselves (read marks, pins, drafts) and approvals;
- images that live on the web (they stay links).

Things the export left out (too large, not exportable, no permission) and anything that came
back changed are counted with the reason.

Sharing comes back only when the package carries it, the **Sharing** switch is on and you hold
Manage access at the destination. Each person it names is given again what they had, on what the
import made, as you would have given it by hand: what you may not give, what a lock above decides and
a person the place does not let be named are left out and counted, and a person who is not an
account here is left out too. What was there already keeps its own sharing.

## Set up an imported thread

Open the thread to read its imported messages. Choose **Choose an agent** in the composer,
then choose a provider, agent, model and environment and select **Set up**.
The imported messages stay as history; the next message starts local work with that setup.
The header marks **Imported history**. **Needs setup** means no local agent is chosen yet;
**Choose agent and provider…** in the header opens the same setup chooser.

## Undo an import

**Undo import** in the report shows its plan first: what the import made, and what stays because people added to it or changed it
since. If a check blocks Undo, its reason appears in the dialog. With no blockers, confirming deletes what the import made
and nothing else, the usual way, so you can still bring it back until the date shown.
A document somebody else changed stays; so does anything written in a place or folder of the import's since, and the places and
folders that hold it. Only what you may delete now is deleted. Things you imported into that already existed (replaced versions) are
not rolled back; the new versions stay in their history.

After Undo, the report keeps its results and download and no longer offers to open the removed place.

You can import the same package again after Undo. It creates a new copy and leaves the
removed copy available for restoration. Importing that new copy again keeps its identity.

## Import plain files

The same wizard takes files that Orch never wrote. **Files** brings Markdown, HTML, Word and
text; **Folder** keeps the folders as document folders; **Archive** takes a zip, or a tar
(`.tar`, or packed with gzip, bzip2, xz or zstd: `.tgz`, `.tar.gz`, `.tar.bz2`, `.tar.xz`,
`.tar.zst`) of such files with their images; a zip that holds other zips (as a Notion export
does), or several zips chosen together, is read as one. Relative links and images between the
files are resolved to the new documents and attachments; a file that cannot be found is named
in the report and the text keeps the link as written.

Limits for what one import may bring are set by an operator; they follow what an export may
hold unless changed.
