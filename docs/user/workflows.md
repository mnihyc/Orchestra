---
group: work
title: Work and collaboration
screens: /chat/$chatId, /place-chat/$nodeId, /org/$slug/messages, /workbench, /thread/$threadId, /topic/$topicId, /project/$projectId, /org/$slug, /doc/$documentId, /inspect/$kind/$id, /shared, /approvals, /document-folder/$folderId, /documents/$nodeId
keywords: chat, documents, share, approvals, steer, stop, fork, forward, layout, code, conflict, private
summary: Work with people and agents through threads, chat, documents and shared views.
---

# Work and collaboration

A topic keeps related threads, a chat and documents together. A thread is one conversation with one agent, with its own title, inputs, model choices and history. People who can open the same thread continue the same work, with the permissions the thread shows.

## Talk to people and agents

The chat and the composer of the agent have different recipients. Talk about a change in the chat. Send to the agent of the thread when you mean to give it work. Mentioning a person only notifies them.

- **Forward** sends the messages and files you select to people, chats or another agent. A plan shows what will be delivered first. It never shares the whole place quietly.
- **Steer.** While the agent runs a turn, a message you send can steer it if the agent program allows steering. The message keeps its place and is shown once.
- **Settings change at the next turn.** The model, the effort and other settings apply from the next turn; a steering message does not change them in the middle of a turn. To apply one sooner, press **Stop**, change the setting and continue with a new input.
- **Stop** ends the turn you see and holds the waiting inputs, so that the next queued input does not restart the work at once. **Continue** releases the held queue.

If a cancellation could not end a process of the agent program, the thread says so. Look at the running work or the terminal before you assume its effects ended.

## Read the conversation

The conversation shows the agent's messages, thoughts, plans and tools. A tool step opens to its command or file change, its result and the original call of the agent program. Large output keeps a useful part inline and a link to the whole. A tool Orch does not know keeps its original data. A sub-agent step names its role, its request and its state.

- Use **Find in the conversation**, or select across turns to copy, quote or forward.
- The view can show the conversation, the work, or more detail.
- A saved snapshot, or a look at removed content, is read-only.
- The live connection status says when new actions must wait. After a reconnection the page catches up from where it was; you do not have to recreate the thread.

## Decide a request

A pending request shows what would happen and the options the agent program gave.

- A command or file change can offer **Allow once**, a broader allowance, **Deny** or **Abort**.
- A question to a person asks for answers one by one. Answer the required ones before **Send answers** can be used.
- A form from an MCP server shows its fields and lets you accept, decline or cancel, as that request allows.
- A request for a port or a capability has its own plan and effects. Read them before you confirm.

Only someone with the permission the request needs can decide it. A viewer reads the card and sees why they cannot answer. A page that has lost its connection waits for it. If a colleague answers first, the card shows that result, and your late answer does not overwrite it.

## Share a thing or a place

**Share** shows what the person or agent will receive. A thread or a document is one thing. A topic, folder, project or organization offers the place itself, or the place with its contents. Those reaches differ, and the defaults of the place apply to content made later. A role in a place reaches what lies inside it, folders included. Giving one document does not give its parent.

- Holders can share what they hold, where the place permits it. Nobody can give more than they hold.
- Each entry records who gave it and any end time. The giver can take back their own grant.
- Managing the permission page, inheritance, **Private**, locks and defaults needs the separate permission to manage. **Private** previews who would lose access before it is applied.
- Use **View as** and the source of an entry to understand access. An administrator's override is confirmed and leaves a mark on the page and in the audit trail. It does not overcome a lock above it, an owner-only policy of a provider or a machine's own policy.

The same field names who gets the access: a person, a group you made, a role at a place, everyone who sees a place, or the members of an organization. The last two differ. Type a locator such as `dep2.sub3 developer`, `every dep2.sub3`, `members of dep2`, `dep2 reviewers` or `@alice`. A single match becomes its name and place when you press Enter. If several places match, choose the one whose full address you mean. A place you cannot see is not offered.

Groups can hold other groups and roles, and their current members get what the group is given. A cycle, or more than five nested groups, is refused. Renaming a place keeps its entries.

## Give agents the team's knowledge

Share a document, a chat, a thread or a place with the agents that should use it. Choose each area on its own:

| Area | Choices |
|---|---|
| Documents | **Can view**, **Can edit**, **Can manage** |
| Chats | **Can view**, **Can post**, **Can manage** |
| Threads | **Can view**, **Can send**, **Can manage** |

**Fork** and **Archive** are separate rows under **Advanced**, and **Manage** includes them. Private places below stay out unless you share them separately. An agent receives only what each giver can share now. When a giver loses a permission, that contribution stops and the others stay.

What Manage lets an agent do:

- **Documents.** Delete, move and restore documents. Making folders, copying, moving and restoring run at once, without a confirmation. Only deleting a document needs a preview and a confirmation.
- **Chats.** Delete anybody's message.
- **Threads.** Archive, unarchive, fork and delete. For each act on a thread or a chat, the agent first gets a complete preview without any change, and repeats its arguments with the confirmation word to carry it out. A plan that changed or expired does nothing. Deleting a thread settles running work and keeps the normal window for recovery.
- **A fork** uses the history, the binding and the workspace of the source. Its first owner is the giver, and it starts with no initial gift to an agent. The giver must meet the normal requirements for the destination and the binding.

Agents never own things and never administer places, permission pages or grants.

Sharing alone sends no instructions and starts no turn. Anyone who can Interact with the receiving thread can direct its agent using all of its permissions. **Read**, for the viewers of the thread, is a separate choice that is not ticked. Giving a document to an agent gives its viewers no **Edit** or **Manage**.

An agent's edits and messages name that agent. Its tool for sending messages takes a chat or a thread as target, with optional files. A chat message is visible to people. A message to a thread follows the delivery rules of the receiving topic. To start an idle thread, each send must say so on purpose, and the topic must allow waking idle threads (on by default). Each sending thread has its own limits per hour: 12 announcements and 60 messages to single threads. A limit of zero forbids that kind of send. The answer says whether the message was posted, queued or delivered. Announcements and milestones are recorded in the conversation and sent to the audience chosen. Reasoning from another thread is never shared as ordinary conversation.

Reading and search tools return one text answer, and tools that change something return a structured result. See [references](references.md) for descriptions, outlines and links an agent can follow, [skills](use-skills.md) for versioned instructions and scripts, and [announcements](notices-and-announcements.md#send-an-announcement) for audience and delivery.

## Write documents together

A document is a page of blocks beside a place or a thread. Edit the same page with another person, add a comment, mention a colleague or attach a file. If two people change the same block at once, the conflict is kept for you to resolve, and the second edit does not overwrite the first. The history names people and agents and lets you put an earlier version back, with a check.

- A place can choose a document to **Read first**.
- Import and copy keep the structure of the blocks. **Outline** helps you move through the page.
- **Export** offers JSON, Markdown, HTML, Typst source and PDF, and shows the choices that are really available.
- The address of a video or an audio clip loads in your browser when you choose to play it.

**Browse folders** opens the documents beside the place. A document folder holds documents and other document folders; it is not a project folder, which can also hold topics and threads. A shared document folder opens directly without giving access to the project around it. Its menu has **Copy link** and **Permissions**.

## Arrange the screen and work in files

**Layout** chooses the views and where they sit: conversation, files, terminal, usage, browser, display or another view. It is your own screen. **How agents run** chooses the agent program, the tools and where the credential lives. The two are independent.

- **The left bar.** The activity bar at the left chooses **Navigation** or **Resources**. Pressing the chosen entry hides or shows the bar.
- **The right bar of a thread** groups its views in **Conversation**, **Team**, **Code**, **Environment** and **Agent**. Each group has sections you can open, fold, hide and resize. **Browsers** and **Displays** list their items here and open the viewer in the main area. **Terminal** opens below, also from the status bar. The status bar names the environment, the branch, the changed lines and any limit that needs attention.
- **An empty terminal** shows one state, with the reason for it beside **Open a new terminal**. The facts of an environment use short rows, and their questions open the full explanations about network, credentials and servers.
- **Find.** **Ctrl+K** (**Command+K** on a Mac) opens Find while a page is still loading. Your words stay when the page appears.

Reveal a section with its group through **Open view…** in the finder, or **Add a view**. The **Views** list shows each name with its group. **Resource lists** opens the navigation of the kinds of resource. Drag the header of a section, or use **Move to**, to put it elsewhere. **Reset layout** restores the standard frame and gathers the main tabs into one group; their content and drafts stay.

In **Displays**, choose **Attach VNC server** for a VM or application that already serves VNC on this machine. Enter its loopback port and its VNC password if required. People with View can watch using their Orch account. Closing the display leaves the existing server running. On a connected Linux machine with an accessible local X11 session and x11vnc installed, **Share this machine’s desktop** shares that session. Where the machine has delegated `environment.vnc`, its policy manager can choose **Enable VNC** in the Displays tab.

An empty terminal shows one current state, with its access explanation beside
the new-shell action. Environment facts use short rows; their questions open
the full network, credential and server explanations.

- Tabs and navigation rows wrap when they need more room. The group names of the right bar stay beside their icons.
- On a phone the groups share one row of icons. Focus or hold an icon to read its name; the open group is marked.
- Main and bottom tabs have a list of every open tab with its unsaved mark and **Close**.
- An organization's icon opens its home and **Navigation** from another page. Pressing it again while that home is in view shows or hides the panel.
- When there is less room, the same groups open as sheets below their controls. Press the chosen group again to close its sheet.
- The settings of a place, your personal settings, **Management**, **Help** and **Usage** keep their section navigation inside the page. The global **Navigation** panel keeps the view and width you chose.
- Following a topic's own link returns to its content. **Settings** opens through its own address. **Management** shows **Administrator access** first while **As admin** is off.

Opening work selects its tab if it is open, or adds one to the focused group. **Open in new tab** makes another view, and **Open beside** puts a full view to its right. Every main tab can close. Closing a thread's tab leaves its agent running and keeps its draft. Two views of one thread share the draft and keep their own reading position and their own expanded items.

The address bar follows the focused tab, and **Copy link** in its menu copies that address. **Refresh** restores the workspace, the targets and the drafts that can be recovered. **Back** and **Forward** keep the other tabs. **Close all main tabs** leaves an empty workbench with **Open…** and the layout controls. **Reopen closed tab** brings back the last view you closed. The commands that close a group name their group.

**Layout** offers **2 rows × 4 columns** and **3 rows × 3 columns** beside manual splits. Extra tabs share the last cell, and empty cells stay available as places to drop. **Focus mode** hides the navigation and the panels around and keeps the main views. Leave it through a tab menu, the command palette or the empty workbench; the panels return as they were.

A panel can **Follow active view** or keep a **Fixed target**. Moving a panel out of a side bar fixes the resource it shows. Its tab names that target, and it stays open when the conversation tab closes. Its menu changes the target or goes back to following.

- The page tab names what you are reading, and the breadcrumb names only its parents. On a phone it is the full link back to the nearest parent.
- **Home** opens the organization you selected, and so does the organization's name in **Navigation**.
- **Skip to main content** is the first stop of the keyboard. **F6** and **Shift+F6** move between the visible areas.
- **Team chat** in the left bar opens the existing chat in **Team** and puts the cursor in its box, with the current page staying open.
- The **Resources** landing page lists every kind with its count and **Add**. The documents of a topic live on its **Documents** page. **Help** in the rail is the one entry to the guide.

**Files** shows the environment you may use, with viewers for code and text and for other kinds of file. [Files and attachments](files-and-attachments.md) explains previews, the sizes of chips and what reaches an agent. Edits are checked for conflicts. HTML is shown as code, and previewing a file never runs it.

A terminal belongs to the person who opened it, and a manager of the environment can close it. If Orch cannot end a terminal, it reports that and keeps it in the list. Its final output follows the recording and masking policy of the place. On Windows, the list of processes leaves out children whose identity cannot be told from a reused process number, and Orch refuses to stop an ambiguous tree. A process started during shutdown can escape the final check. Threads in the same environment share its files, its home, its processes, its browser profiles and its displays, so a directory does not keep their work apart.

## Fork, switch and move

- [Switch agent programs with history](switch-agent-programs.md) explains **Carry**, **Summary** and **None**.
- [Plan and goals](plan-before-changes.md) explains the separate **Plan** switch of the composer.
- [Follow progress](follow-live-work.md) explains live counts, cancelling and your status layout.
- [Carry a thread to another agent](carry-a-thread.md) explains the method the plan shows and why a program may use a fallback.

**Fork** makes another thread that its maker owns. A fork that changes nothing uses the one-request action. **Fork to…** opens the plan when you want another destination, history, files or arrangement. **Switch** keeps the thread and changes what it runs on. **Move** carries the work of an environment to another place. Before you confirm, read the real plan: the permissions it needs, how much of the history is kept, the files chosen and the mounts left out. **None** as the history means the new session knows nothing of the source.

A copy carries the writes to a private mount when the destination can hold that mode; a mount that is left out says why. A large tree that is live needs you to choose the files, with their size and the time. Keep the job page for progress or to cancel. The commit point of the plan decides which side stays in charge. If a home or a private layer that you asked for cannot be moved, the plan blocks the request; it never leaves the chosen content out quietly.

## Keep and find work

The tree shows chats and documents with their places, lets separate branches fold, and can focus on your work or on one place. Stars and recents help you come back to work. **Activity** can sum up what a place holds directly, or go down through everything below it. **Archive** pauses old work. Deleted content can be restored until its retention ends. Someone allowed to may look at retained content without restoring it. See [Finding things](finding-things.md#search) for the shared search and the finder.
