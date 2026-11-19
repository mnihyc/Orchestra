# The finder

The dialog is named Find and its combobox Search terms, so the same title is not announced twice.
Its last row opens Advanced search with the current words, or the empty search page when no words were typed.
The short placeholder does not repeat the prefix buttons; the extra modes hint card is absent.
Every name group, including Recent and Starred, uses the person's display name and @username or
the readable place path. Routing addresses remain searchable and keep their served destinations,
but are never used as a row-detail fallback. Root places need no repeated namespace context.

The focused Views chooser keeps the same search and activation path, with a visible Views title,
names and trailing container names. It has no generic Commands heading or per-row description cards;
registered explanations remain search vocabulary. Resource lists and environment Resources are
distinct choices. The ordinary Find dialog keeps its existing groups and readable context.
The root shell registers its shortcut before a page or bootstrap finishes. The picker can open,
focus and search during Loading, retaining its field when the full frame appears. Its early reads
are the ordinary explicit searches; shell org metadata and saved keys remain bootstrap/cache reads.
One dispatcher serves early and later bindings, so the same shortcut still toggles only once.

- `TagProjects.tsx`: A tag opens its defining project or a readable project below the place that defines it.

Built: one field, opened by `Mod+K` from anywhere or by the magnifier in the rail, for everything that has
a name and everything that can be done. Names come from the server 150 ms after the last key (places, then
people, then resources, each asked for on its own so that none crowds another out); settings, commands and
help are what the page holds, matched as the letters are typed. A sign at the start of the field keeps it to
one kind of answer (`>` commands, `@` people, `:` settings and the rows of permission pages, `?` help, `/` what was
said), the buttons under the field write the same signs, and the groups are put in the order of how well they
answer. Enter opens what is chosen here, `Mod+Enter` in a new browser tab; the first row is chosen until the
reader walks. Named screens open the server's `href`, shared with Everything; names without a screen
retain their card or tag-project chooser. State and last activity distinguish repeated names. A full
remote group ends with a selectable link to Everything, keeping the words and participation focus.
Starred and Recent accept every kind, with a selected-name Star/Unstar action in the footer. What a screen offers to do is registered with
`useRegisterCommands` (see `commands.ts`), and a command that leads to a screen is listed only where the router
has the route. Covered by unit tests of the ranking, the groups and the registry, and by end-to-end tests of
every group and of what a person may not see. Name lookups require two characters except recents,
stop after a bounded access-check budget and explain partial/empty answers. Words match in any order,
quoted words stay together, and name tiers precede visits and activity. Native HTTP and browser
regression sources cover these boundaries; execution belongs to the integration rounds.
An open permission page registers its actual rows: a result discloses, marks and focuses the row after the
finder leaves. Setting results retain their key through the route and focus it in the common editor, also
when another key is chosen on the same screen. A person's card reads the ordinary privacy-filtered profile,
offers Message and pages the current direct node/document shares between the two people; choosing one opens
its Access dialog. The server cuts to both people's current View before paging, without hidden counts.
The arrow key that lists other scopes of a setting is still not built.

`>` runs acts as well as navigation: New thread/document, the current thread's native Fork/Fork to,
Switch provider or agent and Ask on the side, every addressed settings section, the session's opposite
As admin choice, appearance, Help articles and Keyboard shortcuts. Menu acts re-read their current
entry before running; a refusal stays visible with the menu's own reason. Settings use the same section
lists as their frames, including the org's single Outside entry and flat-org filtering. Thread settings
lead to its nearest named topic/project. Help articles use the existing Help opener and appear once in
ordinary search, or as acts in `>` mode. All matching acts are reachable without a thirty-row cap.

Creation delegates to the ordinary node handlers after checking the selected place; with no suitable
context it asks Where. It uses the existing authorized two-character, debounced name lookup and retains
partial-answer notices, without loading trees or names while hidden. Closing the chooser or changing
the signed-in person cancels a late read. An authorized New document uses the native immediate-create
flow, including its title caret, rather than introducing a second form. Native writes retain their own
idempotency and permissions; the palette prevents a repeated activation while it gives focus back.

The Keyboard shortcuts act opens the shared searchable table at up to 960 px. Search matches action,
current physical keys and context locally. Change/Remove/Restore appear on hover, keyboard focus or a
touch device; capture keeps existing conflict/refusal rules and Escape returns focus. Serial saves keep
unknown personal fields and bindings, use the cached current state, and commit keys only after success.
Failed saves retain the proposed keys and Retry. The Help view uses this same table.

Source regressions (not run by writers): `tests/unit/finder-acts.test.ts`, component
`palette-creation.test.tsx`/`shortcut-table.test.tsx`, browser `tests/e2e/finder-acts.spec.ts` and the existing
keys/finder programs. The browser source covers 1440/400, both themes/languages, native acts, one-write
document/fork/admin/theme behavior, theme persistence/refusal, prompt focus and touch actions.

- `Finder.tsx`: The finder: one field for everything that has a name, every setting, every row of a page, every command and the help; its groups, the prefixes that keep to one, and what Enter does in each.
- `Card.tsx`: The card of a person or a group found by name: who they are, how they are addressed, and where their page is, when there is one.
- `Marked.tsx`: A name with the letters that were typed marked in it, so that a reader sees why it was found. */
- `commands.ts`: The commands there are: what a screen offers to do, kept in one place so that the finder lists what can be done where the reader is.
- `shellCommands.tsx`: The commands that hold everywhere or follow the screen in view: going to a screen, making things, how the page looks, the workbench, and the acts on the place being looked at.
- `menuCommands.ts`: The current menu entries, refusals and handlers shared with the palette.
- `Creation.tsx`: Authorized place selection before the existing native creation handler.
- `settingsCommands.ts`: The actual section addresses shared with settings navigation.
- `sources.ts`: Where each group of the finder gets its rows: the names the server finds, what the page holds, and what the reader starred.
- `local.ts`: The groups the finder answers from what the page already holds: commands, settings, the rows of permission pages and help.
- `rows.ts`: The rows the finder lists and the groups they stand in: what a row says, what Enter does with it, and how the groups are put in order.
- `ranking.ts`: How the finder ranks what it holds itself: settings, rows of a page, commands and help, for the letters typed so far.
- `modes.ts`: The modes of the finder: a sign at the start of the field keeps it to one kind of answer.
- `destinations.ts`: Where a name leads, and how a result says what it is and where it lies.
- `store.ts`: Whether the finder is open, and what it was opened with: the one switch that the key, the rail's magnifier and the commands of other screens all press. */
- `pageRows.ts`: How the rows of a permission page are reached from the finder: the page in view says so, and the finder lists them only then.
