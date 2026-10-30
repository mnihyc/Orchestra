# System management

Management at `/admin/<section>` is for a system admin in a browser session: the entry stands in the
account menu of whoever is one and nowhere else, and the address of a section read by anybody else
shows the refusal, with what it needs. A section is its own chunk of the build. Everything is shown
as under way until the server has answered; what is not served is absent or says so.

Built here: the Overview (the parts of the system with their last heartbeat, the queues, machines
and threads, and the small charts of what the server stored about itself over a day, a week, a
month, a year or since it was installed, drawn by uPlot, which is loaded when the first chart is
shown; the notice every page shows), Users, Organizations (the two limits of each root org, naming
an owner, restoring those deleted), Sign-in (who may register, confirming addresses, passwords, a code
by mail with its lock, identity providers), Hosts (with when a host counts as full and what being
full does), Runtimes, Capabilities, Harnesses (the table of what each adapter supports is read from
the descriptors), Agents of the system, Plugins, Storage, PKI, Settings (the settings editor of the
settings screens at system scope), and Audit, which is the system's trail of the trace screens with
their filters, grouping, restoring and export. A person's name in Users opens their card, and what they
did is one step from it.

Waiting for the screens of other parts (`Waiting.tsx` is what each would show): Channels (the
channels editor at system scope), Templates and MCP presets (the resource lists at system level),
Access defaults (the defaults page given the system as its place) and Rates (the rate card and the
catalogue panel of the usage screens). Until each is joined it is not listed and its address is no
screen. Not served by the server: the preview of a change to the system's defaults. An identity
provider's Test asks the server, which reads the provider's description of itself and its keys.

- `Management.tsx`: The frame of Management: who may be here, the list of its sections, and the section in view with its name and what it is for.
- `sections.ts`: The sections of Management: the address of each, its mark, the group it is listed under, and whether the part it shows is built in this tree.
- `search.ts`: What the address of a Management section says besides which section it is: the search and filters of a list and the span of the charts, each read with everything it may not say left out.
- `useSearchBox.ts`: A search box whose text is written to the address once the typing has paused, and not at every key.
- `Waiting.tsx`: What a section of Management shows while the part it is made of belongs to a screen that is not joined to this build yet: it says so, and where the same thing can be reached meanwhile.
- `StatusTag.tsx`: A few words that mark a row: that a person is suspended, that an org is personal, that a host is full. The words are always written; the tone only helps.
- `SettingRows.tsx`: Plain settings of the system, each changed where it is shown: a switch, a choice or a number, with where its value comes from and the way back to what Orch ships.
- `Overview.tsx`: Health, heartbeats, queues, versions, charts of the stored metrics, and maintenance banner.
- `Installation.tsx`: Read-only release and verified backup records, explicit release lookup, database-only coverage and the existing operator commands.
- `InstallationProgress.tsx`: The independently served migration step, rows where known and elapsed time while the API is unavailable.
- `BackupRestoreList.tsx`: The verified manifest's actual inventory, with unknown counts, recorded locations and uninspected host copies kept distinct.
- `Charts.tsx`: The small charts of the Overview: threads by state, turns and how long they took, machines connected, events waiting, what waits for a cap or a lock, and the blob store's room, over a day, a week, a month, a year or since the system was installed.
- `charts.ts`: What the charts of the Overview are made of: the span each one covers, which measurements it asks for, and how what the server stored becomes lines.
- `BannerEditor.tsx`: The notice every page of the system shows across its top: its text, its level, and how long it stays, put up or taken down from here.
- `Users.tsx`: Users: creation with what their own org starts as, suspending and resuming, admin status, reset links that may switch a second step off, and a name that strangers' wrong tries have closed.
- `UserDialogs.tsx`: The acts on a person that ask first: making one, the link by which they set a new password, suspending and resuming them, and making them a system admin or not.
- `Organizations.tsx`: Every root org with its owners and its size, each of which a system admin enters as an Admin of it; naming an owner where none is left; and the two limits of each: whether it takes members, and whether it holds orgs inside it.
- `SignIn.tsx`: Who may register; confirming addresses; password sign-in; a code by mail, with its lock; how wrong tries are counted; and identity providers.
- `IdentityProviders.tsx`: The identity providers people may sign in through: the list with each one's callback address to copy, and the form that adds or changes one.
- `Channels.tsx`: The system's own ways out of the application: its mail server, which also sends confirmations, resets and sign-in codes, its bot, push, webhooks and installed kinds; whether root orgs without a mail server send through it; and which private addresses channels may reach.
- `Hosts.tsx`: The system's hosts: capacity, a meter of what their units declare they need, what each does when full, load, drivers, enrollment, runtimes, mount locations, recycle and move.
- `hosts.ts`: How a host's room is read: the memory its units declare they need against the memory it reports, when that counts as full, and what being full does there.
- `Runtimes.tsx`: Both pools on every host: environments and provider runtimes, what each is behind in, Update required, and updating those ticked.
- `Templates.tsx`: The system's templates for environments and for providers' runtimes, each list with its seeded one.
- `Mcp.tsx`: The system's MCP presets.
- `Capabilities.tsx`: Per-driver capability definitions.
- `Harnesses.tsx`: Adapter descriptors and supported builds, with what each harness keeps as memory.
- `HarnessVersions.tsx`: Each program's inherited policy, current target and compatibility status, use counts and reviewed batch updates.
- `ProgramCapabilities.tsx`: A program's declared capabilities, grouped as compact states with reasons when unavailable.
- `ProgramList.tsx`: A compact list of agent programs and their reported builds and status.
- `harnesses.ts`: What an adapter says it supports, read out of the descriptor it declares and from nowhere else: native fork, images, steering, an agent terminal, reporting what a turn is doing, and what it keeps as memory.
- `Agents.tsx`: The system's profiles.
- `Plugins.tsx`: External plugin lifecycle and errors.
- `AccessDefaults.tsx`: The system's defaults page: what every org starts from, the shipped values one click away, and how many orgs follow each row.
- `Rates.tsx`: The catalogue of estimates: its date and where it came from, the families in it, Upload a file…, Fetch now, and Put back the shipped one.
- `Settings.tsx`: The system's registry settings.
- `Storage.tsx`: Blob capacity and consumers.
- `SemanticSearch.tsx`: Search by meaning: what the system names for it, and each root org's index with its state, progress and estimate, the disk it takes and why it waits.
- `Pki.tsx`: CA and machine certificate status.
- `pki.ts`: How the authority's figures are read: a fingerprint in groups a person can compare by eye, and how many whole days a certificate has left.
- `Audit.tsx`: The system's audit and export.
- `exports.ts`: The few things an export of a trail is made of: the days between two days, the day it is today, the most days one file may span, and what the file is called.
