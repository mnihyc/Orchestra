# Writing for Orch

How Orch writes everything a person reads: the README, the guides, the words on the screens, error messages, release notes and commit messages. It has two parts: the rules, and the glossary of the words a person meets. Write with them; check a draft against them before it goes in.

## Rules

### The reader first

1. **Say what the reader can do before what the system guarantees.** A page starts with the action or the question it answers.
   - Before: "A group chat has a name, a home, visibility and managers."
   - After: "A group chat has a name, public or private visibility and managers, and belongs to a **home**: an organization, project, topic or thread."
2. **One page, one task.** The title is what the reader does ("Add a provider"), not a noun ("Resources and movement"). A page whose title needs "and" is two pages. A reference (every setting, every error) is another kind of page and lives in the reference.
3. **Write "you", in the active voice, in the present tense.** Not "the person", not "it is offered", not "currently".
   - Before: "Controls are offered according to your rights; only the author edits their text."
   - After: "You see only the actions you may use. Only the author can edit a message's text."
4. **Use first, limits after.** A limit or a refusal comes after the steps, in its own short section ("If it does not work"), and says what to do.

### Shape

5. **Steps are numbered, one action each, and start with where.** "In **Resources → Providers**, choose **Add provider**." A task page runs: one sentence of introduction, **Before you start**, **Steps**, **What you will see**, **If it does not work**, **Next**.
6. **Short paragraphs, short sentences.** At most five sentences and about 80 words in a paragraph, about 25 words in a sentence. Facts of one kind become a list.
   - Before (one paragraph of 230 words): "A provider's Use entry can narrow more than models and efforts: open the entry's options and set **Where**. Left out, it reads **Anywhere**. Name environments, hosts or machines to limit the work to them; a host or machine covers every environment on it, those made there later included. ..."
   - After:
     > In a provider's **Use** entry, open the entry's options and set **Where**.
     > - Left out, it reads **Anywhere**.
     > - Name environments, hosts or machines to limit the work to them. A host or machine covers every environment on it, including ones made later.
     > - **Where** only narrows. It never gives Use.
     > - An entry is one grant: the model, the effort, the agent setup and the place must all be allowed by the same entry.
7. **Headings say what the reader does or asks.** Sentence case. Never "Overview", "Introduction", "Where", "Example", "What to expect".
   - Before: "## Where", "## Example", "## What to expect".
   - After: "## Read a message from an admin", "## Send a message to every member".
8. **Setting keys, defaults and numbers the reader does not act on stay out of the guide.** The reference carries them; the guide says what the reader can change and links.
   - Before: "The shipped hourly limits are 12 announcements (`broadcast.rate_per_hour`) and 60 individual thread messages (`broadcast.thread_messages_per_hour`)."
   - After: "You can send 12 announcements and 60 thread messages an hour. An administrator can change both limits (see the settings reference)."
9. **Define a product word once, where it first appears, in plain words, and use it the same way everywhere.** The glossary below is the list. A word with two meanings gets two names.

### Words

10. **No words that belong to the people who build Orch.**

| Do not write | Write |
|---|---|
| batch, lane, cut, candidate, integrator, scaffold, seam (for the work of building Orch) | what the thing is: the release, the thread, the interface, the release process |
| proof, proven, unrun, gate, conformance, walk, fixture | check, tested, passed, not tested yet |
| reserved, placeholder | **Not available yet**, or leave the feature out |
| native (meaning "the vendor's own") | the program's own, built in, or name the program |
| harness, adapter (for an agent program) | agent program |
| this release, currently, at the moment, first release | nothing (the page is about the version it ships with), or "since 0.1.2" |
| simply, just, easily, obviously, ordinary, existing, actual, explicit | delete the word |
| allow, enable (for what the product does) | let, turn on, or say what the reader can do |

11. **Things that change do not belong in a guide:** version numbers, counts, timings, dates. They go in the release notes or the reference, which are versioned.
12. **Say it once.** If two pages overlap, one links to the other.
13. **A message that says "no" says what to do next:** whom to ask, what to choose, where to go.
14. **Name things; do not describe them.** A button is a short verb or noun ("Send", "Archive"), not a sentence.

### Status words

The same on every page: **Available**, **Experimental** (works, may change, not supported) and **Not available yet**. A feature that is not **Available** says so in its first lines. Pre-release status is stated once, in the README and in the release notes, and is the same everywhere.

### Release notes and commit messages

15. **A release note bullet** is one sentence, 30 words or fewer, saying what a person can now do or no longer suffers, with a link to the guide. Details go in the guide. The order of a release's notes is fixed (`.github/release-notes/TEMPLATE.md`): upgrade notes, breaking changes, highlights, changes, known issues, compatibility, downloads.
16. **A commit message:** a subject in the imperative, capitalised, at most 72 characters, no full stop; a blank line; a body that says why, wrapped at 72. A release commit is "Orch 0.1.3" with a short body. Never in a subject or a body: a batch, round or lane number, a branch name, "candidate", "integrator", "proof", "unrun", the path of an internal document, a decision reference, a person's name, a tool's name.
    - Before: "Seal the release schema of the thirty-eighth batch"
    - After: "Record the migrations shipped in 0.1.2"

## Glossary

The words of the screens, one definition each. **Say** is the name we use; **Do not say** are names that belong to something else, to the people who build Orch, or to nothing. "Org" is the short form of organization and may stand in a label, a column or a menu item where the kind is clear; write "organization" in a sentence.

### People and places

| Say | What it is | Do not say |
|---|---|---|
| **organization** (org) | The top of a team's work. It holds people, departments, projects, and its own settings and permissions. | team, company, tenant |
| **department** | An organization inside another organization. | sub-org, division |
| **project** | A place inside an organization that holds topics and further projects (shown as folders in the tree). | repository |
| **topic** | A place for one piece of work. It keeps related threads, a chat and documents together. | channel, ticket, issue |
| **thread** | One conversation with one agent: its inputs, its model choices and its history. | conversation, session, job |
| **chat** | People talking to each other beside the work. No agent answers in a chat. | channel, human thread |
| **document** | A page of blocks that people and agents write together, with comments and history. | note, page |
| **place** | An organization, project, topic, thread or document: somewhere in the tree where work and permissions live. | location, node |
| **person**, **people** | A human with an Orch account. | user (except for the API) |
| **member** | A person who belongs to an organization or a group. | user |
| **agent** | The AI that works in a thread. | bot, assistant |
| **service account** | An account for a program, not a person. | bot account |
| **group** | A set of people that permissions can name. | team |
| **home** | The place a thing belongs to and takes its defaults from: a group chat's home is an organization, project, topic or thread; a resource's home is an organization. | parent (in a sentence to a person) |

### Where work runs

| Say | What it is | Do not say |
|---|---|---|
| **environment** | Where work runs: its files, tools, processes, browser and display. Every thread in an environment shares them. | sandbox, box, unit; container, except where the screen means the container itself and not the environment's files (removing or recreating it) |
| **workspace** | The files an environment keeps for work. A thread's **directory** is the folder in it where its agent starts. | project folder |
| **machine** | A computer a person connected to Orch with the connector. | computer, device, client |
| **host** | A Linux machine with a node, where Orch makes managed environments in Docker. | server (that is Orch itself), node |
| **node** | The program on a host that makes managed environments. | agent, daemon |
| **connector** | The program on a machine that connects it to Orch and does what Orch asks within its owner's limits. | agent, client |
| **managed environment** | An environment Orch made from a template on a host. | unit, container |
| **template** | The description (a Dockerfile and settings) from which Orch makes a managed environment. | image, blueprint |
| **gateway** | The entrance that machines and managed environments connect to. | proxy, relay |
| **file relay** | Files uploaded to an environment or read from it pass through the server without being kept on its disk. | proxying |
| **snapshot** | A saved copy of an environment's files that Orch keeps and can bring to another environment. | backup |
| **checkpoint** | The record of an environment's files before a message, kept so that the files can be restored to before that message. | snapshot, restore point |
| **backup** | A saved copy of the whole system, made by `update.sh`. | snapshot |

### Providers, agent programs and work

| Say | What it is | Do not say |
|---|---|---|
| **provider** | What supplies a model: an API key or a sign-in, and the models it may serve. | vendor, backend |
| **model**, **effort** | The model a thread asks, and how much work it is asked to put in. | engine, level |
| **agent program** | The program that runs an agent: Claude Code, Codex, DeepSeek Harness. | harness, adapter, CLI |
| **agent profile** | How an agent program is set up on a machine or host: version, options and variables. | agent setup (that is where it runs), config |
| **turn** | One request to an agent and the work until it ends. | run, round |
| **steer** | Send guidance into a turn that is running. | interrupt (that ends the turn) |
| **Stop** | End the turn that is running and hold what waits. | cancel, kill |
| **queue** | Messages that wait for the current turn to end. | backlog |
| **approval** | A request that waits for a person to decide. | permission request |
| **approval mode** | How an agent program asks before it acts. Four names: **Ask** (asks before actions it does not trust), **On request** (asks when it needs a decision or its file rules block an action), **Auto** (the program's own reviewer decides eligible requests) and **Bypass** (allows every action in the environment; questions and plans still need an answer). | permission mode, autonomy |
| **Plan** | A switch of the composer: the agent proposes a plan and waits before it changes anything. It is not an approval mode. | |
| **skill** | Versioned instructions and files that agents can use. | plugin, prompt |
| **MCP server** | A tool server that an agent program can call. | tool provider |

### Access

| Say | What it is | Do not say |
|---|---|---|
| **permission** | One thing a person or group may do on a place or resource, shown on its permission page. | right, privilege |
| **role** | A named set of permissions at a place: **Viewer**, **Interactor**, **Developer**, **Admin**. | level |
| **View**, **Interact**, **Edit**, **Use**, **Manage** | The access levels. **View** reads. **Interact** takes part in a thread or chat, which includes sending to the agent. **Edit** changes content. **Use** spends a provider or runs work in an environment. **Manage** changes and administers the thing. | take part (use "Interact"), full control |
| **Contribute** | The middle level of an organization, project or topic: it includes **Start threads** and **Create topics** and the other everyday work of a place. | write |
| **access request** | A request for missing access, sent to the people who can give it. | permission request |
| **grant** | Access given to a person or group, with an end date if you choose one. | |

### Things you open

| Say | What it is | Do not say |
|---|---|---|
| **Inbox** | Where a person reads notices and requests addressed to them. | notifications (that is the delivery) |
| **announcement** | A message to a place's members that can also wake agents. | broadcast |
| **resource** | A provider, agent profile, environment, machine, host, template, skill or MCP server that people use. | asset |
| **Management** | The screens of the system's administrators. | admin panel, console |
| **audit trail** | The record of every change of access and every act on the system. | log |

### Words of the guide

Words that a few screens and pages use and that need one meaning each.

| Say | What it is | Do not say |
|---|---|---|
| **holder** | Whoever holds a permission: a person, a group, a role at a place, everyone who sees a place, or the members of an organization. | grantee |
| **locator** | The text typed in a holder field to name a holder, such as `dep2.sub3 developer`. | query |
| **Use entry** | A grant on a provider that names the models, efforts, services and places that may be used. | rule, policy |
| **agent setup** | Where an agent program runs, who runs its tools and where its sign-in stays: **Run with provider**, **Forward model calls**, **Copy sign-in access**, **Move sign-in** or **Use local sign-in**. | arrangement, mode |
| **recovery rule** | A rule that says what happens when a model call fails: wait and try again, continue, fail over, or wait for a person. | retry policy |
| **candidate** | A provider and model that a recovery rule may fail over to. Only used with call recovery. | fallback |
| **capability** | Something an agent can use that a model or a provider supplies: web search, page fetch or image generation. | feature, tool |
| **driver** | How a host makes environments: Docker, or a directory of a machine. | backend, engine |
| **container runtime** | The program that holds a container: runc, gVisor or Kata. | runtime (alone) |
| **hand over** | Let Orch manage the policy of a role of a machine. Settings kept on the machine still win. **Dedicated** declares a machine that holds nothing else. | delegate, give control |
| **primary gateway**, **tunnel gateway** | The gateway a connector uses; the one the tunnel of a tool uses, which can differ. The **gateway catalogue** is the list that administrators edit. | endpoint |
| **plan window**, **reset** | A period of a provider's plan with its usage and limit; one use of a reset that the provider offers to start a window again. | quota period |
| **mount**, **volume**, **kept layer**, **private view** | How an environment attaches or keeps data: a source attached in a mode; storage that outlives the container; what a same-image replacement keeps outside the volumes; changes kept apart from the source. | disk, drive |
| **build**, **pin** | A version of an agent program; to fix the version that runs. | release (that is Orch's), lock |
| **program home** | The folder in which an agent program keeps its own files, sessions and sign-in. | home (alone; that is the place a thing belongs to) |
| **executor** | The program through which an agent program that runs elsewhere reaches an environment. | runner |
| **override** | An administrator's confirmed entry on a page they could not otherwise manage, recorded in the audit trail. | bypass (that is an approval mode) |

### Words of the product kept in English

Product names and the names of programs are not translated: Orch, Codex, Claude Code, DeepSeek Harness, Docker. The Chinese editions use the Chinese words of the catalogue (`frontend/src/i18n/zh-CN.json`) for everything else; when a Chinese word and an English word here disagree, the catalogue is corrected, not the reader.

## Add or change a guide page

The user guide in `docs/user/` is also the help inside the application. Each page is built into the web application and belongs to the screens it explains.

- Name the exact router path of each screen in the `screens` front matter of the page, including detail pages and redirects.
- Give the page a title, search keywords, and a plain, short `summary` in both editions' front matter. The summary ships with the page and uses no model.
- Connect the question card of the screen to the heading that explains it. The help checks report a route, a translation or a destination that is missing, and a hint that links to a page or a heading that is not there.
- Keep both editions together when behavior changes. Every English heading needs a Chinese one, and a translated heading keeps the address of its section with front matter such as `heading_id.索引状态: index-states`; the reader still sees the heading as it was written.
- Add the page to `docs/user/README.md` under the group it belongs to.

## Checks

A word-list check over `docs/` and the README (rule 10), warnings for long sentences and paragraphs (rule 6), a link check, and a heading check (rule 7). For every page ask: could someone who has never seen Orch finish this task from the page alone?
