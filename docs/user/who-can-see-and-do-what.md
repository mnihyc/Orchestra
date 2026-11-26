---
group: access
title: Who can see and do what
screens: /thread/$threadId, /topic/$topicId, /project/$projectId, /org/$slug, /resources/environments/$id, /doc/$documentId, /chat/$chatId, /place-chat/$nodeId, /shared, /inbox, /approvals
keywords: how do I share, access, permission, permissions, holders, group, role, members, everyone, locator, override, create, make, new, start, project, topic, thread, sharing.allow_external, sharing.allow_holders, access.manage, access.external, capability.policy
heading_keywords.who-may-make-projects-topics-and-threads: create project, make project, new project, create topic, open topic, new topic, create thread, start thread, new thread, permission, 创建项目, 创建话题, 开始线程, 权限, 开始会话
summary: Ask for access, decide who may make things, share with the right holders, and understand an administrator's override.
---

# Who can see and do what

Every thing in Orch has a permission page. **Share** opens it. **View as** and **Reaches / Covers** explain who holds a permission and why. A thread, its provider and its environment each have a page of their own.

## Requesting access

When you open a [locked reference](references.md#locked-references) or are refused inside work you can already see, you can ask for the access you lack.

1. Choose **Request access**. The card offers the least access that is needed.
2. Choose how long you need it: until removed, one day, one week, 30 days, or a date. The end counts from the moment you send the request, and it shows on the request and in its notifications.
3. Add a reason if you like, and send.

Only one request of yours can wait on a thing. Take it back from the card if you no longer need it. A refusal still names who can give each permission that is missing. When every part of a request has an answer, the card shows the results, and you can ask again for access that is still missing. A request with several parts is withdrawn part by part: taking it back withdraws the ones still waiting.

**How a decider answers.** A person who can decide gives the access to the requester, on that one thing, with one duration button: one day, seven days, thirty days or **Always**. A place can change these choices. **Other…** opens the full decision, and **Deny** takes a note if you want to add one. Decisions are recorded and the requester is told. An entry with an end stops giving access when it ends. The decision starts with the end the requester asked for, and the decider can change or remove it. If the requested end has passed, choose a future end, or until removed, before you approve. The end of access is separate from the request's own deadline for a decision.

**Who is told.** Only the people named directly in the Owners row of the thing, if they can decide, get an Inbox item and see it under **Waiting for you**. Groups, roles, inherited managers and administrators are not told. When none of the owners can decide, the request goes up to the owners named directly in the places above, then to the administrators of the organization. If that finds nobody, everyone who can decide now is told. A request raised during a turn first goes to the person who started the turn, if they can decide it. A schedule, or a starter who cannot decide, uses the walk through the owners. Other people who can decide can act on the card, or under **Others you may decide**, without a notice, an unread mark or a count. Their permission is checked again when they decide.

Proposed plans and held messages follow the same rule. The first **Proceed**, **Keep planning** or **Run it** settles the request for everybody. **Run it** uses the approver's own current access to the model and keeps the original author named.

When one person decides, every recipient's copy says who decided what, and when. It leaves all lists of pending requests and their counts, and its Inbox item becomes read without another notice. It stays in the history. A second decision shows the outcome already recorded and grants nothing twice.

## Who may make projects, topics and threads

To make something you need a permission on the place that will contain it.

| What you want to make | Permission on the containing place | Who holds it by default |
|---|---|---|
| A project in an organization | **Contribute to Org** | The administrators of that organization |
| A topic in a project or folder | **Create topics**, part of **Contribute to Project** | The developers of that project and the roles above |
| A thread in a topic | **Start threads**, part of **Contribute to Topic** | The developers of that topic and the roles above |

**Contribute** is the middle level of an organization, project or topic. It also announces to the threads of the place, sets its status and tags, and adds environments for work there. **Create topics** and **Start threads** can be given alone, to let someone make things there and nothing else. Whoever makes a topic or a thread owns it. **Contribute** reaches no topic or thread inside: those follow their own pages and roles. The controls to make things check these individual permissions, also when they were granted without **Contribute**.

Open the menu of the containing place and choose **Share**. An organization also has **Roles and access** in its settings.

- Under **can take part**, the short form says what taking part allows at this kind of place.
- **By area** shows the permission rows named above.
- On **People**, a role gives its usual permissions at this place. Read them before you change a role just to allow one kind of creation.
- A person or a group can hold one lasting role at a place and others with an end date beside it, such as **Viewer** and **Developer** until tomorrow. The strongest role still in force applies, and when one ends the others stay.

For one person or group, use a **Custom** row in **By area**. Choose the permission, add the person or group from the picker, and allow it or refuse it. For example, on your organization, allow **Contribute to Org** for your group of project maintainers to let its members make projects without making them administrators. To refuse one person who would get that permission through a role, give that person a **Refuse** entry on the row.

Check the result in force and its source: a rule locked above this place still applies. Changing a row needs the permission to share or to manage the page that the row shows, and a read-only row names who can change it.

The **Defaults** of a place decide the starting permissions of things made inside it. They do not give anyone the permission to make those things in the containing place. The page of a new project and the **Contribute to Org** row answer different questions, and the same holds for topics and threads. Starting a thread also needs the separate access to the provider, the model and the environment that its creation dialog shows.

## General access

The form for a new thread, and the top of **Share**, offer **As the topic decides**, **Everyone in the topic can view**, **can take part**, **can manage**, and **Only people added**.

For example, choose **Everyone in Fix login can view** to let its topic's
role holders watch while owners and people added keep the higher lines.
**Viewers can open the environment** gives read-only looking at files, terminal
and screens up to the selected level; it never gives Use. Its provider,
environment, documents and chats keep their own permission pages.

## Talking to an agent in an environment

A message to the agent of a thread makes it act in the environment of the thread for the sender. Sending one needs three things:

1. **Interact** on the thread.
2. **Use** of its provider, with the model and effort.
3. **Use Environment** on the environment it runs in.

A refusal names everything that is missing at once, and one request for access asks for all of it. Each part reaches only the people who may give it.

**Use Environment** lies between **View** and **Manage**, so whoever manages the environment holds it. It starts with the Interactors where the environment is managed, and with nobody but its owners on a person's machine. Its row can also name work in a place: whoever may send to a thread there can then have that thread's agent act in the environment. To put a thread into an environment and to choose its directory there, **Manage Environment** is still needed. How far the thread's agent is confined (its **Tool access**) is chosen by whoever manages the thread, within the widest the environment allows: its machine's policy and its template, which stay with whoever manages them.

## Name the holder you mean

The same picker chooses people, groups you made, roles at a place, everyone who sees a place, and the members who belong to an organization. Members and viewers are different audiences. The **Use** row of a resource can also name the work in a place. Groups can hold the other kinds of holder, groups included. A cycle, or more than five nested groups, is refused.

Type `dep2.sub3 developer`, `every dep2.sub3`, `members of dep2`, `dep2 reviewers`, `acme.dep2.alice` or `@alice`. A single match becomes its name and full place when you press Enter. When two places match the end of the address, you have to choose the full address. A place you cannot see is never suggested.

For example, adding **Developers of Acme > dep2** to the **Can view** row of a thread lets the current developers, and the roles above, of that place see the thread. It does not make them members of the organization of the thread. **Reviewers, Acme > dep2** stays a different group from the reviewers of dep3. A rename or a move keeps the stored identifier and the entries. A holder whose place was removed stays inactive until it is restored.

## Share within your authority

Holders can share up to what they hold, where the place allows it. Managing the page, its inheritance or its locks needs the permission to manage. Naming an outsider also needs the permission to share outside. To name the group or the role of another place you only need to see its source, not to manage it.

When you share a place, you are asked whether you mean the place alone or its contents too. The preview says what follows and what stays private. Opening a thread does not open a provider, or **Files** and **Terminal**. That access belongs to the whole environment, so use separate environments for work that must stay apart.

## Limit where a provider's work runs

To share a provider for **Ask** alone, choose Use, open Options and select **Ask** under **Purposes** before adding
the people, group or role. The first grant is already restricted to Ask. Existing entries use the same purpose control in their options. People from another organization follow the same outside-sharing
policy. The organization's Ask list and the existing model and service allowlists still apply. This entry cannot
start ordinary agent turns or pay for other purposes. With every purpose selected, the entry permits all purposes;
selecting Threads alone does not also select paid search, page fetch, images, reviews, summaries or embeddings.

A provider's Use entry can narrow more than models and efforts: open the entry's options and set **Where**. Left out, it
reads **Anywhere**. Name environments, hosts or machines to limit the work to them; a host or machine covers every
environment on it, those made there later included. **Where** only narrows and never gives Use. An entry that names no
**Agent setup** allows only the forms in which the sign-in stays where it is kept; a copy or a handover is allowed by
naming it. An entry is one grant: when several entries let one person in, one of them has to allow the model, the effort,
the purpose, the agent setup and the environment together, and what a dialog offers is what the entries that allow the rest give. An
entry that names no places allows everywhere, with its own models and agent setup only. A thread that is started,
switched, forked or adopted into an environment outside the list is refused with the entry and the environment named, and
so is a thread that moves to another environment, or whose entry is narrowed, at its next turn; it is never moved quietly
to another environment or provider. A tool call an agent makes from a thread is work of that thread's environment. Work
that belongs to no thread's environment (Ask, summaries, embeddings and the semantic index) is not narrowed by **Where**
and still needs its purpose and model or service to be allowed.

You can give or change an entry only up to what one entry of yours allows. When it would allow more, the refusal names
the member that goes beyond: purposes, models, efforts, agent setups or places. A request for the use of a provider goes to the
people who could write the entry it asks for, keeping its requested purposes, models and efforts.
Before a model is chosen, a dialog offers the agent setups that every model chosen afterwards will find allowed.

## Understand an override

An administrator may manage a page while the page withholds its content. To do that, the override is explicit. The confirmation says that the administrator will be added to the page and that the act is recorded as an override. The page then shows that entry, and the audit trail marks it. An act that is already permitted needs none.

An override gives no token and no silent exception. It cannot lift a lock above the administrator's place, a subscription that is owner-only, or a machine's own policy. It is separate from the approval mode **Bypass** of an agent, which only governs how the agent program asks inside its boundary.

For example, an administrator of Acme who looks into a private topic first reads its administered row, which holds the least. Opening its page shows no conversation. Acting through the override adds the entry that is needed, and the audit trail marks it.

To answer a command request of an agent, or to open a port, use the permission described in [How approvals work](how-approvals-work.md). A role in a thread does not give every kind of access to a resource or an environment.
