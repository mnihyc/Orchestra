# Taking things away

Export… is in the menu of a thread, a chat and a document, where it gives one file at once, and in the menu of an org, a project, a
folder and a topic, where it makes one archive of what the person may read there. Everything calls one function,
`openExport(target)` (`ExportHost.tsx`), and one host in the frame of the application draws what it opens. A thread's Export… is the
entry of the menu every node has (`features/navigation/NodeMenu.tsx`), so the header of a thread, the "⋯" of a row and the menu of a
place all say it alike; a chat's is in the menu of its header (`ChatMenu.tsx`, and the group menu); a document's menu must call
`openExport({ kind: 'document', id, name })` and build its entry with `exportEntries` (`menu.tsx`).

Built: the small dialogs of a thread (how much of the conversation, Markdown or JSON, the turns, and the three things only a person's
export can add), of a chat (the form, the messages, the attachments) and of a document (Markdown, one page of HTML or Typst source, the
attachments and the comments); the dialog of a place with its tree of ticks, the kinds, the form, visible content/date options, one line under the options that
says workspace files are not included, and the plan asked again as anything changes; the job followed in the status bar, with a notice and Download when it ends and the reason when it fails; Download
read in pieces, with how far it has come; Your exports (`/settings/exports`) with Download and Remove, a job's progress, the reason a job
failed, and the archives that were removed or ran out of time, each with one line of what it left out (the parts not ticked, and what the
server left out of the rest); the way to ask for the right to take a thing away.

How it reads what a person may take away: Export is a permission of its own (`thread.export`, `chat.export`, `doc.export`), which as
shipped whoever reads a thing holds. Where somebody reads a thing and may not export it, its Export… is switched off with one line and
Request access; in the dialog of a place such things are greyed in the tree where the page can tell (threads of an open topic, documents
of a place whose Export row the person lacks) and counted in the line of the plan, which says how many things were left out and where.

What is not here, and why:

- PDF typesetting can fail to load or reject a document; the dialog shows the error, permits another attempt and offers Print from the document's page.
- Whether the caller may take a thread or a chat away is what the server says (`can.export` on both views); `pending.ts` is the one
  place the screens read it, until they read the members where they are. An archive whose place is gone is drawn as "a place that is gone".
- Your exports is a page of its own; hanging it among the personal settings is done where those pages are joined.
- The line "Workspace files are not included." leads to the Environments tab of a topic, and for any other place to the list of
  environments, because the user guide has no article on taking a workspace away yet; `WorkspaceNote.tsx` is the one place to change
  when it has. A topic says it only where its threads run in an environment; an org, a project and a folder always say it, because
  their view does not tell.
- Documents beside a thread are not rows of the tree: they go with the thread.
- A place with more than two thousand documents lists the first two thousand; a place that is ticked takes all of them all the same.

- `ChatFile.tsx`: A chat as one file: Markdown to read or JSON for a program, a range of messages, and whether the attachments its messages carry go with it, which makes an archive.
- `ChatMenu.tsx`: Export… in the menu of a chat, using its served decision and optional cached owner name without fetching its place; direct and group membership remains authoritative.
- `choices.ts`: What the person chooses for an archive besides which rows are ticked, and the request all of it comes to.
- `DocumentFile.tsx`: A document as one file: Markdown with its losses, a PDF typeset locally, HTML or Typst source, and choices for attachments and discussions.
- `download.ts`: Fetching an archive and saving it, with how far it has come kept where every screen that offers Download can show it.
- `ExportHost.tsx`: The one entry point of Export…: `openExport(target)` from any menu, and the one host in the frame of the application that draws what it opens and follows the jobs that write archives.
- `ExportStatus.tsx`: The archives being written, among the jobs of the status bar: what runs and how far, and what has just ended; pressing it opens the list of one's exports.
- `Group.tsx`: One labelled row of an export dialog: the name of what is chosen at the start of the line, the choice beside it.
- `hints.ts`: The hints of the export dialogs, by id: what ticking a place takes, the two forms, how much of a conversation, what is left out, how long an archive is kept, and the span of days.
- `jobs.ts`: The archives being written: which jobs this page follows, how far each has come, and the notice when one ends, with Download.
- `losses.ts`: What a document loses when it is written as Markdown, counted from its blocks before the file is made, in the kinds the server tells it by afterwards.
- `menu.tsx`: The Export… of a menu: offered to whoever may take the thing away, left out of a row's menu otherwise, and on the thing's own screen switched off with one line and the way to ask.
- `pending.ts`: What the export screens ask of a thread, a chat and an archive, now that the generated types name all three.
- `PlaceExport.tsx`: Export… on an org, a project, a folder or a topic: one archive of what its person may read there, chosen in a tree, planned as the choice changes, and written by a job.
- `PlaceOptions.tsx`: What an archive holds besides the rows that are ticked: the kinds, the form, how much of each conversation, the attachments, the earlier versions and the comments, and a span of days. Every choice is visible directly.
- `PlaceTree.tsx`: The tree of ticks in the dialog of a place: the place and what lies in it that its person may read, each row with a box, its name, what is in it, and how many things there are left out.
- `plan.ts`: The plan of an archive read for the line under the options: how much against what the place allows, by how much it is over, and what is left out and where.
- `PlanLine.tsx`: The line under the options: how much the archive would hold against what the place allows, how long it is kept, and what is left out; past a limit, by how much, with Choose less.
- `RequestExport.tsx`: Request access to take a thing away as a file: whom to ask, a line for the reason, and where the request stands.
- `rows.ts`: The rows of the tree in the dialog of a place: the place, what lies in it that is open, and the documents written beside each place that is open.
- `save.ts`: Saving a file the page holds: under the name the server gave it, through the browser's own download.
- `selection.ts`: Which rows of a place are ticked, read from the few ticks that differ from their place, and the request that those ticks come to.
- `target.ts`: What can be taken away: a thread, a chat or a document as one file, and an org, a project, a folder or a topic as one archive.
- `ThreadFile.tsx`: A thread as one file: how much of the conversation, Markdown to read or JSON for a program, a range of turns, and the three things only a person's export can add.
- `WorkspaceNote.tsx`: The line under the options of a place that says workspace files are not in an archive, and leads to where they are taken away.
- `YourExports.tsx`: Your exports: the archives made for the person, what each is of, when it was asked for, how large it is and until when it is kept, with Download and Remove at the end of the row.
