# Providers, profiles, environments, hosts, connected machines and templates

Environment deletion accepts the existing leased removal job in its transaction when due, retains its frozen volume window and keeps an authorized recovery view after the active row disappears. Confirmation reads unit/attach facts; a machine reference alone does not distinguish a managed unit from a registered directory. The environment Settings entry uses one native registry editor and its actual source; template Settings retains one first-template guide link.

All eight resource kinds, including MCP servers and mounts, use `ResourceListFrame`: one page-header
Add, search and compact Filter/Sort buttons in `FilterBar`, then selection, name and a primary action visible
without hover at every width. A reader can count and clear a selection; selection grants no bulk
permission. Secondary facts yield room before the main action. MCP/mounts offer permitted Edit or
ordinary Open through their native route. Component and real eight-kind browser layout sources are
authored but unrun; the integrator owns executable and rendered proof. The kind strip appears only
when the native Resources navigation view is hidden. That view's + opens these same lazy creation
flows; a resource detail header holds only the item's own acts. Template creation uses Add too.

The seven non-provider detail pages have at most six visible primary tabs. Software, storage, tools
and settings use the same section navigation below them; old tab addresses still open their exact
section. Ordinary overviews use one compact fact list, while diagnostic reports and complete JSON
are opened through Details. Explicit technical field editors remain one disclosure. Long fields,
registered entries, threads, runtimes and locations use twelve-row pages, retaining visited drafts
while hidden readers pause. The integrator measures the two-screen height budget at both widths;
source-written layout is not rendered proof. Providers retain their own composition.

The screens of the resources a team's agents run on, under `/resources`: machines, providers, agent
profiles, environments, templates and hosts, each as a list that narrows by what is typed and by its
filters (kept in the address) and as a page with its tabs, with the dialogs that make one.

Built: machines (the command that enrolls one, seen when it connects; its page, what is registered
from it, its local policy as enforced there), registering an agent profile, a provider and an
environment from a machine, providers (lists by what one owns, may use and manages; a page with
Overview, Models, Sign-in and keys, Usage, Settings and Activity; interface-first creation with
independent credential keeper/runtime and visible sources), agent profiles, environments (made from a template on a host, started,
stopped and kept awake from their page), templates (made, their recipe edited under the revision that
was read) and hosts (capacity, storages, what runs on one, whether it takes new environments, the
command that enrolls one). Access on each page shows what the reader holds and why, and opens the
permission page of the resource.

Every screen is laid out from the outside in. A list has the row of kinds above its heading only when
the Resources navigation is not already shown, and a
resource's own screen has none: its heading says the way there, `Resources / Kind / Name`, each part a
link (the kind leads back to the list as it was left, with what was typed and chosen in it), and a small
screen has an arrow beside the title for the same way back. Every row of every list has its act at its
end: the main act stays visible at every width; the rest are in a menu that is also the menu under the right mouse button, with the
menu key and with `Shift+F10`, and only where the reader may;
rows that are ticked share the acts that all of them can take, above the table, and an act that destroys
names what it is about to destroy. Every page has Edit: the name, the description and whatever else the
kind's update request takes, with the rest of the same form shown read only and the reason beside it,
and the same menu in its heading. A selected Settings panel omits a duplicate Edit or Rename entrance
when its native fields already offer that edit. Delete remains in the header menu once. A "?" stands beside every option that is not plain, its words in the
catalogue under `help.hints`; `hints.ts` lists the forms and the browser test opens each of them and names
the options that have none. A machine whose certificate lapsed or was revoked is enrolled again from its row
or from its Certificate tab; what that request is, until the generated types hold it, is in `pending.ts`.

MCP presets and mounts have served lists, pages, Share and revisioned Edit forms. Mount locations distinguish
existing paths from host volumes, and deleting a volume names its data and asks for the mount's name. Their
uses are read separately, with private uses counted. Provider runtime configuration saves variables and secret
names; profile descriptor overrides keep reported defaults separate. An environment can change its home within
its root org, with the change of settings and defaults explained before saving. Resource hints open the guide.

A provider's Models tab shows what each model may be run with there: the groups above the list with how
many models each has, and for each model its groups, the efforts it takes, its context (its own window and,
where a manager opened one, its largest) and the speeds and access programs that are switched on, with what
its source offered and nothing yet switched on set apart. Its row editor, headed "What this model can be run
with here", is where a manager switches a speed, an access program or a larger context on, names what
threads start with (all of the context as shipped), puts the model in groups and gives its rate with steps
for large prompts; what the server refuses is shown at the field. It lists the caps on the provider that
cover the model, and "Add a limit…" and "Limit this group…" call the form that makes a cap once the usage
screens provide one.

Provider sign-ins follow native methods and the keeper's live permission. Their runtime controls,
usage and limits are implemented. Each module below names its concrete responsibility; native
capabilities remain subject to the machine's actual declaration and current server admission.

- `ResourceList.tsx`: Everything's bounded resource pager, server-side search/focus/filters and common sort/add controls. The frame preserves each kind's columns, owner fallback, admin marks, acts and selection of loaded rows; a failed continuation retains them and can be retried.
- `ResourceCreation.tsx`: One lazy Add dispatch for the landing, Resources + and empty start/composer/First steps choices; detail pages do not create unrelated resources. Creating keeps an unfinished starter, with an explicit choice refresh and credentials link.
- `ListControls.tsx`: Filter contains participation and kind-specific criteria; Sort contains order/direction, with active marks. The parent retains query state while their compact popovers close; opening either makes no request.
- `inventory.ts`: Typed readers of Everything's tagged resource views; no browser-side access or inventory filter.
- `Landing.tsx`: Every resource kind with its native authorized count and its existing lazy Add flow, sharing one summary read with navigation.
- `summary.ts`: Landing and navigation share a single count read per committed screen opening, including when the page's code arrives later.
- `jsonObject.ts`: Object configuration is checked before it is sent, keeping invalid text in the form.
- `agents/Overrides.tsx`: A profile's declared descriptor overrides, saved without turning reported defaults into overrides.
- `mcp/Edit.tsx`: Ordinary name, command/arguments or URL and write-only authentication fields; Advanced public JSON preserves extra settings, with revisioned saves and machine-owned imports.
- `mcp/definition.ts`: Lossless connection-field projection, transport/authentication choices, local validation and separate secret writes.
- `mounts/Edit.tsx`: Revisioned mount labels, attachment target and whether its data is writable.
- `environments/Doors.tsx`: Files and shells opened from an environment's own page with its own permissions.
- `environments/Restart.tsx`: Reviews what a same-unit restart ends and preserves before choosing its turn grace.
- `Facts.tsx`: A list of facts, each a name and what is true of it: the shape in which a resource says what it is, and a code box for a command to copy.
- `DetailSections.tsx`: Shared section navigation within at most six primary resource tabs, twelve-row paging, retained drafts and paused hidden readers; lazy Details and heading acts without duplicate entrances.
  Each section's content has its own fault boundary, outside which its navigation stays usable.
  ResourceTabs recognizes this layout and leaves its navigation outside the outer tab boundary;
  ordinary panels retain their default tab isolation.
- `templates/SchemaSettings.tsx`: Driver-schema fields through the shared settings row, localized search, opt-in selected sections/paging and the same source/reset/unknown-member behavior.
- `templates/SchemaForm.tsx`: Schema input primitives and invalid auxiliary draft tracking; technical JSON stays an explicit editor.
- `templates/TemplateEditor.tsx`: Atomic template form/JSON/validation state; detail pages opt into compact sections and Details for the complete JSON view.
- `HomePicker.tsx`: Home choices reuse live rows and load only the selected org, with an authorized selected-place fallback and failed-read Retry. Opening creation never fans out over every org.
- `NewAccess.tsx`: The "Who may use it" choice that ends a dialog which makes a resource: only me, everyone in the home, or as the home decides, with one line saying what the choice gives.
- `ResourceAccess.tsx`: The Access tab of a resource: what the person holds on it and through what, and the way to its permission page.
- `acts.tsx`: What a row of a list of resources can do, said once: its button, its menu, its right-click menu and the acts several ticked rows share.
- `trail.ts`: Where a resource screen lies: Resources, then its kind, then itself; and the filter each list was last left with, so that the way back returns to it.
- `hints.ts`: The forms of the resources screens that hold an option which is not plain, and the hint that stands beside each option, by the form it stands in.
- `Hint.tsx`: The hint of a setting, by the key the server names it with, and the hint of a field that a descriptor declares.
- `EditDialog.tsx`: The frame of the dialog that edits a resource: the name and what its kind lets be changed after it was made, and under them what cannot be changed, read only in the same form with the reason.
- `ResourceNav.tsx`: All eight kind links as a fallback while the native Resources navigation view is hidden; its visible-slot lifetime restores them without guessing from viewport width.
- `SettingsTable.tsx`: The settings a resource resolves, each with its value and where that value comes from, and whether the reader may change it.
- `UnitConfiguration.tsx`: Both pools use the shared setting rows for values, provenance and timing; after-start commands have one dedicated editor with provider-owner authority. Startup failures retain one translated recovery line, with saved diagnostics under Details and a bounded preview that expands in place.
- `UnitCommands.tsx`: Named literal programs and arguments run after a unit starts, with bounded deadlines.
- `templates/UnitFields.tsx`: Template commands and registered programs stay visible and are edited as literal argument lists.
- `amounts.ts`: Amounts of processor, memory and disk in the words a reader uses: processor in cores, memory in binary units, disk in gigabytes, each with the language's grouping.
- `harnesses.ts`: The names of harnesses as the system's adapters title them.
- `executorNames.ts`: The executor and tool-set words supplied with an arrangement or readiness report.
- `programNames.ts`: The name supplied for a program, in the reader's language.
- `tabs.ts`: The sections of a resource's page, as the address names them: read by the routes before the pages themselves are fetched.
- `search.ts`: What the address of a list of resources says: the words searched for and the one filter, read so that whatever is not one of the things it may say is left out.
- `providers/ProviderList.tsx`: Providers by what one owns, may use and manages, and all of them for an admin: status, plan, runtime, turns running, and what each is behind in.
- `providers/labels.ts`: Provider names use the shared place label, with only the origins the reader may see.
- `providers/SetupNotice.tsx`: A setup reason is shown once with the act that addresses it; observers can read the reason too.
- `providers/Provider.tsx`: Six active-only sections with one setup action; Settings composes lifecycle, sources, ordinary setting rows, arrangements and access, Activity composes threads/sessions, Delete stays in the header menu.
- `providers/AddProvider.tsx`: Examples precede the interface fields they fill. Both sign-in menus use `signInChoices.ts`: unread/unusable programs stay named with their actual reason and the existing Add profile flow, keeping this draft and home. Credential/connection locations and actual source names replace runtime terms; compact menus omit boilerplate. Keys stay deferred, policies narrow choices, refusals return to their step, and acknowledgement closes once. Sources use the shared inline program registrar.
- `agents/AddAgent.tsx`: Available adapters or a program on a machine, using the same registry and native descriptor; profile creation retains that machine/kind and custom-agent help.
- `providers/ReadFailure.tsx`: Read failures preserve Retry and the ordinary authority-checked Delete action without fabricating facts or permissions.
- `providers/Runtime.tsx`: A provider's runtime, made with the provider and kept as long: its template and host, status, start and stop and when it stops by itself, when a stopped one is started to renew, pending updates and Update, the job that makes it with a build's log, and harness processes.
- `providers/Arrangements.tsx`: How agents may run on this provider: owner choices narrowed by the credential keeper's physical policy. Unavailable ways cannot be enabled; an older saved allowance can still be removed. Setup and writes use the same policy as binding.
- `providers/Models.tsx`: Saved models and the provider's already-reported suggestions, without another native read on opening. Acceptance preserves reported names; the action answer updates both the saved collection and its parent provider. Speeds and access programs remain manager choices.
- `providers/ModelLifecycle.tsx`: A model's pause and deletion show their exact current effects without moving its threads.
- `providers/Login.tsx`: Native sign-in state and fields, with an interactive screen flow preferred even for an older cached descriptor; deliberate token import remains available.
- `providers/Key.tsx`: Managed or machine-held API keys. Saving shows the pending source read and installs its returned provider, including model suggestions, without another GET or guessed readiness.
- `providers/Endpoints.tsx`: Endpoints of an API provider with the options their format declares, presets, and the secrets of an endpoint.
- `providers/Threads.tsx`: Visible bound threads and batch switch.
- `providers/Usage.tsx`: The usage screen opened on a provider: every thread that ran on it, of whatever org.
- `providers/Caps.tsx`: Caps on use of the provider by person, group, model or project, in tokens, in credits or in turns at once; and who may let a turn past one.
- `providers/History.tsx`: What was done to the provider, by whom and when: its page, its sign-in, its models and arrangements, its runtime.
- `providers/DangerZone.tsx`: Disable and delete: what each ends and what it leaves, with the threads that depend on the provider and Switch threads… beside a refusal.
- `providers/Settings.tsx`: The provider's Details source editor and the runtime values in force, each with where it comes from.
- `providers/state.ts`: What a provider is and how it stands, as one phrase each: "Subscription · Pro", "API key · 2 endpoints", "Needs sign-in", "Limit reached · resets 14:00", with what to do about it.
- `providers/acts.tsx`: What can be done with a provider, from its row in the list and from its own page: take the next step of setting it up, open, switch on or off, edit, rename, copy its link and delete, each only where the reader may.
- `providers/Edit.tsx`: The dialog that edits a provider: its name; what it was made as, shown as fixed; and its endpoints, how agents run on it and its runtime's values, each changed on its own tab.
- `agents/Profile.tsx`: Harness version, capabilities, parameters and configuration.
- `agents/AddAgent.tsx`: The dialog that makes an agent profile in an org: which harness it runs and what it is called. A profile for a harness installed on a machine is registered from that machine's page.
- `agents/AgentList.tsx`: The agent profiles a person may use or manage: name, harness, version pin, where each is usable (any managed runtime, or one machine) and whether it is switched on, with the means to add one.
- `agents/descriptor.ts`: What an agent profile's harness declares about itself, read with care: its adapter wrote it, and a profile whose harness has not yet reported has nothing.
- `agents/acts.tsx`: What can be done with an agent profile, from its row in the list and from its own page: open, switch on or off, edit, rename, copy its link and delete, each only where the reader may.
- `agents/Edit.tsx`: The dialog that edits an agent profile: its name, the build of its harness it runs and whether it is on; the harness it was made for, shown as fixed, and its profile, changed on its own tab.
- `machines/Machine.tsx`: Identity, facts, roles, registered resources and certificate.
- `machines/Enrollment.tsx`: Platform installer command and enrollment progress. The issuer's opaque receipt identifies the exact enrollment, through one initial/reconnect status read and native user news; known resource feeds carry its online transition. Unrelated arrivals never finish the dialog, and no inventory poll runs.
- `machines/EnrollmentRoles.tsx`: Named connector roles with independent handover, and a separate Node choice using the host enrollment.
- `machines/Policy.tsx`: The machine's local policy as it is enforced there, before every page: shown to whoever sees the machine, changed on the machine, and from here only for the keys its owner handed over, by whoever manages what each bounds; the arrangement types the machine allows and what its exec node may offer among them.
- `machines/PortPolicyValue.tsx`: A machine's reported port ceiling in human words; command text retains its exact original JSON.
- `machines/Tools.tsx`: A machine's Tools, for all its environments: what each exec node offers, the MCP servers switched on there, and those found in the harnesses' own configuration, by name, with Import…
- `machines/Activity.tsx`: What Orch asked the machine to do, and what it refused.
- `machines/MachineList.tsx`: The machines of a person: their name, system, whether they are connected, which connector they run, when they were last seen and what they do, with the means to add one.
- `machines/MachineLink.tsx`: The same machine name link in resource rows and recorded storage locations.
- `machines/RegisterAgent.tsx`: The dialog that registers an agent profile for a harness found on a machine: which harness, what the profile is called and which org it belongs to.
- `machines/RegisterEnvironment.tsx`: The dialog that registers a directory on a machine as an environment: its name, the directory, its org, whether files may be written and shells opened from outside, and who may use it.
- `machines/RegisterProvider.tsx`: The dialog that registers a provider from a machine: the login a harness already holds there, or an API key that is typed on the machine, with the command that types it.
- `machines/Registered.tsx`: What is registered from a machine: its agent profiles, its providers and its environments, each with where it leads, and the three acts that register one more.
- `machines/facts.ts`: What a machine reports of itself, read with care: the facts are what its connector wrote, and a machine that has not reported yet has none.
- `machines/Programs.tsx`: The shared page and inline registration for agent adapters/provider sources: actual declarations, Test, Remove, exact kept configuration and Reload, with failures retained beside the draft.
- `machines/Installation.tsx`: Observed account/service/data/version and exact commands for the installed instance; missing legacy facts remain unknown.
- `machines/acts.tsx`: What can be done with a machine, from its row in the list and from its own page: enroll it again, refresh what it reports, open, edit, rename, copy its link and revoke it.
- `machines/Management.tsx`: One confirmed, audited permission-page grant before an administrator manages another person's machine.
- `machines/Edit.tsx`: The dialog that edits a machine: its name, which is all its request takes, with what the machine reports of itself shown as fixed.
- `machines/EnrollAgain.tsx`: The dialog that enrolls a machine again: the command to run on that machine, what stays (its name, its page, what is registered from it) and what goes (its old certificate), and the wait for the machine to come back.
- `environments/Environment.tsx`: An environment as a resource: owners, home and address, place, the threads in it and where each runs, its page of two levels and who reaches into it, its Tools, pending updates, its own settings, and removal with its reason; for a directory on a person's machine, its switches.
- `environments/Tools.tsx`: An environment's Tools: what its exec node offers, in order, and whether Orch's own tools are offered beside it; the MCP servers switched on there; those found in a harness's own configuration, by name, with Import…; and a server of the environment's own, where its machine allows one to be added.
- `environments/Settings.tsx`: An environment's applicable unit values, provenance, apply timing and editors, including commands after start.
- `environments/Network.tsx`: The actual and requested network, its safe policy sources, and recovery through the existing change plan.
- `environments/AgentReach.tsx`: One permission-cut overview of the network, secret names, mounts, each visible thread's key placement and browser profiles, shared by the page and inspector. Its agent-reach cache is separate from the people/permissions reach; settings and profile answers apply their native rows, with only a related visible overview read again when the answer omits it. The native redacted home path supplies inherited dependencies even with navigation closed, without a tree request.
- `environments/History.tsx`: What was done to the environment and in it, by whom and when: shells, files written, an agent's changes turn by turn, threads put in and taken out; narrowed by kind, path, thread and person.
- `environments/EnvironmentList.tsx`: The environments a person may use or manage: where each comes from, how it runs, how it stands, the threads in it, whether it is behind, and what the reader may do there; with the means to register a directory on a machine.
- `environments/MakeEnvironment.tsx`: The dialog that makes an environment from a template on a host: its name, the template, the host or the one with the most room, and who may use it; the job that makes it is followed from the page it opens on.
- `environments/words.ts`: An environment in a few words: how it runs ("From Default on host-2", "~/work on Alice's laptop"), how it stands, and what the reader may do there.
- `environments/acts.tsx`: What can be done with an environment, from its row in the list and from its own page: start or stop one that Orch made, enable or disable, open, edit, rename, copy its link and delete, each only where the reader may.
- `environments/Edit.tsx`: The dialog that edits an environment: its name and the two switches that decide what people may do in it from outside; where it runs and what it belongs to, shown as fixed; and its own settings, changed on their tab.
- `environments/Remove.tsx`: The dialog that deletes an environment: it is named to be sure, and says what goes and what stays, with the threads that depend on it and the refusal the server gives while any are bound.
- `templates/Template.tsx`: Driver-schema forms by kind (for environments, or for providers' runtimes), where its image comes from (a reference, a Dockerfile, an archive), its limits and what an environment made from it declares it needs, how its exec node is launched and what it offers, and sharing.
- `templates/Import.tsx`: A new template from a Dockerfile, an image or an archive: what was read, as notes above the form.
- `templates/NewTemplate.tsx` and `QuickTemplateFields.tsx`: Short creation from a complete driver example, with name, place, purpose, driver and image or host account; the returned template opens for remaining settings and access. Plugin-required fields remain available. Creation uses the save-time schema check without a preview request for each edit.
- `templates/SchemaSettings.tsx`: Searchable shared setting rows for the driver's schema, with localized reference text, actual template/default/automatic sources, gear-only default/reset and retained in-row structured editors. Sparse edits preserve unknown siblings; a reader sees values under one authority notice. The resource page supplies bounded section navigation.
- `templates/SchemaForm.tsx`, `schema.ts` and `TemplateEditor.tsx`: Reusable schema controls, local reference navigation and lossless edits, plus the form/JSON switch and server pointer diagnostics. Unfinished raw drafts block save/view changes; template saves retain the reviewed whole-resource revision and explicit conflict review.
- `templates/TemplateExamples.tsx`: Driver-owned worked examples choose their driver and fill the same configuration the form edits.
- `templates/TemplateHosts.tsx`: Template advice distinguishes missing reports from enrolled hosts that are currently offline.
- `templates/TemplateList.tsx`: The templates a person may use or manage, of both kinds: where each comes from, its driver, where its image comes from, how it confines, how many units were made from it and whether it is on, with the means to add one.
- `templates/spec.ts`: What a template's spec says in a few words: where its image comes from, how it confines what runs in it, what it needs and what its exec node offers. The spec is the driver's own JSON, read with care.
- `templates/acts.tsx`: What can be done with a template, from its row in the list and from its own page: enable or disable, open, edit, rename, copy its link and delete, each only where the reader may.
- `templates/Edit.tsx`: The dialog that edits a template: its name, its description and whether units may be made from it; what it was made as, shown as fixed; and its recipe, changed on its own tab.
- `hosts/Host.tsx`: A host: owner, capacity and storages, what its units declare they need of it, what it does when full, its load over time from stored samples, what runs on it from both pools, its page with the limits each entry carries, and enrollment.
- `hosts/KeptLayers.tsx`: A host shows each environment's kept installed-file layers, including archived environments with no unit.
- `hosts/AddHost.tsx`: The dialog that enrolls a host: whose it will be and who may use it, then the single-use command for each system, and a wait that says when the host has connected.
- `hosts/HostList.tsx`: The hosts a person may use or manage, the system's among them: whose each is, which drivers it has, how much of its memory the environments there declare they need, what runs there and whether it takes new environments; with the means to enroll one.
- `hosts/Runtimes.tsx`: What runs on a host, from both pools and whoever's it is: each unit with the environment or provider it serves, where it stands and when it was last busy; one is stopped, leaving or interrupting what runs in it, or removed after naming it, with its volumes only when asked.
- `hosts/words.ts`: What a host's figures say: how much of what it has the environments there declare they need, how much of a storage is used, and why a new environment cannot go there.
- `hosts/acts.tsx`: What can be done with a host, from its row in the list and from its own page: pause it or resume it, open, edit, rename, copy its link and delete, each only where the reader may.
- `hosts/Edit.tsx`: Quick Rename only. Header/list Edit enter the canonical Host Settings rows, with accepted-response Saved/Undo, guarded drafts and no duplicate four-setting dialog.
- `hosts/Remove.tsx`: The dialog that deletes a host: it is named to be sure, and says what the server will refuse, a host that something still runs on or that holds a volume of a mount.
- `mcp/Preset.tsx`: An MCP server's page: a preset's configuration, secrets and sharing; for an imported one, where it was found, the machine that keeps its definition, Import again and Forget; and where it is on.
- `TechnicalValue.tsx`: Ordinary paths and executable facts show short names with exact focus/copy values. Runtime definition fingerprints and structured before/after values stay in lazy Details, shared by resource pages and update plans; actual ordered argv is retained.
- Resource identity cells have a twelve-character minimum and contained intrinsic width; long resource/location/status values truncate with native focus/hover disclosure. An empty Where cell uses a short, privacy-correct state, leaving its full explanation on demand. A unit-backed provider names its host, never its connector's container id.
- Activity's header reports the current session-history readers, using disabled observers of their existing query keys; it does not equate model access with a successful archive read. Environment create/recreate/move/removal owns its displayed lifecycle ahead of stale physical or update observations. The admitted Host page reply joins all loaded host projections through the existing cache writer after read admission; it starts no extra request. Temporary consent distinguishes the idle/thread lifetime from its warning interval.
- `SandboxReadiness.tsx`: The native access-level comparison and authorized recovery words. A machine has one neutral/warning line per environment outside its tabs; dismissal lasts through tab changes and renewed observations until the boundary itself changes. Full explanations and permitted policy links are on demand.
- `mcp/Import.tsx`: The dialog that imports a server from a harness's own configuration: what Orch keeps and where, that agents there will act with it as the machine's user, and switching it on for the environment.
- `mounts/Mount.tsx`: Mount overview, where it appears, whether its data may be written, used-by and sharing.
- `mounts/Locations.tsx`: A mount's locations and their visibility/offline reasons; common machine management and served data-making/storage choices govern add/remove on every machine.
- `mounts/Here.tsx`: One machine/host's readable mount locations, reported sizes, attachments and guarded add/remove forms.
- `mounts/Where.tsx`: Readable location links with offline state, plus shared explanations for concealed, unconfigured and unreported locations.

Environment removal states the resolved/frozen retention window and treats connected directories as registrations. The host Overview adds safe kept-data/failure rows under host.manage and offers no early purge there.
- `providers/entry.ts`: What a model's entry at its provider says it can be run with, as a form: efforts and the one a thread starts with, images, the speeds and access programs switched on and the one each starts with, the context, groups and rate; with the rules the server keeps to it, read at the field before anything is sent.
- `providers/ModelEditor.tsx`: The row editor of a model, headed "What this model can be run with here": its efforts and the one a thread starts with, images, and the speeds, access programs and context that are off until a manager turns each on, with what the source offered and what a thread starts with; its groups, the limits that cover it, and its rate.
- `providers/Groups.tsx`: The groups of a provider's models: chips on a model's row editor, added by typing with the provider's other groups offered, and the strip above the list with how many models each group has and "Limit this group…".
- `providers/capEntry.ts`: Where a provider's Models tab hands over to the form that makes a cap: "Add a limit…" on a model and "Limit this group…" on a group open it with the provider and what it covers filled in.
- `providers/ProviderUsageCard.tsx`: Independent member states, exact balance/spend text and observation times on the page and in compact provider choices; unsupported members share one Not reported line.
- `providers/details.ts`: Canonical state/source accessors and lossless decimal display for provider details.
- `providers/SourceFields.tsx`: Registered program and address choices, write-only authorization, HTTP policy and exact-recipient key consent.
- `providers/Sources.tsx`: Manager-scoped source offers, the revisioned sources PUT, learned descriptors, retained drafts and source/network policy guidance.
- `providers/sourceOptions.ts`: Served default-host metadata and keeper/template program scope, projected into the common source choice body with secrets separate from public configuration.
- `providers/PlanPanel.tsx`: One native observation card with a permitted refresh and optional provider-owned reset control.
- `providers/Reset.tsx`: Exact vendor reset review with Cancel-focused keys and one retained request identity.
- `providers/Ledger.tsx`: Bounded provider ledger summaries and applicable cap observations for provider and thread Usage.
- `providers/ledger.ts`: Exact ledger addition and UTC daily period selection without vendor counters.
- `providers/plan.ts`: Native observation times, reset review eligibility and UTC ledger periods.
- `providers/RuntimeConfiguration.tsx`: Shared provider unit configuration, write-only secret values and a reviewed replacement plan.
- `providers/Home.tsx`: A provider's one made-by-Orch or existing-directory harness-home choice, using its physical machine's roots and the common setting revision guard.
- `environments/DataSources.tsx`: One workspace/home chooser driven by driver facts and machine roots; host and personal machine pages share the allowed-directory usage section.

- `Library.tsx`: A resource’s giving and reported use share one shape on its row and page.
