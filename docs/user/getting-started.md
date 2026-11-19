---
title: Getting started
screens: /, /setup, /login, /register, /invite/$token, /org/$slug, /project/$projectId, /topic/$topicId, /thread/$threadId
keywords: first thread, team, solo, invitation, provider, key, sign in, scripted example, start here
---

# Getting started

Alice starts a small code fix with an agent. Bo opens the same thread, reads
what happened and answers the agent's waiting request. They talk in the human
chat beside the work; Bo then continues the same agent conversation. Their
topic keeps the threads and the document of findings together.

## Choose your place

Sign in at the address your team gave you. An invitation joins the organization
it names; a personal invitation is for its named address, and a link invitation
has its own expiry and number of uses. The first administrator uses the setup
page on a new installation. A deployment may also enable open registration or
an external sign-in provider; the sign-in page shows only the methods offered
there. An implementation reserved by the deployment is unavailable. If an
invitation shows the wrong account, sign out there and sign in as the intended
person; a failed sign-out stays on the invitation so you can retry.

If the sender can no longer grant the invitation's access, ask them for a new invitation.

Every person has a space of their own. Choose that for solo work, or select your
team's organization in the left bar. Start here belongs to the selected place;
it is an ordinary document with reading and history. Creating another team
organization requires the invitation or permission that the interface shows.
The tree can remember where you last worked. A project may contain folders,
and a topic lies in a project or folder.

## Add a provider

Open Resources, then Providers, or follow Start here's provider step. A provider
is the source of a model and its credential. For an API provider, use the offered
preset or specify its endpoint and protocol, then give your own key. For a
subscription, choose one of the adapter's offered sign-in methods and follow
its code, browser or import instructions. A file import copies its contents;
a runtime file path names a file already in that provider's runtime.

Choose the models the provider may serve. A model's display name is what people
see, and its routing name is what is sent to the vendor. Speeds, access programs
and context sizes must first be opened on the model by a provider manager.
The thread chooses its own value from that offered set. A sign-in, saved key
or configured endpoint proves setup; a completed turn proves the full path works.
If the provider needs attention, its page says what is missing.

The provider has owners and a home. Share Use only with people who should spend
that key or plan. A teammate may watch a thread without being allowed to start
turns on its provider. Model use and access to a thread are separate checks.

## Prepare an environment

An environment holds the files and tools the work will use. Create one from an
available template on a host you may use, or connect a computer you manage.
The chooser shows actual host/machine availability and the requirements of
the selected template. A connected computer's local policy comes first and
can refuse a server request, including an administrator's.

Threads in one environment share its workspace, home, processes, browser
profiles and displays. The thread's directory is its working place; opening
Files or a terminal reaches the whole environment. Use a separate environment
for work that needs a separate trust boundary.

## Start a thread

In your selected project or folder, open or create a topic and choose New thread.
Give the thread a title that names the work. Choose its provider, profile, model
and environment from the available choices. How agents run explains where the
harness, tools and credential will be placed. The first choice follows the
place's effective defaults; a missing choice explains what is required.
When the environment list contains both, **Can use** and **Ask for access**
separate those you can use from those you can ask to use. Selecting the latter
checks who can grant access and offers a request when available, without starting
the thread. Choose an environment you can use or receive access to the one you
requested before starting the thread.

Check the directory and standing instructions before the first input. The
directory's instruction file is a workspace file; standing instructions are
a setting. Trusting project configuration is a separate decision for hooks,
plugins and tool servers. An empty instruction file can still inherit parent
instructions according to the harness's own rules.
An existing path used for the thread's directory must be a directory, not a file, link or special entry.

Send a small request whose result you can recognize. The thread moves from
starting to running, shows the agent's text and steps, and ends the turn with
its outcome and usage. A blocked or failed turn keeps its reason on screen;
do not treat a saved setup step as a final answer.

## Work together

Share the thread with Bo at the level needed for their work. Can view lets
them read; driving the agent and deciding its approvals require the relevant
rights shown by the permission page. Sharing one thread does not share its
topic or its environment. If the next input needs a provider or model Bo
cannot use, the page offers the ordinary way to ask for access.

Use the human chat composer to talk to Bo. Use the thread's agent composer
to address the agent. A colleague answers an agent only by deliberately
sending or forwarding to it; a human discussion does not silently become
another agent turn. Keep findings in a topic document and share that document
with another thread's agent when it should read or edit the findings.

## Try the scripted example

An installation that offers Example agent and Scripted subscription can run
the same conversation controls without a model. It emits deterministic steps,
questions and outcomes from the scripted adapter. Its commands and MCP results
are simulations, so it demonstrates collaboration and controls rather than a
real code fix or a model's quality. The adapter's directive manual is in the
source distribution at `extensions/agents/fake/README.md`.

## Find help and recover access

Press `Ctrl+K` (`Command+K` on macOS) to open the finder. Type a name, command
or question; use `F1` for the current screen's help. A question mark beside a
choice explains it in place. Read [finding things](finding-things.md#the-finder)
for search and keyboard navigation, [workflows](workflows.md) for decisions and
shared documents, and [resources](resources.md) for provider/environment choices.

Your sign-in may require an authenticator or a mailed code when the place
permits it. If a code request fails, the page shows the error and lets you try
again. An accepted request does not say whether a code could be sent. Repeated
wrong proofs are bounded by the sign-in policy; a known
browser has separate protection from strangers' guessing. Use the recovery
page or ask the installation's administrator for a one-use reset link if you
cannot complete the second step. A provider sign-in is separate from your
Orch account and is managed on that provider's page.
