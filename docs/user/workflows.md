---
title: Work and collaboration
screens: /chat/$chatId, /place-chat/$nodeId, /org/$slug/messages, /workbench, /thread/$threadId, /topic/$topicId, /project/$projectId, /org/$slug, /doc/$documentId, /inspect/$kind/$id, /shared, /approvals, /document-folder/$folderId, /documents/$nodeId
keywords: chat, documents, share, approvals, steer, stop, fork, forward, layout, code, conflict, private
---

# Work and collaboration

A topic keeps related threads, human chat and documents together. Each thread
is one agent conversation with its own title, inputs, model choices and history.
People can open the same thread and continue the same work according to the
rights shown there.

## Talk to people and agents

The human chat and the agent composer have different recipients. Discuss a
change in the human chat; send to the thread's agent when you mean to give it
work. Mentioning a person is a notice to them. Forward lets you deliberately
send selected messages and files to people, chats or another agent, with a
plan showing what will be delivered. It does not quietly share the whole place.

When the agent is running, a sent message can steer its current turn if the
harness offers steering. The message keeps its authored position and is shown
once. Model, effort and other settings apply from the next turn; steering text
does not change them in the middle of a turn. To apply a setting sooner, Stop,
change the setting and continue with a new input.

Stop targets the turn you see. It holds waiting inputs so the next queued one
does not immediately restart the work. Continue releases the held queue. The
thread shows a cancellation that could not end a native process truthfully;
inspect running work or the terminal before assuming its effects ended.

## Read the conversation

Agent messages, thoughts, plans and tools appear in the conversation. A tool
step opens to its command or file change, result and original harness call.
Large output keeps a useful inline part and a link to the whole. Unknown tools
retain their original data. A subagent step names its role, request and state;
the native stream evidence is separate from a constructed demonstration.

Use Find in the conversation, or select across turns to copy, quote or forward.
The view can show conversation, work or more detailed information. A preserved
snapshot or inspection of removed content is read-only. The live connection
status explains when new actions must wait; after reconnection the page catches
up from its known sequence rather than asking you to recreate the thread.

## Decide a request

A pending request shows what would happen and the options the harness supplied.
A command or file change may offer Allow once, a broader allowance, Deny or
Abort. A person question asks for answers by question; complete the required
questions before Send answers becomes available. An MCP form shows its fields
and supports accepting, declining or cancelling according to that request.

Only someone with the required current authority can decide it. A viewer reads
the card and sees why they cannot answer. A disconnected page waits for its
live connection. If a colleague answers first, the card shows that result;
your late answer does not overwrite it. An agent's port or capability request
has its own plan and effects, which must be read before confirming.

## Share a thing or a place

Share shows what the person or agent will receive. A thread or document is one
thing. A topic, folder, project or organization offers the place itself or the
place with its contents; those reaches are different, and its defaults apply
to content made later. A role in a place reaches its descendants, including
folders. Giving one document does not give its parent.

Holders can share what they hold where the place permits it. Nobody can give
more than they hold. Each entry records its giver and any end time; the giver
can take back their own grant. Managing the permission page, inheritance,
Private, locks and defaults requires the separate management right. Private
previews who would lose access before it is applied.

The same holder field chooses a person, a custom group, a role at a place,
everyone who sees a place, or members who belong to an organization by their
domain. These last two audiences differ. Type a locator such as
`dep2.sub3 developer`, `every dep2.sub3`, `members of dep2`, `dep2 reviewers`
or `@alice`. A unique match becomes its human name and place on Enter; if
several places match, choose the one whose full address you mean. An unseen
place is not offered. Groups can contain other groups and roles; their current
members receive what the group is given, and a cycle or more than five nested
groups is refused. Renaming a place preserves its existing entries.

Use View as and the source of an entry to understand access. Administrative
override is confirmed and leaves its page and audit mark; it does not overcome
a higher lock, a provider's owner-only policy or a computer's local policy.

## Give agents the team's knowledge

Share a document, chat, thread or place with the agents that should use it.
Can view lets them read the shared content; Can edit permits the relevant
writing, with document management separately controlled. Private descendants
are left out. Agents list the shared tree, read at the requested depth, search
and download; giving a document does not give an agent a human's general access.

An agent's edits and messages name that agent. Its `send_message` tool accepts
a chat or thread target and optional files. Chat messages are visible to people;
a thread target delivers a note under that topic's delivery and wake policy.
The answer says whether the message was posted, queued or delivered.
Announcements and milestones are recorded in its conversation and sent to
the selected audience. Reasoning from another thread is not shared as ordinary
conversation content.

Reading and search tools return one text answer; editing and other actions
return a structured result. Read [references](references.md) for descriptions,
outlines and links an agent can follow, [skills](use-skills.md) for versioned
instructions and scripts, and [announcements](notices-and-announcements.md#send-an-announcement)
for audience and delivery choices.

## Write documents together

Documents are pages of blocks beside a place or thread. Edit the same page
with another person, add a comment, mention a colleague or attach a file.
Changing the same block concurrently preserves the conflict for you to resolve;
the second edit does not silently overwrite the first. History identifies
people and agents and supports a guarded Put back of an earlier version.

A place can choose a document to Read first. Import and copy keep the typed
block structure; Outline helps move through the page. Export offers JSON,
Markdown, HTML, Typst source and PDF, with the actual supported choices shown.
An address for external media loads in your browser when you choose to play it.

**Browse folders** opens the document directory beside that place. Document folders
contain documents and other document folders; they are separate from project folders,
which can also hold topics and threads. A shared document folder opens directly,
without giving access to its surrounding project. Its menu includes **Copy link**
and **Permissions**.

## Arrange the screen and work in files

Layout chooses the views and where they sit: conversation, files, terminal,
usage, browser, display or other available views. It is your own screen layout;
How agents run chooses the harness, tools and credential placement. The two
choices are independent.

The activity bar at the left chooses Navigation or Resources; pressing the
chosen entry hides or shows that bar. A thread's right bar groups its views in
Conversation, Team, Code, Environment and Agent. Each group has sections you
can open, fold, hide and resize. Browsers and Displays list their items here
and open the viewer in the main area; Terminal opens below, including from
the status bar. The status bar also names the environment, branch, changed
lines and any limit that needs attention.

An empty terminal shows one current state, with its access explanation beside
the new-shell action. Environment facts use short rows; their questions open
the full network, credential and server explanations.

**Ctrl+K** (**Command+K** on a Mac) opens Find while a page is still loading;
you can search without waiting for that page, and your words stay when it appears.

Use **Open view…** in the finder, or **Add a view**, to reveal a section with
its group. The **Views** list shows each name with its group at the right;
**Resource lists** opens the navigation of resource kinds. Drag a section's header or use **Move to** to put it elsewhere;
**Reset layout** restores the standard frame and gathers existing main tabs into one group, keeping their content and drafts. Tabs and navigation rows
wrap when they need more room; the side bar's group names remain visible
beside their icons. On a phone the groups share one row of icons. Focus or
hold an icon to read its name; the open group is marked. Every choice stays
in the row. Main and
bottom tabs also have a list of every open tab with its unsaved mark and Close.
An org icon opens its home and Navigation from another page; pressing it again
while that home is in view shows or hides the panel. When there is less room,
the same named groups open as sheets below their controls. Press the chosen
group again to close its sheet. Each place's settings, personal settings, Management, Help and Usage keep
their section navigation inside the page. The global Navigation panel keeps
the view and width you chose.
Following a topic's own link returns to its content; Settings opens through its
specific address. Management first shows Administrator access while As admin is off.

Opening work normally selects its existing tab or adds one to the focused group. **Open in new tab**
creates another view; **Open beside** places a full view to its right. Every main tab can close.
Closing a thread tab leaves its agent running and keeps its draft. Two views of the same thread share
that draft while keeping separate reading positions and expanded items.

The address bar follows the focused content tab, and **Copy link** in its menu copies that address.
Refresh restores the workspace, targets and recoverable drafts. Back and Forward preserve other tabs.
**Close all main tabs** leaves an empty workbench with **Open…** and layout controls. **Reopen closed tab**
restores the most recently closed view. Group-close commands name their group explicitly.

**Layout** offers **2 rows × 4 columns** and **3 rows × 3 columns**, alongside manual splits.
Extra tabs share the last cell; empty grid cells stay available as drop targets. **Focus mode** hides
surrounding navigation and panels while retaining the main views. Leave through a tab menu, the
command palette, or the empty workbench. Your previous panel visibility and sizes return.

A contextual panel can **Follow active view** or keep a **Fixed target**. Moving one out of a sidebar
fixes the resource it currently displays. Its tab identifies that target, which stays open even when
the corresponding conversation tab closes. Use its menu to change the target or resume following.

The page tab names what you are reading; the breadcrumb names only its parents.
On a phone it is the full nearest-parent back link. **Home** opens your selected
org, and the org name in Navigation opens that home too. **Skip to main content**
is the first keyboard stop; F6 and Shift+F6 move between the visible areas.
The left bar's **Team chat** opens the existing chat in **Team** and puts the caret in its box,
keeping the current page open. The Resources landing lists every kind with its count and **Add**.
A topic's documents live in its **Documents** page, and
the rail's **Help** is the one guide entry.

Files shows the environment you can access, with code/text and other viewers.
[Files and attachments](files-and-attachments.md) explains previews, chip sizes and what reaches an agent.
Edits are conflict-aware. HTML is displayed as code; previewing a file does not
execute it. Shells belong to their opener; an environment manager can close
them. Their final retained output follows that place's recording and masking
policy. Files, home, processes, browser profiles and displays are shared by
threads in the same environment, so a directory does not isolate their work.

## Fork, switch and move

[Switch agent programs with history](switch-agent-programs.md) explains Carry,
Summary and None. [Plan and goals](plan-before-changes.md) explains
the composer's separate Plan control. [Follow progress](follow-live-work.md)
explains live counts, cancellation and your status layout.

Fork creates another thread owned by its maker. A fork that changes nothing
uses the ordinary one-request action. Fork to… opens the plan when you want
another destination, history, files or arrangement. Switching keeps the
existing thread and changes what it runs on; moving an environment carries
the environment's work to another location. Read the actual plan, required
permissions, history fidelity, selected files and detached mounts before
confirming. No history means the new session knows nothing of the source.

[Carry a thread to another agent](carry-a-thread.md) explains
the method shown in the plan and why a build may use a fallback.

Private mount writes accompany a copy when the destination can host that mode;
a detached mount says why. A large live tree requires an explicit file
selection with its bytes and time. Keep the job page for progress or cancellation;
the plan's commit point determines which side remains authoritative.
If the implementation cannot transfer a requested home or owned private layer,
the plan blocks that request; it does not silently omit the selected content.

## Keep and find work

The tree shows chats and documents with their places, allows unrelated branches
to fold, and can focus on your work or one place. Stars and recents help return
to it. Activity can summarize a place's immediate contents or expand through its
descendants. Archive pauses old work; deleted content remains restorable until
its retention expires. Authorized inspection of retained content does not
require restoring it first. [Finding things](finding-things.md#search) explains
the shared search and the finder.
