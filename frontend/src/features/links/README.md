# The Links section of a document

One section of a document's right bar with four tabs, and a full graph in the main area. It replaces the separate
Related section: Related is a tab here.

- `Links.tsx`: the section. Reads which tabs the document shows and the first page of what refers to it and of what it
  refers to; draws the tabs with their counts (a plus where more pages follow) and the chosen tab's body.
- `ReferencedBy.tsx`: one row for each source, the line around each mention with the reference in bold, and the
  mentions that follow those shown.
- `References.tsx`: what the document points to; what the reader may not open stays a locked chip with its written label.
- `RelatedTab.tsx`: the related list, a page at a time.
- `GraphTab.tsx`, `GraphPage.tsx`, `GraphView.tsx`, `GraphCanvas.tsx`, `graphLayout.ts`: the small graph, the full view
  (at the document's address with `?graph=1`), the picture with its list of nodes for the keyboard, the canvas, and the
  force layout that is run to rest before anything is drawn.
- `LinksDialog.tsx`: the tabs one document shows, set by whoever manages it.
- `anchors.ts`, `listState.ts`: where a mention opens; what a list draws from the pages read.
