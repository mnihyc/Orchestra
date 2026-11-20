---
group: getting-started
title: Getting started
screens: /, /setup, /login, /register, /invite/$token, /org/$slug, /project/$projectId, /topic/$topicId, /thread/$threadId
keywords: first thread, team, solo, invitation, provider, key, sign in, scripted example, start here
summary: Set up your place, a model and an environment, then start shared work with an agent.
---

# Getting started

Alice starts a small code fix with an agent. Bo opens the same thread, reads what happened and answers the request that is waiting for the agent. They talk in the chat beside the work, and Bo then continues the same thread with the agent. Their topic keeps the threads and the document of findings together. This page takes you through the same steps: choose your place, add a provider, prepare an environment, start a thread and work with a colleague.

## Choose your place

1. Sign in at the address your team gave you. On a new installation, the first administrator uses the setup page.
2. If you have an invitation, open it. It joins the organization that it names. A personal invitation is for its named address, and a link invitation has its own expiry and a number of uses. If an invitation shows the wrong account, sign out there and sign in as the right person. When signing out fails, you stay on the invitation and can try again. If the sender can no longer give the access the invitation names, ask for a new invitation.
3. Choose where you work. Every person has a space of their own, which is right for solo work. For team work, select the organization of your team in the left bar.

The sign-in page shows only the methods this installation offers. It can also offer open registration or a sign-in through another provider. A method that is not offered is unavailable.

**Start here** belongs to the place you selected. It is an ordinary document with reading and history. To create another team organization you need the invitation or permission that the screen shows. The tree remembers where you last worked. A project can hold folders, and a topic lies in a project or in a folder.

## Add a provider

A provider is the source of a model and of its credential.

1. Open **Resources → Providers**, or follow the provider step of **Start here**.
2. For an API provider, use the preset on offer, or give the endpoint and its protocol, then give your own key.
3. For a subscription, choose one of the sign-in methods offered for the agent program and follow its code, browser or import steps. Importing a file copies its contents. A file path in a container names a file that is already in the container of that provider.
4. Choose the models the provider may serve. The display name of a model is what people see, and its routing name is what is sent to the vendor. A manager of the provider first has to open speeds, access programs and context sizes on the model. A thread then chooses its own value from what is offered.

A sign-in, a saved key or an endpoint you configured proves that the setup is done. A turn that completes proves that the whole path works. If the provider needs attention, its page says what is missing.

A provider has owners and a home. Share **Use** only with the people who should spend that key or plan. A teammate may watch a thread without being allowed to start turns on its provider: use of a model and access to a thread are separate checks.

## Prepare an environment

An environment holds the files and the tools that the work uses. Make one from a template on a host you may use, or connect a computer you manage (see [Connect a machine](connect-a-machine.md)). The chooser shows which hosts and machines are available and what the chosen template needs. The local policy of a connected computer comes first, and it can refuse a request of the server, an administrator's included.

Threads in one environment share its workspace, its home, its processes, its browser profiles and its displays. The directory of a thread is where its agent works. Opening **Files** or a terminal reaches the whole environment. Use a separate environment for work that needs a separate boundary of trust.

## Start a thread

1. In your project or folder, open or create a topic and choose **New thread**.
2. Give the thread a title that names the work.
3. Choose its provider, agent profile, model and environment from what is available. The first choice follows the defaults of the place, and a choice that is missing says what it needs. The text **How agents run** explains where the agent program, the tools and the credential are placed.
4. When the list of environments holds both kinds, **Can use** separates the ones you can use from those under **Ask for access**. Choosing one of the latter checks who can give access and offers a request, when one is possible. It does not start the thread. Choose an environment you can use, or wait for the access you asked for, before you start.
5. Check the directory and the standing instructions before the first input. The instruction file of the directory is a file of the workspace, and standing instructions are a setting. Trusting the configuration of a project is a separate decision for hooks, plugins and tool servers. An empty instruction file can still inherit the instructions of a parent directory, by the rules of the agent program. A path you use as the directory of a thread must be a directory, not a file, a link or a special entry.
6. Send a small request whose result you can recognize.

The thread goes from starting to running, shows the text and the steps of the agent, and ends the turn with its outcome and its usage. A turn that is blocked or failed keeps its reason on screen. Do not take a saved step of setup for a final answer.

## Work together

1. Share the thread with Bo at the level they need. **Can view** lets them read. Sending to the agent and deciding its approvals need the permissions that the permission page shows. Sharing one thread does not share its topic or its environment.
2. If the next input needs a provider or a model that Bo cannot use, the page offers the usual way to ask for access.
3. Use the composer of the chat to talk to Bo, and the composer of the thread to talk to the agent. A colleague answers an agent only by sending or forwarding to it on purpose. A discussion between people never becomes another turn of the agent quietly.
4. Keep the findings in a document of the topic. Share that document with the agent of another thread when it should read or edit them.

## Try the scripted example

An installation that offers **Example agent** and **Scripted subscription** can run the same controls for a conversation without a model. It produces steps, questions and outcomes that are fixed. Its commands and MCP results are simulations, so it shows collaboration and controls, not a real code fix or the quality of a model. The manual of the directives of the scripted agent program is in the source distribution at `extensions/agents/fake/README.md`.

## Find help and recover access

- Press **Ctrl+K** (**Command+K** on a Mac) to open the finder. Type a name, a command or a question. **F1** opens the help of the current screen, and a question mark beside a choice explains it where it stands.
- Read [Finding things](finding-things.md#the-finder) for search and navigation with the keyboard, [Work and collaboration](workflows.md) for decisions and shared documents, and [Resources and movement](resources.md) for the choices of providers and environments.

Your sign-in can need an authenticator or a code sent by mail, when the place allows it. If a request for a code fails, the page shows the error and lets you try again. An accepted request does not say whether a code could be sent. Repeated wrong tries are bounded by the sign-in policy, and a browser you know has separate protection from guesses by strangers. If you cannot finish the second step, use the recovery page, or ask the administrator of the installation for a reset link that works once. A sign-in to a provider is separate from your Orch account and is managed on the page of that provider.
