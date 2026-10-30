# Documents

Document lists separate readable references from the minimal names of private documents the reader administers.
Those names open only their permission pages; editors, the navigation tree and export choices consume readable
references alone. Readable activity paging and the independently name-ordered private continuation share one
page limit. Deleted-document restore lists retain their existing management requirement.

Built: a document is a page of blocks written in place beside an org at any level, a project, a
topic or a thread: by several people at once, with no Save, no draft and no preview panel. All
twenty block types are read and written (text in six levels of heading, quotes and callouts of four
tones, bulleted, numbered, to-do and toggle lists, code with a language, formulas and diagrams,
tables, columns, dividers and page breaks, films and sounds by address). What is typed is sent as
batches of operations on blocks, one in flight at a time and shown as "Saving…" until answered; what
others write arrives by the document's feed and is laid over the page without moving the caret or
touching a block the reader has an unsent change in; the change that came second is kept beside the
first with Replace, Keep both and Discard mine. Comments on a block and on words of it, History with
what changed between two versions and putting one back, "changed since you were here", rename,
Move, Private and Open again, Delete and its undoing, Export… (the one dialog of `../export/`, which
gives a document as Markdown, HTML or Typst from the server and typesets a PDF locally), Markdown pasted into the page or imported as a file, and
the list of documents beside a place are all here. What is stored and sent is Orch's own blocks;
Markdown is a habit of typing and a way in and out.

Images, films, sounds and other files are uploaded by paste, drop, the picker and Files, with progress,
Retry and Remove. The Files dialog opens, downloads and deletes files; image width and alignment are editable.
PDF uses the server's authorized Typst/image bundle, a local worker and local body/math/code/Chinese fonts.
Diagrams become drawings, and a diagram that cannot be drawn stays labelled as its source. Print is the fallback.
What is not here, and why: adding people to a document is the permission page, which the system does not offer yet,
and Share says so; a thread's Documents view is `../inspector/Documents.tsx`, and the Documents view
of the right bar and the documents of an org's home and of a project are to be mounted by those
screens from `DocumentList.tsx`, which takes the node it is of.
On a topic, Documents lives in the page's tab only. The side view is offered on org, project and
thread screens; the same screen eligibility hides older saved topic-side entries without erasing
the reader's saved layout.

The editor is Tiptap 3 on ProseMirror with a schema of Orch's own. The editor's state is turned into
blocks and back at the edge of two modules (`schema.ts`, `blocks.ts`), so that what is sent, what
differs and what is kept are plain data tested without a browser.

- `Document.tsx`: A document's page: its title, its description, the line under it with who changed it last and its access in words, who is here, its blocks, Comments, History, and its menu (Private, Move, Share with an agent, Export, Print, Delete).
- `Editor.tsx`: The block editor: Orch's own schema of twenty block types on a rich-text base, changes sent as operations on blocks, and no Save.
- `blocks.ts`: Orch's blocks as plain data: their order, the tree they make, their canonical form, and the batch of operations that turns the blocks the server holds into the blocks a person has made.
- `schema.ts`: The twenty block types and their inline content, and the mapping between the editor's state and Orch's blocks.
- `sync.ts`: What is typed is sent as batches of operations, one at a time and in order, shown as "Saving…" until answered; what others write is laid over the page; and the change that came second is kept beside the first.
- `remote.ts`: Other people's changes laid over the page: rows the feed delivered become changes of the editor's state, block by block, without touching a block its person has an unsent change in, and without a trace in the person's own undo.
- `commands.ts`: What the keys and the menus do to a page of blocks: end a block and start the next, join one to the one before, put a block under the one above and take it out again, turn it into another type, remove it.
- `BlockMenu.tsx`: The block menu behind a slash, and the habits of Markdown that turn into blocks while typing.
- `Handle.tsx`: The handle beside a block: move it with what is under it, turn it into another type, remove it.
- `TextBar.tsx`: The small bar over a selection of text: bold, italic, underline, strike-through, code, link, colour, highlight, Comment and Turn into.
- `tints.tsx`: The twelve colours of the palette as a menu: the swatch, the name, and none. */
- `lines.ts`: Views of the lines that carry something beside their text: the box of a to-do, the arrow of a block that folds, the number a list begins at, the language of a code block.
- `pieces.ts`: The blocks a text says, as the page lays them: the server's one reader makes them, and nothing in the page reads Markdown.
- `paste.ts`: Paste: text that says Markdown, and a page of rich text, become the blocks the server's reader says they are, where the caret is.
- `Together.tsx`: Several people in one document, and an agent that was let write there: who is here, who writes in which block, and the change that came second kept beside the first.
- `Tables.tsx`: Tables: growing by their edges, a header row and column, cells that span several, colours and column widths.
- `Columns.tsx`: Columns side by side with shares that are dragged; under one another on a narrow screen.
- `Drawn.tsx`: Formulas and diagrams: the source in a small editor under the drawing, which follows as it is typed; loaded only on a page that has one.
- `codeColours.ts`: Colours of the code in a document's code blocks, by the parser of each block's language as the code viewer has it: fetched the first time a block of that language is drawn, and not before; a language without one stays text.
- `LinkCard.tsx`: The card of a link in a document, shown while the caret is in it: where it leads, and Open in new tab, Copy link and Edit; a Ctrl+Click (⌘-click) on a link opens it at once.
- `Readme.tsx`: The README of a place at the top of its view: the document it named, up to that document's first page break or its first twelve blocks where it has none, with Read more, which opens the document.
- `NameNewDocument.tsx`: The existing New document control creates immediately; the page selects its title once, then Enter moves into the editor. No preliminary name surface or Create step.
- `Media.tsx`: Images, films, sounds and files as blocks: put into the document, or for a film and a sound given as an address, which is played only once the reader chose to.
- `Comments.tsx`: Discussions beside the text: a card at the height of its block with the words it is about, answers, reactions and Resolve; the list of them all; and what becomes of one whose text changed or went.
- `views.tsx`: The Comments view of a document, in the right bar beside Chat: the discussions of the document in view, and what opens it.
- `marks.ts`: What the page marks beside its text: the blocks that changed since the reader looked, the block a link led to, and the words a discussion is about. */
- `History.tsx`: The History drawer: each stretch of writing with who wrote and how much, people and agents alike, what it changed, the document as it stood then, and putting a version back.
- `diff.ts`: The words that differ between two texts, for a difference that marks the changed words inside a block. */
- `SinceYouLooked.tsx`: Changed since you were here: who changed which blocks since the reader last had the document open, marked in the page.
- `Placement.tsx`: Where a document lies and who may read it: moving it, deleting it, and making it private and open again.
- `pdf.ts`: A PDF typeset in the browser: the Typst compiler and the fonts a document needs, fetched from Orch itself when somebody first exports, and the diagrams the page drew put into the source the server wrote.
- `typstBundle.ts`: Reads Orch's Typst export zip into bounded virtual files: source and authorized images, with no extraction to a machine's filesystem. */
- `Files.tsx`: The files a document holds, against its limits, each opening in the code view.
- `DocumentList.tsx`: The documents beside a place: the list of an org at any level, a project and a topic, with open discussions, what changed since the reader was there, the mark of one an agent made, New document and Import Markdown.
- `PlaceDocuments.tsx`: The Documents view beside a place: the list of the documents written beside the org, project or topic the screen shows, and for a thread the list of its topic.
- `register.tsx`: Puts the real Documents view into the workbench, in place of the note that said it was not built.
- `useMakeDocument.ts`: Creates beside a node and opens the new page with its title selected, from the list and navigation menus alike. Enter moves into the editor; an old page never steals focus.
- `editor.css`: The look of a page of blocks: the column it is read in, each block type, the twelve tints in both themes, and the marks of other people's work.

Stored inspection reuses the fixed block editor with collaboration, live suggestions and mutation keys absent. Attached bytes use the flagged stored reader; the ordinary document page and its seen writes are not mounted.
