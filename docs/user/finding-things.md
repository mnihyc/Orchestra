---
group: settings
title: Finding things
screens: /search, /help, /help/$article, /everything, /trace/$nodeId, /u/$username, /dev/ui
keywords: finder, search, find, command palette, shortcuts, hints, help, keys, semantic search, similar meaning, regex, filters, 语义搜索
summary: Find a place or a resource by its name, search what was said in threads, chats and documents, or open help, from one place.
---

# Finding things

Orch has one field that goes to anything with a name, runs any command and answers a question. Around it are a page that searches what was said, a search inside one thread, and help that answers where the question arises.

## The finder

Press **Ctrl+K** (**Command+K** on a Mac) from anywhere, or press the magnifier in the rail, and type a few letters.

The names of everything you may open are listed as you type: organizations, projects, folders, topics, threads, documents and chats, then people, then the resources you can see (providers, agents, environments, hosts, templates, MCP servers, mounts and machines). Under each name stands where it is: the way down from its organization ("Web › Checkout"), or its address. Things you starred and places you went to lately come first.

A sign at the start of the field keeps the answers to one kind:

| Sign | Keeps it to |
|---|---|
| `>` | Commands: what can be done here, such as a new topic, or switching the theme |
| `@` | People, by name, username or address |
| `:` | Settings, by what they do, and the rows of permission pages, by what they allow |
| `?` | Help: articles, by title and heading |
| `/` | What was said: the words after it are searched in threads, chats and documents |

The buttons under the field write the same signs. The arrow keys move along the rows, and **Enter** opens the row chosen. **Ctrl+Enter** opens it in a new tab of the browser. A command you may not use is listed, switched off, with the reason. Nothing is listed that you may not see.

With nothing typed, the finder lists what you starred and what you went to last, and offers what you can do on the screen and its help. The last row, **Advanced search**, opens the search page with the words you typed.

## Search

Press **Ctrl+/** (**Command+/** on a Mac), or use `/` in the finder. Search reads the threads, chats and documents you may read. One character is enough on the Search page, Chinese included. Separate words may appear in any order. Put a phrase in double quotes to keep it together.

**Where it looks.** Search starts **In the org** that is shown in the left bar, with the departments and projects in it that you can read. The same root applies to the names and the content found from the rail.

- Open the chip of the organization and choose **All my orgs** to include your personal organization and every other organization you can read. **Place** narrows inside that choice, and changing the organization clears a narrower place.
- The address keeps the choice of organization for sharing and for a reload. A new search uses the organization in view again, and choosing **All my orgs** does not make it your next default.
- Results from all organizations carry chips of the organization, and get headings of organizations only when more than one organization answers.
- An empty search in one organization offers **Search all my orgs**. Its count is the results you can read received so far, and **+** means that more may remain.

**How it orders.** **Best matches** puts names and titles first, then descriptions, headings and file names, then body text. Within a field, more occurrences come first, then the newest. **Newest** orders word matches by time.

**Results.** Open a result to read its item, its message or its block of a document. The chip of a person opens their card, and the chip of a result opens the card of the thing. The line beside its time says where the match was found, such as **In the name** or **In chat messages**.

### Words and similar meaning

Results for words start as you type, after a short pause. If an organization uses **Semantic search**, results with a similar meaning arrive through the same search. Whichever source answers first fills the list, and the server sends the combined order as the other answers arrive. A hit without highlighted words is marked **Similar meaning**.

- **Searching…** and the count stay while a source is running. The results stay usable.
- **Similar meaning unavailable** means that source could not answer. Hover, focus or tap the status for its reason. The results for words remain.
- **Index paused** means that a query vector that was stored can still answer while indexing waits for its provider. A new query cannot call that provider while it is off.
- A quoted phrase, **Regex** or **Newest** uses words alone and says why.

Meaning waits for a pause in typing or for **Enter**. Changing the query cancels the previous search. A query that is repeated can reuse the query vector of its organization. It does not send a request for embeddings for each key.

### Keep your place

The list can settle into the latest order from the server while you have not started to read it. Hovering a result, focusing it or opening its card holds a new order until you leave it. After you scroll past the first screen, use **Show more**, or the time to settle has ended, the rows stay where they are, and later results are added below them. Results that come only from meaning sit under **Similar meaning**.

### Show more

The first **Show more** reveals all the results already received from both sources. It sends no new search. Later presses continue the same search and add its next results. If the model changes, words continue. Start a new search to use the new semantic index. The search and its filters stay in the address, so it can be bookmarked or sent to someone else. What they see still depends on what they may read.

### Filters and words only

Use the tabs to search threads, chats, documents or results that carry files. On a wide screen the filters are at the left. On a smaller screen open **Filters** for the same controls. The active filters appear above the results. Remove one there, or choose **Clear**.

- Choose a place, an owner, a creator, a last editor, or the person who wrote the part that matched.
- Choose **Created** or **Edited** for the range of dates. Documents and threads use their own times, and chats use the times of the message that matched. An older link with a date and no choice of date uses **Written**, the time of the part that matched.
- Keep to a type of file, to titles, to mentions of you, to shared items or to things you take part in. The kinds of threads and **Written by** stay available for a narrower search.
- A result of a document that is about a file shows a current file that matches. Some files of threads have no type on record. Choose **Any** to include them. Open a result to reach a file that exists only in the workspace of its thread.

Search remembers your tab, your sort, your filters and your open groups of filters on this device. It does not keep the words you typed. An address with its own sort or filters takes precedence. A place, chat or document you saved and can no longer search falls back to the default view. The same selection applies to words and to meaning. A filter never gives access. Direct and group chats appear only for the people in them. Inspecting them does not add them to search.

**Regex** uses regular expressions instead of literal words. An expression that is not valid stays beside the field. A quoted phrase, a regular expression, **Title only** and **Newest** use words alone. [Semantic search](semantic-search.md) explains its setup, the states of the index and the choices of models.

## Names and recorded activity

**Everything** lists the places, people, documents and resources you can open. Its filters narrow the list, and choosing a row opens that thing. The public profile of a person shows their own words, their pinned work and the activity you can read. A pin gives no access to its destination.

The audit trail of a place records who changed what and when. Filter by person, action, kind or dates, or group the entries by day or kind. The permission to read the trail is checked for that place. Opening the profile of a person does not give it. Content that was retained opens in a read-only inspection when your permissions allow it, without restoring the content first.

## Find in a conversation

In a thread, its menu holds **Find in the conversation**. A bar appears under the header with a field and two arrows. The first hit is brought into view and lit. **Enter** goes to the next older hit and **Shift+Enter** to a newer one. The words are marked in the rows that are drawn.

## Help and hints

A question mark beside a term or a choice opens a card where it stands: what it is, which one to choose by what you want, where it is set, and a link to the article that treats it. **F1** opens the help for the screen in front of you, beside it: what the screen is for, the hints on it, the articles that belong to it, and the keys that are bound now. A hint that is shown without being asked, such as the one the first time you open the finder, can be closed for good. It stays closed on every device you use.

The gallery of the interface at `/dev/ui` is available only in a development build. It shows the shared controls and their states, and it creates no work and no permissions. A production build does not offer that gallery.
