---
title: Who can see and do what
screens: /thread/$threadId, /topic/$topicId, /project/$projectId, /org/$slug, /resources/environments/$id, /shared, /approvals
keywords: how do I share, access, permission, permissions, holders, group, role, members, everyone, locator, override, create, make, new, start, project, topic, thread, sharing.allow_external, sharing.allow_holders, access.manage, access.external, capability.policy
heading_keywords.who-may-make-projects-topics-and-threads: create project, make project, new project, create topic, open topic, new topic, create thread, start thread, new thread, permission, 创建项目, 创建话题, 开始线程, 权限
---

# Who can see and do what

## Requesting access

A [locked reference](references.md#locked-references) keeps the name that was
shared and offers the least access it needs. Add an optional reason and send.
Only one request by you may wait on a thing; take it back from the card if it is
no longer needed. A refusal inside work you already see still names who can
give each missing permission.

Deciders can grant the requested access to that person on that one thing with
one duration button. The shipped choices are one day, seven days, thirty days
and Always. The place can change them with `access.quick_grant_durations`.
Other… opens the full decision; Deny accepts an optional note. Decisions are
recorded and the requester is told. A timed entry stops giving access when it
ends.

Only eligible people directly named in the thing's Owners row receive an Inbox
item and appear in Waiting for you. Groups, roles, inherited managers and admin
reach create no fan-out. If none of those people may decide, the request walks
up the places' directly named owners, then eligible org admins. If that walk finds
nobody, every person who can decide now is told. A request raised during a turn
first tells the person who started that turn, when they can decide it; a schedule
or an ineligible starter uses the Owners walk. Other
deciders can act on the card or in Others you may decide, without a notice,
unread mark or pending count. Current authority is checked again when deciding.

Proposed plans and held messages follow the same rule: the first **Proceed**,
**Keep planning** or **Run it** settles the request for everybody. Run it uses the
approver's own current model access and keeps the original author named.

When one person decides, every recipient's copy says who decided what and
when. It leaves all pending lists and counts, and its Inbox item becomes read
without another notice. It stays in history. A second decision shows that
recorded outcome and grants nothing twice.

Each thing has a permission page. Share opens that page; **View as** and
**Reaches/Covers** explain who holds a permission and the complete reason.
A conversation, its provider and its environment have separate pages.

## Who may make projects, topics and threads

Making something is a permission on the place that will contain it:

| What you want to make | Permission on the containing place | Who holds it by default |
|---|---|---|
| A project in an org | **Interact Org** (`org.interact`) | That org's admins |
| A topic in a project or folder | **Interact Project** (`project.interact`) | That project's developers and higher roles |
| A thread in a topic | **Interact Topic** (`topic.interact`) | That topic's developers and higher roles |

Open the containing place's menu and choose **Share**. An org also has
**Roles and access** in its settings. Under **can take part**, the short
form says what participating allows at this kind of place. **By area**
shows the individual permission row named above. On **People**, a role
gives its ordinary permissions at this place; read those permissions
before changing a role merely to allow one kind of creation. A person or
group can hold one lasting role at a place and others with an end date beside
it, such as Viewer and Developer until tomorrow: the strongest role still in
force applies, and when one ends the others stay.

For one person or group, use a **Custom** row in By area: choose the
permission, add the actual person or group from the holder picker, and
allow it or refuse it. For example, on your org allow Interact Org for
your project-maintainers group to let its members make projects without
making them admins. To refuse one person who would otherwise receive
that right through a role, give that person a Refuse entry on this row.
Review the effective result and its source; a rule locked above this
place still applies. Changing a row takes the sharing or page-management
authority shown there; a read-only row names who can change it.

The place's **Defaults** decide the starting permissions of things made
inside it. They do not grant permission to make those things in the
containing place. A new project's page and the org's Interact Org row
therefore answer different questions. The same distinction applies to
new topics and threads. Starting a thread also needs the separate
provider/model and environment access shown by its creation dialog.

## General access

The new-thread form and the top of Share offer **As the topic decides**,
**Everyone in the topic can view**, **can take part**, **can manage**, and
**Only people added**. Everyone here means people holding Viewer or a higher
role at that topic. An outside person with a direct View entry alone is not
that role audience. Higher lines keep owners and people added; a different
mix of rows shows **Custom**. These are the same permission-page changes you
can make in its full form. A document offers View, Edit and Manage.

For example, choose **Everyone in Fix login can view** to let its topic's
role holders watch while owners and people added keep the higher lines.
**Let them into the environment** gives read-only looking at files, terminal
and screens up to the selected level; it never gives Use. Its provider,
environment, documents and chats keep their own permission pages.

## Talking to an agent in an environment

A message to a thread's agent has it act in the thread's environment for the
sender. Sending one therefore needs three things: **Interact** on the thread,
**Use** of its provider with the model and effort, and **Use Environment** on
the environment it runs in. A refusal names everything missing at once, and one
request for access asks for all of it, each part reaching only those who may
give it.

Use Environment lies between View and Manage, so whoever manages the
environment holds it. It starts with the Interactors where the environment is
managed, and with nobody but its owners on a person's machine. Its row may
also name the work in a place: whoever may drive a thread there may then have
that thread's agent act in the environment. Putting a thread into an
environment, its directory there, and how far its agent is confined still take
Manage Environment.

## Name the holder you mean

The same picker chooses people, custom groups, roles at a place, everyone
who sees a place and members who belong to an org by domain. Members and
viewers are different audiences. A resource's Use row can additionally name
the work in a place. Groups may contain the other holder kinds, including
nested groups, with cycles and more than five nested groups refused.

Type `dep2.sub3 developer`, `every dep2.sub3`, `members of dep2`,
`dep2 reviewers`, `acme.dep2.alice` or `@alice`. A unique locator resolves
on Enter into its human name and full place. Two matching suffixes require
an explicit full-address choice. A hidden place is never suggested.

For example, adding **Developers of Acme > dep2** to a thread's Can view row
lets current Developers and higher roles there see that thread. It does not
make them members of the thread's org. **Reviewers, Acme > dep2** stays a
distinct group from Reviewers in dep3. A rename or move keeps its stored
identifier and existing entries; a removed holder-place is inactive until
restored.

## Share within your authority

Holders share up to what they hold where the place permits it. Managing the
page, its inheritance or locks takes its management right. Naming an outsider
also needs the outside-sharing authority. Naming another place's group or
role needs seeing its source, not managing that source.

Sharing a place asks whether you mean the place alone or its contents too;
the preview states what follows and what remains private. Opening a thread
does not open a provider, or Files and Terminal. Those doors belong to the
whole environment, so use separate environments for work that must stay apart.

## Understand an override

An admin may manage a page while the page withholds its content. To perform
such an act, **override** is explicit: the confirmation says the admin will
be added to the page and the action recorded as an override. The page then
shows that entry; the audit marks it. An act already permitted needs none.

An override supplies no token or silent exception, and cannot lift a lock
above the admin's place, an owner-only subscription or a machine's local
policy. It is separate from the agent's approval mode **Bypass**, which
governs only how the agent program asks inside its existing boundary.

For example, an Acme admin inspecting a private topic first reads its
minimal administered row. Opening its page supplies no conversation. Acting
through the explicit override adds the needed entry and audit mark.

To answer an agent's command request or open a port, use the appropriate
authority in [How approvals work](how-approvals-work.md), rather than assuming
a thread role grants every resource or environment door.
