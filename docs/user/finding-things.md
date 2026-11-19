---
title: Finding things
screens: /search, /help, /help/$article, /everything, /trace/$nodeId, /u/$username, /dev/ui
keywords: finder, search, find, command palette, shortcuts, hints, help, keys, semantic search, similar meaning, regex, filters, 语义搜索
---

# Finding things

Orch has one field that goes to anything with a name, runs any command and answers a question, a page that
searches what was said, a search inside one conversation, and a help that answers where the question arises.

## The finder

Press `Ctrl+K` (`Command+K` on a Mac) from anywhere, or press the magnifier in the rail. Type a few letters.
Names of everything you may open are listed as you type: orgs, projects, folders, topics, threads, documents
and chats, then people, and then the resources you can see (providers, agents, environments, hosts, templates,
MCP servers, mounts and machines). Under each name stands where it is: the way down from its org
("Web › Checkout"), or its address. Things you starred and places you went to lately come first.

A sign at the start of the field keeps it to one kind of answer:

| Sign | Keeps it to |
|------|-------------|
| `>` | Commands: what can be done here, such as a new topic, or switching the theme |
| `@` | People, by name, username or address |
| `:` | Settings, by what they do, and the rows of permission pages, by what they allow |
| `?` | Help: articles, by title and heading |
| `/` | What was said: the words after it are searched in conversations, chats and documents |

The buttons under the field write the same signs. The arrow keys move along the rows; `Enter` opens the one
chosen. `Ctrl+Enter` opens it in a new tab of the browser. A command you may not use is listed, switched off,
with the reason. Nothing is listed that you may not see.

With nothing typed the finder lists what you starred and what you went to last, and offers what you can do on the
screen and its help. The last row, **Advanced search**, opens the search page with the words you typed.

## Search

Press `Ctrl+/` (`Command+/` on a Mac), or use `/` in the finder.
Search reads the conversations, chats and documents you may read.
One character is enough on the Search page, including Chinese.
Separate words may appear in any order. Put a phrase in double quotes to keep it together.

**Best matches** puts names and titles first, then descriptions, headings and file names, then body text.
Within a field, more occurrences come first, then the newest. **Newest** orders word matches by time.
Open a result to read its item, message or document block. A person's chip opens their card.
A result's chip opens the thing's card.
The line beside its time says where the match was found, such as **In the name** or **In chat messages**.

### Words and similar meaning

Word results start as you type, after a short pause.
If an org uses **Semantic search**, results with a similar meaning arrive through the same search.
Whichever source answers first fills the list. The server sends the combined order as the other answers.
A hit without highlighted words is marked **Similar meaning**.

**Searching…** and the count remain while a source is running. Results stay usable.
**Similar meaning unavailable** means that source could not answer. Hover, focus or tap the status for its reason.
Word results remain. **Index paused** means a stored query vector can still answer while indexing waits for its provider.
A fresh query cannot call that provider while it is off. A quoted phrase, **Regex**, or **Newest** uses words alone and says why.

Meaning waits for a typing pause or `Enter`. Changing the query cancels the previous search.
A repeated query can reuse its org's query vector. It does not send an embedding request for each key.

### Keep your place

The list can settle into the latest server order while you have not started reading it.
Hovering a result, focusing it or opening its card holds a new order until you leave it.
After you scroll past the first screen, use **Show more**, or the settling time ends, the rows stay put.
Later results are added below them. Meaning-only additions sit under **Similar meaning**.

### Show more

The first **Show more** reveals all results already received from both sources.
It sends no new search. Later presses continue the same search and append its next results.
If the model changes, words continue; start a new search to use the new semantic index.
The search and its filters stay in the address, so it can be bookmarked or sent to someone else.
Their results still depend on what they may read.

### Filters and words only

Use the tabs to search conversations, chats, documents or results that carry files. On a wide screen the filters
are at the left. On a smaller screen open **Filters** for the same controls. Active filters appear above the results;
remove one there or choose **Clear**.

Choose a place, an owner, a creator, a last editor or the person who wrote the matched part. Choose **Created** or
**Edited** for the date range. Documents and conversations use their own times; chats use the matched message's times.
An older link with a date and no date choice uses **Written**, the matched part's time.
You can keep to a file type, titles, mentions of you, shared items or things you take part in. Conversation kinds
and **Written by** remain available for a narrower search. A file-focused document result shows a current matching file.
Some conversation files have no recorded type; choose **Any** to include them. Open a result to reach a
workspace-only file in its conversation.

Search remembers your tab, sort, filters and open filter groups on this device. It does not keep the words you typed.
An address with its own sort or filters takes precedence.
A saved place, chat or document that you can no longer search falls back to the default view.
The same selection applies to words and meaning. A filter never grants access.
Direct and group chats appear only for their participants. Inspection alone does not add them to search.

**Regex** uses regular expressions instead of literal words. An invalid expression stays beside the field.
A quoted phrase, a regular expression, **Title only** and **Newest** use words alone.
[Semantic search](semantic-search.md) explains its setup, index states and model choices.

## Names and recorded activity

Everything lists the places, people, documents and resources you can open. Its filters narrow the list;
choosing a row opens that thing. A person's public profile shows their own words, pinned work and the
activity you can read. A pin does not grant access to its destination.

A place's audit trail records who changed what and when. Filter by person, action, kind or dates, or
group the entries by day or kind. The right to read the trail is checked for that place; opening a
person's profile does not grant it. Retained content opens in a read-only inspection when your rights
allow it, without restoring the content first.

## Find in a conversation

In a thread, its menu holds Find in the conversation. A bar appears under the header with a field and two
arrows. The first hit is brought into view and lit; `Enter` goes to the next older hit and `Shift+Enter` to a
newer one. The words are marked in the rows that are drawn.

## Help and hints

A "?" beside a term or a choice opens a card in place: what it is, which one to choose by what you want, where
it is set, and a link to the article that treats it. `F1` opens the help for the screen in front of you, beside
it: what the screen is for, the hints on it, the articles that belong to it, and the keys that are bound at the
moment. A tip that is shown unasked, such as the one the first time you open the finder, can be closed for good;
it stays closed on every device you use.

The UI gallery at `/dev/ui` is available only in a development build. It shows
shared controls and their states; it creates no work or permissions. A production
build does not offer that gallery.
