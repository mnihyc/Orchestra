---
title: Import and export
screens: /workbench, /topic/$topicId, /project/$projectId, /org/$slug, /thread/$threadId, /documents/$nodeId, /document-folder/$folderId, /settings/exports, /settings/imports
keywords: import, export, package, archive, folder, Markdown, HTML, Word, people, conflicts, report, undo
---

# Import and export

## Import

Choose **Import…** from a place's menu or its Documents view. Drop files or choose
files, a folder or an archive. Orch recognises a package from its manifest.
Markdown, HTML, Word and text files can be imported with their local images and
files. A folder or archive keeps its folder structure. Notion's trailing export IDs
are removed from document and folder names; a number distinguishes matching names.
Their links still use the original file paths.

Choose the destination place and, if needed, a document folder. **Browse folders**
walks the place's folders a page at a time; search can also find a separately
shared folder. For a package, choose whether to make its root inside the
destination or put its contents there. The destination's access and settings apply.

## Review an import

The review creates nothing. Check the content, people, conflicts and **Not
imported** tabs. Choose an action for each item and open its preview. Match a
source person to an account, or leave them as an imported-person label. Type at
least two characters to find an account. Skipping a place or folder also skips
its contents. An Admin
can attribute work to matched accounts; otherwise it is attributed to the importer
with the original names. Comments, versions and sharing have separate switches.
Sharing can be restored only where the package carries it and you may manage it.
Content filters and Conflicts show matches on the current page. Use **Next page**
to check later items; switching tabs keeps the plan already checked.

The summary shows the item total and source bytes against the byte limit. An item
limit blocker says the counted items and the limit. A blocker explains what must
change before the import can start. Reimporting can
update an earlier import; the review shows the conflicts and available actions.

## Follow and undo an import

Start the import and follow its progress. Close the dialog while the job runs;
the status bar keeps its progress. Select **Notify me when done** to see a
notice while Orch is open. A cancelled or failed import keeps what was made so far and its report
says what happened.

Lists show one page at a time. **Next page** replaces that page; **Back to first
page** returns to the beginning.

The report lists each result and can be downloaded as JSON or CSV. Choose where to
save it in a browser that supports saving directly to a file. The report streams
to that file; browsers without direct saving explain why Download is unavailable.
**Your imports**
keeps your reports. **Undo import** first shows the reported effects and when
deleted work can be brought back. When the checks are available, confirm the plan
to start undo. When affected items or their access cannot be checked, Undo stays
unavailable and the plan says why.

## Export a package

Open a place's **Export…** action and choose **Orch package (can be imported)**.
Choose what to include and review the plan before starting. Files and comments
are included by default; earlier versions are optional. A package can be imported
into another place. Documents have JSON records and readable Markdown and HTML copies,
with their local files beside them.

Native agent sessions, workspaces, secrets, usage and audit, personal state and
approvals do not come back. Web images remain links. The import review and report
list these and any other losses.
