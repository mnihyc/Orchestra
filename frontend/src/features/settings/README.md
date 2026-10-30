# Personal and hierarchical configuration

Every scope uses `Frame.tsx`: personal, org/department, project, topic and Management. Section links
stay in a fixed vertical list from a 600px viewport; below that one Select activates the same real
links. Every scope's sections replace the unrelated Navigation tree. `sidebarPresence.ts` follows
the existing native visible-view lifetime, so a hidden/closed/moved bar restores the frame's fallback
without guessing saved layout visibility. A scope heading labels a reused Section of the same name;
there is one title and no redundant Back link or common repeated subtitle. The legacy settings Usage
address redirects to the actual report.

Help and Usage reuse the same visible-sidebar ownership token for their section controls. Their route
metadata and native links live with those features; settings frame, save and Undo behavior is shared
without mounting an unrelated org tree.

`navigation.tsx` shares the real org/department, project and topic links with their page fallback.
The sidebar reuses each page's canonical native identity query and can own that read on a cold drawer
opening. Org destination aliases, flat-org sections, exact setting links and the topic's existing
remembered Settings tab select one current item. Settings pages hold no navigation-tree subscription.

Ordinary settings save on change: discrete choices write immediately, text uses the shared typing
pause or leaving its form. `AutoSave.tsx` retains raw invalid/IME/refused drafts, serializes newer text
against the accepted answer, and supplies `ui/SaveStatus` with quiet Saved/Undo. Place, notification
and system edits/Undo retain native revisions (including absence); an explicit conflict recovery alone
reads again. Personal name/appearance/profile endpoints have no revision member, so their inverses
compare current cached named fields and patch only changed fields. Creation, security proof and atomic
publication/review forms retain their explicit actions. Profile fields and pins share their existing
per-person mutation key. Forms are keyed by identity, never by the value/revision they just saved.
The accompanying component/browser sources are authored, unrun; passing/visual/request proof belongs
to the integrator's single round and current-world walkthrough.

`personal/RecoveryCodes.tsx` shows the backup sheet once, after authenticator activation or replacement. Plaintext lives only in an open dialog; reads expose metadata only. Replacement verifies fresh identity and a second step. After a saved code is used, its browser has one short-lived replacement allowance, so the last code does not strand its person. The act registry stores only the account and safe metadata.

Profile offers the shared twelve-colour avatar palette and a server-chosen reset. Removing an address
is an explicit null write with the current password where one exists; the screen explains the mailed
second-step and last-sign-in blockers and leaves final validation to the server. Setting links retain
their registry key and focus the actual row, including a second key opened on the same editor.

What a person sets for themself, and what an org, a project and a topic set for the work inside them.

A person's own pages (`/settings/...`) are real: the profile (name with the picture it makes, the address
changed with its confirmation, language, time zone, theme and density), security (the password, the
second step by an authenticator app with the code to scan and by a mailed code, linked sign-ins,
sessions ended), API tokens (made with permissions and a place, shown once, revoked) and what the
interface keeps (arrangements saved by name, the viewer each kind of file opens with, tutorials and
hints, and what this browser keeps) and Your exports (the export part's page, in the same frame:
`../export/YourExports.tsx`). Notifications shares that same frame. Its Usage entry goes to
`/usage`; the older `/settings/usage` address redirects to that same report.

An org's settings (`/org/$slug/settings/$section`) are the same at every level, a root org and a
department alike: General, Departments, Members (the list with domain, role and state; a member moved,
held out and let back, or removed with what that does said first; the invitations and the rules they
are made by), Groups, Roles and defaults (the permission page's own People, and its Defaults opened from the
section), Policies (the settings editor), Models, Tags, Service accounts, Audit (by who did it, the action and the date), Channels (a root org's ways out,
each from the fields its kind declares, tried with Test, with the far side's own answer when it
fails) and Send elsewhere (the destinations of a node, also on a project's settings and a topic's tab).
Usage is the usage part's screen and says so. A project has its general settings, the editor, its tags
and its destinations; a topic's Settings entry opens that same scope frame.

The settings editor draws every key of the registry that can be set at a place, by group, from the
schema the server serves: the value in force, a pill that says where it comes from, a way to set it
here and to take it away again (Reset to inherited), a lock for whoever makes the rules, the lock of a
place above, and the secrets of a place, whose names are listed and whose values are only written. Somebody who may
change none of it is told once, above the rows, what changing it takes. Its rows and the frames of the pages are laid
out by the width of their own box, so that the same page serves a wide window and the narrow pane of a topic.

Editing stays in that same value column. Complex values open beside their name using a named
pencil; the control edits only this scope's contribution. Default values are named in the control,
including a truthful unset default, instead of Empty actions or Default/Own/Resolved cards. The gear
retains Reset and optional inherited contributions. Native `writable` is decisive at every scope;
mixed collections say only some values are read-only, and a newly refused row drops its editor.
“Where it applies” opens native applicability and overrides together on the row, without a gear or
an initial request per key. A changed value hides old reach until opened again; no inferred counts
or automatic repair reads. The shared settings frame provides one gap between all direct sections.

What is not built yet says so on the page, as a route does that has no screen: the usage screen (`Usage`), the
providers' models overview (`Models`), and the editor of the text a channel sends (`Text…`, a button that says
why it does nothing yet). The pages of a person's own notifications and usage are those parts' own screens.

Tested against the fake API (`tests/doubles/settings.ts`): `tests/e2e/settings.spec.ts` for what a person can do,
`tests/accessibility/settings.spec.ts` for every page in both themes, `tests/unit/settings.test.ts` and
`tests/unit/qr.test.ts` for the rules.

- `Editor.tsx`: Registry-driven scope editor.
- `sectionLabels.ts`: One section name in settings navigation, headings and screen identities.
- `SettingRow.tsx`: Effective value, provenance, lock and apply semantics.
- `ResourceChoice.tsx`: Template and Host settings offer usable served resource names inside that same revisioned row. Read-only/current unavailable references do not print ids; disabled choices retain the current name, and automatic host selection writes null. Selected names share one stable detail cache; full option lists load only while the actual picker opens. Opening a choice never writes, and a refused/empty list leaves Retry or recovery reachable.
- `choiceCopy.ts`: Supplies the current setting key as context so nested schema enums use the same short name and separate explanation in both languages. `SettingValue` supplies it once; `ValueControl` keeps schema shape and offered values independent of business keys. Copy comes from `i18n/choices.ts`; unknown plugin values retain their declared term.
- `runtimeEffects.ts`: Change effects follow the current driver; memory warnings compare the draft to measured use.
- `ValueControl.tsx`: The control that edits a value, drawn from the shape of its schema: a switch, a whole number with its unit, one word of several, a text, a list, a map, an object, or JSON where nothing fits better.
- `settingsModel.ts`: What the settings editor knows of a key and its value without drawing anything: the group a key belongs to, which keys a place has, where the value in force comes from, and what kind of control its schema asks for.
- `Secrets.tsx`: The secrets of a place: their names and where each is set, written and replaced and removed, and never shown.
- `refusals.ts`: What a reader is told when they may not do something here: what it takes, on what, and whom to ask.
- `ScopeSettings.tsx`: The settings of a project and of a topic: the same sections an org has that apply to them, drawn from the same parts.
- `Destinations.tsx`: The destinations of a node: a form from the channel's fields, which kinds, the level, Test and the last result.
- `NotifierForm.tsx`: The form that makes or changes a channel or a destination from the fields its kind declares: its name, each field with its sentence, and what is written and never shown.
- `notifierFields.ts`: What a kind of channel asks for, turned into what is sent and back: which of its fields are secrets, the values of the others as a config, and what a form starts with.
- `ChannelText.tsx`: Text…: what a channel sends, for one notification and for several together, by language, with a preview and the shipped text one click away.
- `PersonalFrame.tsx`: The page every personal setting stands in: the row of the pages, the name of the one in view and what it is for.
- `PlacePicker.tsx`: Choosing one place of the orgs a person belongs to, by typing part of where it lies: "Acme / Research / Web".
- `TokenForm.tsx`: The form that makes an API token, for its person or for a service identity: narrow as it opens, its exact permissions shown and sent, a subtree and an end, and the maker's fresh proof in the same act.
- `personal/Profile.tsx`: Name, avatar, address with its state and a change that waits for its confirmation, language, timezone, theme and density.
- `personal/Security.tsx`: Password; the second step, by an authenticator app and, where the person's orgs allow it, by a code sent by mail; identities and sessions.
- `personal/Notifications.tsx`: Subscriptions, quiet hours, and Outside Orch: every way a person is told when they are not looking, each off until they switch it on.
- `personal/NotificationExceptions.tsx`: A person's differing manual place levels, with current-readable canonical names, native ancestor defaults, in-row changes, Undo, bounded pages and one-response updates.
- `personal/Routes.tsx`: A person's routes: one card for each channel they can be reached through, with its switch, how long a notification stays unread first, how often a message goes, the kinds it carries, and for a bot the link that connects their chat.
- `personal/Connect.tsx`: The dialog that connects a person's chat to a bot: a link, the same link as a QR code, and the wait for Start.
- `personal/Webhook.tsx`: A webhook of the person's own: its name, the address it posts to and an optional secret that signs what it posts.
- `personal/push.ts`: Push in this browser: whether it can, asking its leave, subscribing, and telling the server which browser takes it.
- `personal/Tokens.tsx`: Personal API-token scope and expiry.
- `personal/Usage.tsx`: One's own usage in three figures, leading to the usage screen, and one's share of every cap counted per person.
- `personal/Interface.tsx`: What the interface keeps for a person on every device: the viewer each kind of file opens with, the tutorials finished and the hints closed.
- `org/OrgSettings.tsx`: The settings of an org at any level: its sections in a column beside the one in view, the same at every level.
- `org/General.tsx`: Org identity and, for one that lies in another, its address and Move to…; its owners; deleting it, with where its people go and what becomes of what belongs to it; and what was recently deleted.
- `org/Departments.tsx`: The orgs that lie directly in this one, at whatever level: making one, its short name, how much each holds, and its own settings, which are these same sections; not shown for an org that holds none.
- `org/Members.tsx`: The members of an org with their domain and role and the acts on each, and the invitations still good: the Members tab of the org, and the Members section of its settings; an org that takes no members says so.
- `org/Outsiders.tsx`: From outside: the people who are no members of this tree and reach something of the org now, each way with who gave it and until when, and Take out everywhere here by a plan read whole.
- `org/Invite.tsx`: The rules an org gives its invitations, said beside the Invite button and not only inside its dialog, with where each comes from.
- `org/MoveMember.tsx`: The dialog that moves a member to another org of the tree: what they stop and start reaching, what of theirs stays, and the role they will hold there.
- `org/EndMembership.tsx`: The dialog that ends a membership: what that person owned alone and where each thing goes, what they owned with others, and what becomes of each resource.
- `org/Groups.tsx`: Groups and membership.
- `org/RolesAndDefaults.tsx`: Roles and defaults of an org: who holds a role in it and above it, and the defaults it gives to what is made inside it.
- `org/Policies.tsx`: Scope policy editor.
- `org/Tags.tsx`: Topic tags.
- `org/Channels.tsx`: A root org's own ways out of the application: its mail server, its bot, webhooks and any installed kind, each a form from the kind's fields with Test, the last result and Text….
- `org/ServiceAccounts.tsx`: Service identities and scoped tokens.
- `org/Audit.tsx`: The org's audit trail, with a bypass marked.

Org Audit, project Settings → Audit and topic Settings → Audit share the same URL-filtered trail. Members and permission-page People link to the selected place’s person-filtered trail under current audit.view.
- `SettingValue.tsx`: The common own-value control in the row; inheritance and optional contribution previews stay in the row's source/details.
- `ProviderChoice.tsx`: A provider default is chosen and read by its authorized name inside the existing settings row. The usable-provider picker loads its options on opening; its selected name uses a shared detail cache. The generic schema control remains independent. Selection uses the row's same immediate save, revision, lock, refusal, acknowledgement and Undo path. Read-only or unavailable references expose no raw identifier, examples are human samples rather than live UUID reads, and an empty list opens the existing provider creation workflow. Creation returns the choice without selecting or writing it automatically.
- `EmbeddingsChoice.tsx`: The embeddings of search by meaning, chosen in their setting row: the provider by its name among those the reader may use, and the model, length and prefixes as the registry's members.
- `settingParts.ts`: Separates stored own values from inherited parts without guessing or stripping duplicated text.
- `SettingLock.tsx`: Links the locking place and reads the actual holders of the permission which decides the key there.
- `OutsideOrch.tsx`: One section containing the applicable people and room/system lists under their ordinary guards.
- `settingText.ts`: Catalogue names, sentences and keywords, and size/zero formatting from declared metadata.
- `personal/away/`: Current scoped delivery status, revisioned Pause/Resume, retained failed routes, manager counts and this-device readiness using generated contracts; its README inventories the controls.

`CallsRules.tsx` uses the inherited setting row for compact failure conditions and ordered recovery
steps. Provider/model/profile candidates use the existing resource choices; edits keep raw drafts
until accepted, and ordinary settings revisions fence save and Undo. `calls.return` names the
next-turn return or continued use of a candidate. Thread and workflow recovery share one status
renderer in `../thread/CallRecovery.tsx`.
