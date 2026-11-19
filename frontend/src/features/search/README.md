# Authorized global search

Built: the page of `/search` (its state is in the address), with a query field, result-category tabs,
a grouped filter column on wide screens and the same controls in a sheet on narrower screens,
results as cards that are real links and open the thread at the item, the document at the block or the chat at
the message. Show more reveals received results and then continues the search. A search that is too short, one
that finds nothing and one the server does not answer each say so. The same search kept to one place serves finding
inside a thread, one chat or one document, with match stepping and return focus. Results also open as their own
tabs beside the current conversation through the same authorized readers. Covered by HTTP proofs of exact scopes
and private/deleted content, and authored component and browser cases of stepping, paging, highlights and draft/focus preservation.

Active filters show their current values and can be removed above the results. The device remembers
the filter groups, category, sort and filters without the query text; an explicit address wins.
The full-width result's primary body remains a
real link. Attribution lives outside that link so agent controls are not nested in a link, and a
small named icon in its header retains Open to the side without a label under every card. Normal,
modified and beside activation keep the same authorized targets and anchors.

Order is also address state (`sort=relevance`, with newest-first as the default). Best matches lists
readable place names and document titles before bodies, sharing the literal word/phrase grammar,
name tiers, personal visits and activity. Name cards use the server's address and have no invented
message author or open-beside target. The server applies Everything's inherited decisions inside SQL
before its page limit; exact chat/document selections and two-character exceptions retain their
ordinary rules. Real-route projection/relevance/clock/scale and browser reload sources are authored;
the integration rounds run them.

- `Search.tsx`: Search results and permalinks.
- `Filters.tsx`: Grouped place, person, date, content and participation filters.
- `ActiveFilters.tsx`: Removable current values and the order control.
- `filterValues.ts`: Current readable labels for selected people and places.
- `NamePicker.tsx`: A choice of one place or one person by typing a few letters of the name: what the filters of the search use for "where" and "by whom".
- `Hit.tsx`: One result of a search as a card: where it lies, the words around the match with the match marked, who wrote it and when.
- `Excerpt.tsx`: The words around a match, with the match marked: what a result of a search shows of what was said. */
- `Find.tsx`: Finding words inside one conversation, chat or document: the bar with its field, its count and its two arrows, and the hits behind it.
- `ViewFind.tsx`: A view's own Find bar keeps its search, highlights and return focus beside the content it searches.
- `ResultView.tsx`: A search result opens in its own workbench tab beside the current conversation and returns to its opener.
- `ResultContent.tsx`: Result tabs read a document or a thread through its existing live reader without replacing the route's conversation.
- `returnFocus.ts`: Find and result tabs return the keyboard to the same connected control and its saved caret or text selection.
- `labels.ts`: Finder names and search hits use the common place label with their own readable paths as peers.
- `highlight.ts`: Lighting words where they stand: the words a find looks for, marked in the text of whatever is drawn, without touching what draws it.
- `hits.ts`: What a hit of a search is to a reader: where it lies, who wrote it and when, the words around it with the match marked, and where it opens.
- `params.ts`: What the address of the search says: the words, and what to keep to; read safely from whatever the address holds, and written back as short as it can be.
- `dates.ts`: Dates as the search takes them: a day chosen in a field, turned into the moment it begins or ends in the reader's own time zone.

- `useSearchStream.ts`: Presentation of the server’s streamed order; held while a result is read and fixed once the first screen settles.

Search streams both sources over one request and presents only the server's reciprocal-rank order. Results settle
once the first screen is read; active cards and focus hold pending reorderings. First Show more reveals received
rows and later presses resume the server cursor. Component tests cover independent arrivals, source failures,
filters, typing, cancellation, settling and continuation. Result names use the shared reference card.
