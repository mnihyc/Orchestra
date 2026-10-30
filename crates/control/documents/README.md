# orch-documents

Documents: pages of blocks written in place, with their operations, comments, history, files and exports.

Implemented. Everything that needs neither a database nor a socket is here as plain functions over
values: what a block may be and hold, a batch of operations applied to a tree kept in memory (a block's
place among its siblings is a decimal number of any length, so that two people who write at once never
need the same one), when a version is cut and which are kept, what differs between two states, the notices
a document sends, and the one reader and the three renderers that move a document across the edges as
Markdown, HTML and Typst. The unit tests state the rules: batches that conflict and batches that do not, every
limit, a version cut on a clock that is moved by hand, and for every block type that Markdown can carry that
what is written out reads back as the same blocks, with the losses named for what it cannot. The Markdown links a file of the document by the directory its caller names (`files_dir`: `<title>.files` where the text lies as `<title>.md`, `../<title>.files` from an earlier version, `files` where none is said), and reads a link in any of those forms back as the file.

A document is written beside an org at any level, a project, a topic or a
thread. What is stored is a tree of blocks, never a text to be parsed: a
change is a batch of operations on blocks, several people write at once, and
there is no Save. Markdown is a habit of typing and a format at the edges.
An agent reads, makes and writes documents only where somebody shared a place
or a document with that agent for it, through the same operations. History records
the people and threads that wrote in each version window, including a stretch
that changes only files. Added, replaced and removed files are named by their
stored names, without a machine's local paths. Attaching a workspace file uses
the shared bounded reader before the writing transaction, with the document's
current edit access and per-file and total limits checked again before commit.
Replacing a file keeps its identity and links; detaching it also removes blocks
that show it and releases its storage reference. The server writes a
document out as Markdown, HTML and Typst; a PDF is typeset from the Typst in
the reader's browser, so no service compiles a document.

Naming someone in a document or comment does not send a notice. The mention
helpers identify names in content; an explicit person-notification action is
responsible for telling a current reader.

| Module | Planned responsibility |
|--------|------------------------|
| [blocks](src/blocks.rs) | The tree of blocks: its twenty types, inline content, nesting, and the limits a document is held to. |
| [position](src/position.rs) | Where a block stands among its siblings: decimal numbers of any length, and the shortest number that fits between two. |
| [operations](src/operations.rs) | Batches of changes to blocks (insert, update, move, remove), each applied whole; a change written on an older state of a block refused, with what the block is now. |
| [versions](src/versions.rs) | Earlier versions: cut by the clock while a document is written and after it was left alone, before a rewrite and before one is put back; what lies between two is one entry with its authors; listed, read, put back, and kept within a number and a size. |
| [changes](src/changes.rs) | What changed between two versions, block by block, with who wrote it, people and agents alike; and who last changed each block. |
| [comments](src/comments.rs) | Discussions on a block, or on words of it: opened, answered, reacted to and resolved beside the text; no part of the document or of a version, and in an export or in what an agent reads only where that was asked. |
| [files](src/files.rs) | The files a document holds beside its text, of any kind, and their limits. |
| [markdown](src/markdown.rs) | Markdown at the edges: one reader for what is pasted, imported or written by an agent, and one renderer for an export and for what an agent reads, whole, as its headings alone, or with the discussions beside it; it says as much of a document as Markdown can and names what it could not. |
| [tags](src/tags.rs) | The closed set of tags that Orch's Markdown is written with beside GitHub's, found line by line outside code before the text is read as Markdown. |
| [convert](src/convert.rs) | One way into blocks for every kind of text a document can be made from: Orch Markdown, HTML, a Word file and plain text, each read once, with the relative paths it names resolved by whoever holds what it came with. |
| [merge](src/merge.rs) | Changing a document to what its Markdown now says while keeping what each block already is: its identifier, its comments' anchors, its links, and whatever Markdown has no form for. |
| [checkout](src/checkout.rs) | A document checked out as a file and committed back: the front matter that says which document the file came from and at which sequence. |
| [warnings](src/warnings.rs) | What a reader says of what it kept as words or left out: a stable code, its parameters, and the English sentence for whoever does not know the code. |
| [html](src/html.rs) | A document written out as one HTML file that carries its images and needs nothing else to be read. |
| [import](src/import.rs) | The shared inert HTML converter and fresh block identities for ordinary document creation. |
| [typst](src/typst.rs) | A document written out as Typst source with its images, from which a browser typesets the PDF: formulas converted where they can be, diagrams marked for the drawing the page puts there. |
| [presence](src/presence.rs) | Who is in a document, and who is writing in which block. |
| [describe](src/describe.rs) | What a document is said to be in one line: the first line of its description, or while it has none, of its first paragraph. |
| [references](src/references.rs) | References an agent writes and reads: one grammar for a shared-tree path or an identifier with a part, where one stands in Markdown, and the forms a reference is written in. |
| [links](src/links.rs) | The line around a mention of a thing, as one reader reads it: the thing under its name now, other chips named only where the reader may know them. |
| [mentions](src/mentions.rs) | People and things named in a document; the one notice a first mention sends. |
| [placement](src/placement.rs) | The node a document is beside, an org at any level among them; moving it to another, deleting and restoring it. |
