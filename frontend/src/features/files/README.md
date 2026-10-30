# Files and code

A file opens as what it is. The files of a workspace are shown, and where they are text edited, in one code
view (`../../ui/CodeView.tsx`), with a menu under the right mouse button. Which viewer takes a file is decided by
a list, and that list is the extension point: a further kind of file is one module in `viewers/` and one line in
`viewers.ts`. A viewer never runs what it shows. The tree itself is `../inspector/Files.tsx`.

Built: the code and text viewer (highlighting of the languages in `languages.ts`, each fetched when a file of it is
first opened; find and replace, go to a line, several cursors, folding; edited and saved on the version that was
read, with unsaved text kept on the device for seven days, the file that changed underneath shown with Compare, Keep
mine and Take theirs, and a clean tab that follows the file with the changed lines marked), Markdown, images (an SVG
drawn as an image), PDFs with page/zoom/text search, archives listed where they lie, sound and film, differences, text beyond the preview limit, and
the page for a file nothing shows. A path an item of the conversation names opens its file at its line.

PDF reads authenticated bytes, paints one page with an eight-million-pixel ceiling and releases its worker on close.
Its CMaps, standard fonts and binary decoders are lazy local assets.
Attachments in chats, composer drafts, timeline inputs, image tools, documents, references, search and the Inbox share
the same file chip, bounded lazy preview and overlay. The overlay adapts each authorized source to the existing viewer
registry. Static previews never request the whole original in the browser. Unsupported codecs, missing files and
oversized previews keep the same states and explicit download action. Small, Medium and Large choices live in the
reader's device workbench, while a document stores its own choice with the reference or media block.

Not built: the Follow switch, editing document-owned attachment bytes in place, and a separate "Open with" module
(the menu is in `FileMenu.tsx` and the tab's header, in `FileTabs.tsx`). Document attachments open read-only in the shared overlay.

Kept hidden views disable file description, fetching and byte queries as well as directory watches.
The tree's initial directory reads and open-file watches use the same visible-view lease; process and
Git queries follow that lease too. The hidden-reader component source covers a new path and cache
invalidation while hidden, then the actual file output when shown. Explicit file Reload invalidates
only its exact description key, which the live query client accepts.

- `viewers.ts`: The list of viewers, most particular first: what each takes, what it shows and whether it edits; a list to which another viewer is added.
- `languages.ts`: The languages the code viewer highlights, each loaded when a file of it is first opened; anything else is text.
- `content.ts`: Reading a file of the workspace: what it is, its exact version, and its bytes, in the fewest requests that can tell.
- `source.tsx`: Authorized file sources adapt attachments to the workspace's existing viewers without copying their rendering.
- `FileChip.tsx`: One file chip and thumbnail family, used in messages, tool results, documents and file lists.
- `FilePreview.tsx`: One lazy bounded preview and the same unavailable states in every file surface.
- `FileDetails.tsx`: A file card's preview and actions are shared by attachments, references and cards in document flow.
- `FileGuideHint.tsx`: One file help hint opens the matching heading in the reader's authored guide.
- `FileOverlay.tsx`: Attachments open the workspace's viewer for their kind in the same keyboard-accessible overlay.
- `MessageFiles.tsx`: Search and Inbox attachment projections use the same file chip without a second preview implementation.
- `mediaPolicy.ts`: Media viewers ask for the same bounded preview and effective byte policy before decoding an original.
- `door.ts`: The way a thread opens into the workspace of its environment, and what its reader may do through it.
- `lazy.tsx`: A part of the application fetched when it is first drawn, three times over if need be, and made anew when it is asked for again after it failed.
- `FileTabs.tsx`: A file opened by one click, as a file manager opens one: where it opens in each arrangement, its address with a line, and what its viewer needs loaded.
- `FileLink.tsx`: A path that an item of the conversation names, opening its file in the code view where the thread may look at files.
- `FileTree.tsx`: The tree of a workspace: lazy, drawn by the rows in view, and kept equal to the disk.
- `FileMenu.tsx`: The context menus of a file, a directory, several rows, the empty part of the tree, a tab and a selection; cut, copy and paste of rows; an entry one may not use disabled with the reason.
- `Uploads.tsx`: Workspace uploads use resumable sessions and ask once how a batch handles existing names.
- `UploadHistory.tsx`: Upload history names the actor, files, destination, measured bytes and outcome in one compact line.
- `SearchInFiles.tsx`: The search inside files: the lines that hold a word or a pattern, file by file, under the directory chosen.
- `Editing.tsx`: Editing a text file where it is shown: saved on the version it was read at, unsaved text kept on the device for its person alone, the file that changed underneath (Compare, Keep mine, Take theirs), and why a file cannot be edited.
- `AgentEdits.tsx`: A file that is open following the agent's writing: the changed lines marked until the next turn, and who changed it.
- `AddToMessage.ts`: A file, some lines, a difference or a selection of a shell's output put into the conversation's composer as a reference.
- `Unavailable.tsx`: What the place of a file shows when its content cannot be had: why, and what can still be done.
- `OpenWith.tsx`: Open with… and Download, in a file's menu: the viewers that take a file, and the one a person chose for that kind remembered for them.
- `Follow.tsx`: Follow: each file the agent reads or writes brought to the front at the lines it touched, until the person types in a file or picks another tab.
- `DocumentFile.tsx`: A file a document holds, opened in the same view and saved in place by whoever may write the document.
- `viewers/Code.tsx`: Code and text: highlighting, line numbers, folding, find and replace, go to a line, several cursors.
- `viewers/Markdown.tsx`: Markdown: the source beside the rendered text, or either alone; raw HTML never rendered.
- `viewers/Image.tsx`: Images, fitted, zoomed and moved, with their size; an SVG drawn as an image, with its source beside.
- `viewers/Archive.tsx`: An archive listed as a tree: what it holds, with sizes, and one entry opened or downloaded.
- `viewers/Media.tsx`: Sound and film that the browser plays.
- `viewers/Difference.tsx`: A patch and every comparison: two sides or one column, changed words marked.
- `viewers/TooLarge.tsx`: Text beyond the preview limit: its first part, read-only, with how much there is.
- `viewers/NoPreview.tsx`: Anything else: what it is, its size and its type, and Download.
- `viewers/Pdf.tsx`: PDF pages, with a jump to a page, zoom and a search of the text.
- `viewers/pdfAssets.ts`: PDF.js's CMaps, standard fonts and decoders are local lazy assets, fetched by filename when a PDF needs them. */
