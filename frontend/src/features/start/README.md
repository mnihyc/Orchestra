# Type-and-go and explicit binding

Built: the box that heads a topic ("Start a thread…") and a project ("Start a topic…"). Where the defaults of the place settle what a
thread there runs on, typing and pressing Enter makes the thread (and, from a project, the topic around it, and the environment where it
is to be a new one) with what was typed as its first message, and opens it; one line under the box names the provider, agent, display model,
localized effort and location. Absent defaults come from the person's last successful setup on this device, then a first usable
setup when there is no history. The shared device-choice store isolates accounts and lets Interface forget the setup. Every reuse is
checked against the current native answer; an unavailable configured default still needs a deliberate replacement. A saved location is
used only in the same place, while usable, and cannot override the place's fresh-environment rule. Quick start never borrows another
topic's directory merely because creating an environment is forbidden. Optional arrangement follows the current native preference;
remembered setup is not renewed credential-sharing consent. Starting work leaves defaults unchanged, including a topic's
default environment. Set as default is a separate explicit act. Start freezes the displayed binding/model/location before any asynchronous
creation; later inventory changes cannot substitute another agent or model in that attempt.
Change opens the dialog with the first message retained. Where something is missing the same place says which (a provider, an agent, a model, an
environment that may be used or made, a way of placing the agent) and asks for that one choice; for whoever manages the place it offers
to keep the choice as the default of the topic, the project or the org, the widest ticked, and for whoever may not it offers to ask. An
agent whose harness has not reported what it can do is waited for, with nothing offered that cannot run, and the box comes alive by itself
when the report arrives.

The dialog initially asks only Provider, Agent, Model and Where it runs, through ordinary shared selects. Effort, arrangement, access,
directory/source override, naming, first message and environment creation/registration are under Options. A missing arrangement opens
Options itself, so a required choice cannot be hidden behind a disabled act. Collapsing Options retains the chosen values. Machines are
read only while their optional registration tools are visible; their pending state does not pretend there are no machines.
Everything it lists is what the server says a thread there could be bound to, asked before the
thread exists: the providers with the reason one cannot be used, the agents and models each offers, every arrangement in the server's
order with the one the server would take marked and the closed ones shown off with its reason, the environments the person manages with
their reasons, and whether they may make one. A new environment is made for the thread from the defaults of the place, or by the form of the
resources screens, opened from here; a directory of a connected machine can be registered as one. What is ticked is what is sent, the first
arrangement as much as any other, and a refusal says why and is never answered by another way.

The binding-options route is served. A resumed non-default location gets its own native options answer before sending; Retry first
refreshes the location inventory, so removal cannot trap the person on an obsolete saved environment. No polling or extra default write
was added. The staged start's existing idempotent steps and standing-resource recovery are retained. Only its successful completion
remembers a setup; a refused or unfinished form is not the next setup. Both the native binding-options producer and client resolver reject
failed/removed environments; an old usable/default flag cannot authorize them, and an explicitly failed choice never silently makes a new one.
Blocked environment starts have one translated reason, the existing native start operation as Retry, and Run somewhere else for the thread's
manager. The first input stays held. The separate source-preparation notice is suppressed for that same environment block.
Failed-start recovery is keyed by thread and environment: an old retry refusal cannot appear after a native location switch. Retry
captures that environment's id and uses the existing start operation; Run somewhere else remains available while its request
is pending and opens the ordinary environment-focused, permission-checked change plan. A known failed file-preparation phase
is explained in the reader's language instead of printing the engine's file-job sentence. Neither action resends the first
input, changes defaults or adds polling. `failed-environment-recovery` exercises the real queue with full fake-world DTOs,
pending/refused replies and native target changes; its browser source covers both recovery paths at 1440/400 in both languages
and themes. Native file-job generation and input replay are tested separately from the browser's simulated completion.
The global setup presenter retains one prompt scope outside its data hooks. Help or a newer prompt pauses those hooks and keeps the draft;
returning resumes the same form rather than registering a new opening.

Regression suites: component `start-flow`, `start-submit`, `start-recovery`, `start-actions` and `general-access`; native API
`binding_options` failed-default projection; unit `start-choices`, `start-last-setup` and
`start-words`; browser `ffix-start`, `thread-start` and `start-help-switch`, plus the existing accessibility start suite. The last browser
suite covers 1440/400, both themes and EN/ZH. Review the full screenshots and request records when validating changes.

- `Arrangement.tsx`: The arrangement a thread gets: its type, where the harness and its tools run, the executor and what the agent works with there as the environment offers it, how the credential travels, and why another type is not offered.
- `BindingDialog.tsx`: Provider and the agents it offers, model and effort, environment, arrangement, `AGENTS.md` of the directory as it is or will be, options, and the access the thread will start with.
- `MissingDefault.tsx`: Ask only for the missing required choice.
- `QuickStart.tsx`: Create topic/thread/input from scope defaults, with model, effort and environment changeable in the line below; its shared eligibility predicate keeps parent screens from offering a duplicate New action.
- `RequestDefault.tsx`: Ask to set what new threads start with: the right to manage the place, asked of the people who can give it, with a line for the reason and where the request stands.
- `Where.tsx`: Where a thread runs: an existing environment that its creator manages (the topic's first, every other under More), or a new one with template, place, storage, mounts, starting files and who else manages it.
- `choices.ts`: What a thread would be bound to, worked out from what the server says it could be and what the person chose by hand: the one place that decides what is shown as chosen, what is missing and what is sent.
- `options.ts`: The current native answer for a place and its chosen or remembered environment, with refused/placeholder answers withheld, inventory-first retry and live invalidation instead of polling.
- `pending.ts`: What the start screens ask of the API before the generated types name it: what a thread made in a topic, or in a topic made in a project, could be bound to, asked before the thread exists.
- `start.ts`: Starting a thread: the steps from what the person chose and typed to a thread that is open, each shown as it happens and none made twice.
- `words.ts`: The words for what the server says in codes and members: why a provider, a profile, an environment or an arrangement cannot be had, how an arrangement is described, and what a thread would run on in one line.
- `lastSetup.ts`: Typed, per-person successful-setup device choices, revalidated by the native resolver.
