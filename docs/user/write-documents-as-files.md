---
group: work
title: Write documents as files
screens: /doc/$documentId, /documents/$nodeId, /document-folder/$folderId
keywords: markdown, orch markdown, html, word, docx, text, paste, import, export, archive, zip, relative links, pictures, toggle, columns, callout, colour, underline, page break, agent, edit_document, download_shared, checkout, commit, 文档, 文件, 导入, 粘贴, 代理, 智能体
heading_keywords.orch-markdown: callout, alert, toggle, details, columns, span, underline, video, audio, page-break, mermaid, formula, tag
heading_keywords.paste: clipboard, rich text, page, picture, notice
heading_keywords.import-files: folder, zip, link, reference, picture, notion, lark, report, missing
heading_keywords.word-files: docx, heading, table, picture, style
heading_keywords.what-agents-do: file, format, seq, block, address, batch, import, checkout, commit, comments
heading_keywords.what-a-file-cannot-keep: loss, colour, alignment, spans, widths, blank
summary: Use Orch Markdown, paste and file imports to exchange documents with people and agents.
---

# Write documents as files

Orch reads Markdown, HTML, Word and plain text, and writes Markdown and HTML. One reader serves paste, import and agents, so the same text gives the same blocks everywhere. This page shows what you can write in a file, how to bring files in, and how agents work with documents as files.

## Orch Markdown

Orch Markdown is GitHub's Markdown with formulas (`$x^2$`, `$$` on its own lines), diagrams
(a fenced `mermaid` block), callouts as alerts, and a small set of tags:

````markdown
> [!WARNING]
> A callout: NOTE, TIP, WARNING or CAUTION.

<details>
<summary>A toggle</summary>

Blocks under it.

</details>

<columns>

<column ratio="2">

The wider column.

</column>

<column>

The narrower one.

</column>

</columns>

Words in <span color="red" background="amber">colour</span> and <u>underlined</u>.

<video src="https://example.org/clip.mp4">A video by address</video>

<page-break/>
````

Put each tag on a line of its own, between blank lines. Colours are red, orange, amber, lime, green, teal,
cyan, blue, indigo, purple, pink and grey. A column's `ratio` is its weight; leave it out for equal columns.
Any other tag stays as text, and so does a tag without its partner. Text from Notion, Lark and GitHub is read
as it is: Notion's indented children, its `gray` and `red_bg` colours and its `<toggle>` are understood.

A document written as Markdown and read again is the same blocks, but for their identities,
their authors and history, and their comments.

## Paste

Paste Markdown or a page of rich text into a document and it becomes the blocks it says, where the
caret is. Pictures inside a pasted page become files of the document. A short notice says what was kept as
words or left out. Code and table cells take the text as it is.

## Import files

Import accepts Markdown, HTML, Word and text files, or an archive of them. Folders become document folders.
A link from one file to another becomes a reference to the document it becomes; a picture or file it names becomes
a file of that document. A link or picture that leads nowhere stays as its words, and the result says which.
Pictures on the web stay links: Orch fetches nothing.

## Word files

A `.docx` file is turned into HTML and read like any page. Headings, lists, tables, bold, italic, underline,
links and pictures come across. Styles Orch does not know are read as plain text, and the result says so.

## What agents do

An agent can list an empty document folder that is shared with **Can view**. Adding or importing a document needs **Can edit**.

**To add a document**, the agent writes the document in a file, then imports it. The tool `edit_document` takes `file` and `format` (markdown, html, docx or text) instead of `markdown`. Pictures that lie next to the file come with it.

**To change a document**, the agent copies it with `download_shared`. The file begins with the document and its `seq`. After editing the file, the agent commits it with `rewrite` and `file`.

- Only what changed changes. A block written as it was stays untouched, and a block whose words changed stays the same block, so the comments on it and the links to it hold.
- `replace` does the same for a passage. `block_replace`, `block_insert_after`, `block_delete` and `block_move_after` change one block by its address (`#3`, or `#3.2` for the second block under the third).
- `batch` makes several of these edits at once.
- `import` takes a directory or a zip of files into a document folder. The same call reports the planning or the progress while the job continues. Repeat its `path` and `file` with `cancel: true` to stop it, which keeps the files already imported. Use `resume: true` to continue under the current permissions.
- Two imports may plan or convert at once. If a limit refuses a third, retry after one has finished, or cancel a job that exists.

## What a file cannot keep

Orch Markdown preserves folding headings, spans and colours of cells, colours and alignment of blocks, widths of columns, blank paragraphs and nested blocks. Where ordinary Markdown has no form for them, the export adds an `<!-- orch:block ... -->` comment around the readable text.

- Keep these comments when you edit an exported file. They carry layout, never the identity or the author of a block. Other Markdown viewers hide the comments and show the text.
- A caption of an image that has a style is written as Markdown in its comment. Ordinary content needs no comments.
- An inline formula that is unusual uses a span around readable code, to keep dollar signs, line breaks or table pipes. Keep its `orch-math` attribute with the formula. If you change the visible code, the new words are kept.

An import makes new block identities. Authors, history and discussions are outside the file. Checkout and commit
into the original document match its blocks and keep identities, comments and links where the blocks match.
