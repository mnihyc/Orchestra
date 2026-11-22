---
group: operations
title: Move a project between orgs or deployments
screens: /org/$slug, /project/$projectId, /topic/$topicId, /doc/$documentId, /settings/exports, /settings/imports, /thread/$threadId
keywords: import, export, package, orch package, move, copy, project, org, deployment, archive, zip, undo, conflicts, people, plain files, markdown, 导入, 导出, 包, 移动, 项目, 撤销, 冲突
summary: Export an Orch package and review its import into another organization or deployment.
---

# Move a project between orgs or deployments

An **Orch package** is an export that Orch can read back. Export a place as a package, then import the file into any place where you may create things: another organization, another project or another Orch deployment. The import first shows what it would do and writes nothing. When you choose to go on, it runs as a job that you can follow, cancel and undo.

## Export a project as an Orch package

1. Open **Export…** on an organization, project, topic, thread, chat, folder or document.
2. In **Form**, choose **Orch package (can be imported)**. **Files** and **Comments** are on. **Versions** is your choice.

The package is one zip. It holds the structure (places and document folders), the documents with their files and, if you chose them, their comments and versions, the chats, and the threads as history. Each thing has a record that Orch reads back and a Markdown copy that people can read. Documents also have an HTML copy that keeps their layout and links to the files beside them. A manifest lists every part with its checksum, so that a file that is damaged or changed is noticed on import. Everything that the limits of the export leave out is listed in the manifest as left out, with the reason.

Who is named in the package follows the same rules as any export. People appear by name and username, and email addresses are written only when a system administrator exports. The package carries who may see and do what on a place or a document only where you manage its access.

## Import a package

Choose **Import…** in the menu of an organization, project, folder or topic, or **Import** in a list of documents. There are five steps:

1. **Source**: drop the package or choose its file. Orch recognizes it from its manifest.
2. **Destination**: the place and, for a package made of a place, whether it comes back as a new place inside the one you chose, or into it.
3. **Review**: the dry run. It shows the counts, the size against the limits, the objects with the action each would take, the people, the conflicts and what is lost. Nothing exists until you choose **Import**.
4. **Import**: one live line with the phase (checking, matching, creating, report), the items and the time elapsed. You can close the dialog, and the job goes on in the status bar. **Cancel import** stops it. What it made stays, and the report says so.
5. **Report**: one row for each object (created, kept both, replaced, unchanged, skipped, failed) with its reason in words, to download as JSON or CSV. **Your imports** in settings keeps the reports.

If an import fails before it writes any rows of objects, the report still shows why it failed.

You can bring in only what you could make there by hand. A thing you may not create, or a kind that cannot lie in that place (a document where documents are turned off, a chat under a document folder), is shown with its reason and skipped, and the rest still comes. Each batch checks your permissions again, so a change of role during a long import is honored.

An organization in a package comes back as a project. The settings, the members and the sharing of an organization are not for an import to create.

## What happens to people

Each person named in the package is matched, and the first match wins: the same account ID (a package from this deployment), an email address that was confirmed, the username, a choice you make in the **People** tab, and otherwise an **imported person** label. Matching never creates an account.

- **By accounts** writes what the package says an account wrote as that account. Only an administrator of the place is offered it. Documents, their blocks and saved versions follow the same mapping of people as comments and messages. Ownership stays with the person who imports.
- **By me, with original names** makes you the author of everything and keeps the original name where the thing can show it: "originally by …" in the report, at the start of a chat message or a comment, and with the object. Someone who is not an administrator of the place gets this form, and it is the safe default.

## Conflicts and re-importing

An object that meets something already there is found in two ways:

- **Same source**: the thing was brought in before, by an import of the same package or of the same place. The default is **Replace**. The document gets a new version and keeps its identity, so links to it still hold, and unchanged things are left alone. Chats and threads are merged: messages that came before are not repeated, and new ones are added.
- **Same name**: a different thing of the same kind and name lies there. The default is **Keep both**. The new one takes a free name such as "Notes (2)".

The **Conflicts** tab sets the default for each kind of meeting. The choice of a row overrides it (**Create**, **Keep both**, **Replace**, **Skip**). Skipping a place or a folder skips what lies in it. Importing the same package twice into the same place changes nothing the second time, except what was edited in the package since.

References between the things imported, mentions, links to files and relative links are rewritten to the new places in one pass. A reference to something outside the package keeps its label as words.

## What does not come back

An Orch package is a copy of work, not of a system that runs. These do not come back, and the **Not imported** tab says so before you start:

- the sessions of agent programs (a thread comes back as history, bound to nothing);
- workspaces and other files of environments;
- secrets;
- usage and audit records;
- what people keep for themselves (read marks, pins, drafts), and approvals;
- images that live on the web (they stay links).

Things that the export left out (too large, not exportable, no permission), and anything that came back changed, are counted with the reason.

Sharing comes back only when the package carries it, the **Sharing** switch is on and you hold **Manage** access at the destination. Each person it names is given again what they had, on what the import made, as you would have given it by hand. What you may not give, what a lock above decides and a person whom the place does not let you name are left out and counted. A person who is not an account here is left out too. What was there already keeps its own sharing.

## Set up an imported thread

1. Open the thread to read its imported messages.
2. Choose **Choose an agent** in the composer, then choose a provider, an agent, a model and an environment and select **Set up**.

The imported messages stay as history, and the next message starts local work with that setup. The header marks **Imported history**. **Needs setup** means that no local agent is chosen yet, and **Choose agent and provider…** in the header opens the same chooser.

## Undo an import

**Undo import** in the report shows its plan first: what the import made, and what stays because people added to it or changed it since. If a check blocks **Undo**, its reason appears in the dialog. With no blockers, confirming deletes what the import made and nothing else, in the usual way, so you can still bring it back until the date shown.

- A document that somebody else changed stays. So does anything written since in a place or a folder of the import, and the places and folders that hold it.
- Only what you may delete now is deleted.
- Things you imported into something that already existed (replaced versions) are not rolled back. The new versions stay in their history.

After **Undo**, the report keeps its results and its download, and it no longer offers to open the place that was removed. You can import the same package again after **Undo**. It creates a new copy and leaves the removed copy available to restore. Importing that new copy again keeps its identity.

## Import plain files

The same wizard takes files that Orch never wrote.

- **Files** brings Markdown, HTML, Word and text.
- **Folder** keeps the folders as document folders.
- **Archive** takes a zip, or a tar (`.tar`, or packed with gzip, bzip2, xz or zstd: `.tgz`, `.tar.gz`, `.tar.bz2`, `.tar.xz`, `.tar.zst`) of such files with their images. A zip that holds other zips (as a Notion export does), or several zips chosen together, is read as one.

Relative links and images between the files are resolved to the new documents and attachments. A file that cannot be found is named in the report, and the text keeps the link as it was written. An operator sets the limits on what one import may bring. They follow what an export may hold, unless they were changed.
