# Shared UI

`DetailCard` owns inspection by hover, focus and a kept click for people and resource names.
`ChoiceCard` uses the same behavior beside highlighted options without committing them. The
`inspection.ts` focus-boundary helper keeps those portalled cards inside their owning choice popup;
Escape returns to the original control. Card content is created only while visible.
`PersonCard` uses a compact quick card for hover and focus, and the kept profile for a click or
Enter; a touch profile puts the quick acts first. A person choice keeps selection on its row and
uses the shared inspect control for the profile. When a side placement cannot fit, its card stays
outside the list and follows the list's position on resize.

`Popover` also accepts `anchor` for a region whose context menu opens its card. It uses the
same content, bounds, dismissal and focus callbacks as an ordinary trigger.

The shared controls of the design system. Radix primitives are wrapped here, once, and features
use these controls and never the primitives. Colors, typography and geometry come from the shared
tokens, so every control works in both themes, both densities and both languages.

`ThreadStateIcon` and `ThreadStateChip` render the server's reader-specific `ThreadDisplay`;
callers do not infer it from execution state. `LiveThreadStateIcon` reads already-held thread
and tree facts without acquiring a feed. Missing display facts keep the plain thread glyph,
and place marks aggregate the same authorized states through `ContainedThreadStateIcon`.

Features that use SectionNav supply one controlled current section. Its data-current-section also
lets the layout audit compare the actual current link against that section, including compact presentations.

`ActionTrigger` composes an action onto an existing control without changing its styles, DOM or
keyboard behavior. It respects a child's cancelled click and prevents another action while pending;
features use it instead of importing a slot primitive themselves.

An `IconButton` with an explicit `aria-describedby` keeps that complete value or state description
when its name tooltip opens; the tooltip does not repeat its accessible name as a description.
A distinct `reason` still joins the value, so a blocked control explains the refusal. The native
button applies this after tooltip composition; refs, events and focus retain their normal behavior.

Primary navigation stays visible. `Tabs` and the native links/specialized tabs in `FoldingStrip`
wrap onto further lines; arrows traverse the whole set, and Enter follows a focused native link.
`ViewGroup` keeps one horizontally scrolling strip at the ordinary tab height, so long tab sets leave
the view usable. Its existing Palette locator searches titles and authorized paths, with dirty marks
and an addressed Close action. Workbench containers keep their icons and full names visible,
wrapping controls and labels as necessary.
Tabs isolate each ordinary panel. A section layout that places its own boundaries around content
can set its tab definition's `panelBoundary={false}` so the section navigation stays outside them;
ResourceTabs does this for DetailSections, whose visited drafts still keep their Activity lifetime.
Breadcrumbs wrap full ancestor names, hide on a root
page and use only the nearest parent below 600 pixels; each target is at least 24 pixels.
Workbench sheets can delegate their title/actions to the contained bar without a second visible title.
The focused Views palette uses `showTitle` and a distinct `inputLabel`: one visible title, a name and
trailing group per row, without a generic group heading or description cards. Layout presets and the
secondary all-tabs list keep names only; dirty marks and applicable Close controls remain.
A page owns its section navigation inside its main content. Hiding global navigation does not remove
page choices or discard the active panel and its draft.
Layout Drawers use `prompt={false}`, `modal={false}` and `persistent` so their named chrome remains
reachable. An optional `style` reserves its measured height; prompt-layer styles still take precedence.

**The control's role determines its presentation.** Forms and settings show the supplied
`description` or `hint` below the name. `Choice` and `Checkbox` default to `presentation="form"`;
`RadioGroup` offers explained form options. Toolbar callers use `SegmentedControl`, omit switch
captions, or explicitly choose `presentation="compact"`. Supplying a form description must never
silently discard it. Form and picker descriptions are associated through `aria-describedby`.

Open pickers keep descriptions inside their option, after its name. Their closed trigger keeps
only the name. Person, group and place choices use one searchable locator. Any collection that
can grow beyond ten choices needs a workflow suited to finding and comparing items, rather than
an unsearchable dropdown. Small fixed enumerations can use `Select`.

`Switch` and `Checkbox` accept an independent hover/focus `tooltip`; it supplements their visible
caption. `ChoiceField` provides one field-level explanation when needed. It does not replace the
facts a person needs to decide between form options. Guides stay outside the selectable label.

Radix tooltip triggers and toggle/radio primitives share `data-state`. The controlled checked
state is passed explicitly on those roots so opening a tooltip cannot replace checked styling.
Keep native `aria-checked`, value and arrow navigation, including disabled-option skipping.

The three control roles have distinct styling. Primary buttons use the indigo accent. A switch
has a neutral off track, its own green `--switch-on` track when on, and a white thumb, with no box
around its label. Segmented controls have a light neutral track and raised selected surface with
a shadow. Checkbox fills retain `--control-selected` and `--control-on-selected`. List selection
uses a light background and readable text; menu radios show a check. Focus and forced-colour
states remain visible in both themes. `block` fills a segmented group's available width.

`SwatchGroup {label,value,onValueChange,options,disabled?}` is the colour palette. Each option has
`{value,name,sample}`: the name is accessible, the sample stays uncovered, and the selected sample
has an outer ring separated by a gap. The visible group label appears once. Arrow keys select as
in a segmented control; keyboard focus has its own outline. Profile uses the shared identity palette
and writes its existing numeric colour index; resetting still asks the server for the original colour.

Management embeds the ordinary paged resource lists with
`ResourceListScope {scope:"system",embedded:true,children}`. The shared query retains system scope
through search, filters and pagination; the enclosing settings frame owns the heading, navigation
and scrolling. The list keeps its native actions and returned permissions. System notification
channels reuse the org notifier forms with the system owner and create endpoint; secret values
remain write-only and acknowledged channel rows update only their own scope's cache.
The defaults grid accepts a real system target and uses the system defaults read, preview and
revision-checked change. Its implicit roles apply separately in following places; they are named
that way and never treated as one system membership or sent to a node lookup.

Options use content-based sizing: the options and words use `flex-auto`, names use
`break-words`, and a segmented group has `data-segmented` and one neutral track. Do not restore
zero-basis `flex-1` words or character wrapping. The boundary selector uses `:where()` so it cannot
outweigh the selected ground. A checked menu radio shows its check; only keyboard/pointer highlight
shades its row. Component stylesheet cases guard these states; the browser layout auditor measures
the longest word independently of wrapping and selected/highlighted text at 4.5:1 in both themes.

`AsAdminMark` is the one quiet mark for an object returned only through administrator reach. Pass
the server's `admin_access.place` as `of`; absence draws nothing. `iconOnly` is for a tree, `responsive`
keeps only its shield on a narrow table, and `sentence` names the reach in a page header. A private
page also passes `private`. Never derive this mark from a role, permissions, the switch or ownership.
`Button.admin` and `ConfirmDialog.admin` take `{access,permission,of?}`: only the returned `admin_only`
permission adds context, in the existing tooltip or confirmation. No confirmation is added to an
immediate act. `adminSpec(object,permission)` binds that permission to an object's returned access.

**Settings use one compact row.** Use `SettingRow` for a resource or place value: `name`,
`value` (its actual control or read-only value), optional `description`, `reason`, `actions`, `status`
and `details`. A writable row shows its name and one description line at the left and its compact
control and source at the right. Setting descriptions explain the value beside its editor; individual
options inside that control retain compact names. `help` is only supplementary information (bounds, merge behavior or a useful
example); it must not repeat the visible description. A read-only row keeps the supplied description, effective value and short linked source, without a disabled editor. Its collection renders
`ReadOnlySettingsNotice` once, with the actual known contact, rather than a refusal per row.
Callers retain the server's permission, revision and validation rules. Never put a value table above
its editor. Long collections use `SettingsEditor`'s sticky group contents or its narrow group picker.

For a long or structured value, `SettingEditor` uses the same row with `summary`, an accessible
`label`, and its editor as `children`. Optional `open`/`onOpenChange` control the disclosure. It
mounts only when first opened, keeps unfinished input when closed, pauses hidden effects, and returns
focus on Escape or a completed save. `disabled` means a pending act and retains keyboard focus.
An editor whose cleanup changes validation can set `pauseEffects={false}` and explicitly gate its
queries/timers with the open state (`TemplateEditor.active` does this). Cancel must reset the draft,
including raw fields which have not parsed yet. `SettingRow.detailsVisible` lets this disclosure
hide its full-width area without discarding its children.

Registry rows use `placement="inline"`: a plain summary and named pencil open the one own field in
the value column and focus its first editable control. Escape restores the pencil and current summary,
including after reopening. No Empty button, contribution cards or second resolved-value editor is mounted.
The native default is part of the control (“Default: 8”, or “Default: none” for unset text); zero and
false remain real values. Details can still show other inherited contributions on request. Other
atomic resource forms retain the default `placement="details"` and their existing draft lifetime.

Registry values use `features/settings/SettingRow`, which composes this frame with the served schema,
own contribution and source links. Discrete values save on change; free and compound input saves on
valid form blur, Enter or Ctrl/Cmd+Enter. It shows quiet Saved and Undo, preserving the accepted own
revision for that inverse. Reset is in the gear and removes just the local key with the reviewed
revision; reviewed absence is revision zero. Invalid input and refused/conflicting drafts stay put.
Node and system rows expose “Where it applies” directly beside their source. Opening it reads native
`SettingReach` once and shows counts and linked overrides together on that row. More reads another
cursor; Refresh starts with one first page. A changed native value hides the previous facts until
opened again, without a repair GET. No descendant, permission or count is inferred from the tree.
Source links use existing org/tree/detail metadata and resolve a missing org address only on opening.
They never invent a URL from a name or reveal a redacted source. For a competing resource write,
`SettingChanges` shows current values beside the draft; adopting its new revision requires the
person's explicit choice. Unedited fields then take the server value, while edited fields stay.
Read-only structures use `SettingSummary`/`ValueSummary` with readable values and choice chips, never
a JSON dump. Pass `inControl` when a summary sits inside an editor trigger, so it cannot nest another
button or block list. Existing atomic resource forms keep their guarded whole-form Save and explicit
conflict review; the native resource setting adapter owns its one-request resolved-setting answer.

`ContentBoundary` isolates a rendered part with local Retry; it is used by workbench view homes,
tab panels and the shared dialog/drawer/palette/popover bodies. A failed confirmation withdraws its
confirm act until review renders again, keeping Retry and Cancel. Hosts that construct a dialog in
their own component use `DialogBoundary {title,onClose,resetKey?,children}` around that component
(resource creation does this), because a boundary inside a body cannot catch its caller's hooks.
Neither boundary replaces neighbouring views or drafts. Runtime translations fall back to English,
then the key, with development diagnostics only; catalogue/source audits remain strict.

For example, the same bilingual copy serves explained form options and an open picker:

```tsx
const copy = choiceCopy(t, 'machineRole.environment');
const environment = {
  value: 'environment',
  ...copy, // Environment; Let people work in folders of this machine.
};
<RadioGroup label={rolesLabel} value={role} onValueChange={setRole} options={[environment]} />;
<Select label={rolesLabel} value={role} onValueChange={setRole} options={[environment]} />;
// The field question supplements the visible form descriptions:
<ChoiceField label={approvalLabel} options={offeredApprovalModes}>
  <RadioGroup label={approvalLabel} value={mode} onValueChange={setMode}
    options={offeredApprovalModes} />
</ChoiceField>;
```

`Choice` is also the composition point for custom controls: its render child receives
`(content, control)`, where `content` contains the name and the description for the requested presentation
and `control` contains the matching `aria-labelledby`/`aria-describedby`. Put both on the selectable
element. An open picker can retain existing guide metadata: `{label,onOpen,guide:{article,heading}}`
with a localized guide name and its actual destination (`heading:null` for a whole article). The
guide registry coalesces the same destination with the page/section's own entrance. Custom
`renderHelp` spreads its third argument on the actual help item; the guide stays outside the option
or checkbox label and never changes its value. Menus preserve roving keys. A Select with option guides has a dialog popup containing the
ordinary listbox and the highlighted choice's separate guide; Tab reaches its `?`, Shift+Tab returns
to the option, and F1 opens that guide. Disabled options' guides also remain in this guide area,
without making those values selectable. No button is nested inside an option or checkbox label.
Ordinary Menu and ContextMenu entries render names and popup descriptions without guide buttons;
the workbench's single global guide entry is the rail's Help link.
Existing `label` options and Checkbox `hint`/help slots stay compatible during conversion. The raw
Checkbox's `name` is still its native form name: use `CheckboxList` options for named choices, or
`Checkbox label={copy.name} description={copy.description}` for an individual choice.
`Switch` accepts canonical `name` and keeps `label`/`hint`/description metadata compatible;
supplied hints and guides render in forms. Its state remains native `aria-checked`. As admin is a
single direct switch in the title bar, shown only by the server's availability, with its full
Off/On name accessible and icon plus state below 600px; there is no mirror or second popover switch.
An actual open multi-selection list may pass `presentation="picker"` to `Checkbox` or `CheckboxList`:
its familiar name remains `aria-labelledby`, followed by the option description through
`aria-describedby`. TagPicker uses this for the defining place. Toolbar checkboxes explicitly use compact presentation; ordinary page and popup forms retain descriptions.

`Table` sizes fact and action columns to their content, without breaking a fact merely to give the
name more room. One identity column takes the surplus: `Column.grow: true`, otherwise `name`,
otherwise the first eligible column; `grow: false` excludes a leading selection column. The shared
auto layout applies to headers and cells. Do not impose fixed table layout or arbitrary maximum
widths on these values. Existing `from` thresholds and the cell's `shown` set keep narrow summaries
in the identity, once per fact. Genuinely wide data stays in the table's own keyboard scroller,
never makes the page wider. Column metadata adds no second copy of any text and no data request.

Use `ScrollRegion label={name} kind="code"|"table"` for a custom scroller; shared Table and DiffView
already use it. A scroller is a focusable named region with the inset focus ring. CodeBlock is also
a keyboard region. Long code and inspector JSON use the shared six-line `Preview`/Show all pattern,
growing the page on expansion without a nested vertical scroller; copying code always copies the
whole value. `JsonTree preview` opts an inspector into that preview. Its branch toggles have 24px
targets, names such as “Expand Configuration.limits”, and an explicit expanded state.

`ScrollRegion.visibleScrollbars` keeps Radix tracks visible whenever a wide table overflows, including
on devices with native overlay scrollbars. Everything uses this option; its narrow cards stay in the
page's scroll flow. Other callers retain the native scrolling region.

`RowDetails` shares compact list facts: `RowPlace` shows the nearest readable parent only when at least
six character widths fit, with the authorized full path in its tooltip. `RowPath` truncates the start
of a wide table path. `RowEnd` lets desktop row actions replace the whole time on hover, focus or
selection; phones keep time and actions in separate slots. `RelativeTime compact` uses the same local
calendar boundaries as Recent groups and always keeps its exact tooltip. `Tree flat` removes only the
hierarchy gutter and expander; ordinary trees retain their levels and keyboard behavior.
Name-and-place links use `data-row-name-place`: the name takes its intrinsic width, then the
place uses the remainder. A hidden place reserves no gap before the name can truncate.

`EmptyState compact` uses the inline `StateLine` with a title and muted explanation on separate lines.
Workbench bar sections supply its shared inner gutter.

`FilterBar` is the one toolbar: `search`, `participation`, filter children, then `action`
at the right. Its controls use the compact height, including ordinary fields and buttons, so a
select and its adjacent act align. Resource creation belongs once in the page header, with
search and compact Filter/Sort popovers in this bar. `wrap={false}` gives search the remaining
row width beside those short triggers; criteria and order stay in their parent, so dismissing a
popover loses nothing. Other screen-specific filter acts can use its action slot.
The existing `shell/Page` `PageHeader` remains the shared page heading,
with optional mark and compact actions aligned to its first title line. Word `Badge`, labelled
`StatusDot` and `StateChip` all use the same `StatusChip`; numeric badges remain small counters.

`SectionNav` gives real router links the same selected and focused treatment in every settings scope.
Supply `current` and `items` with their `link`, `label`, optional `icon` and grouping. The list stays
vertical when its page container has room, and keeps its choices inside that page on narrow screens.
Global navigation is never replaced by page sections. Never use an overflow menu.
`features/settings/Frame` supplies the common page and navigation geometry, preserving native admin
provenance. Its page heading labels a reused Section of the same name, so the title is said once.

`SaveStatus` shows quiet Saving/Saved/Undone and Undo. Registry rows keep their native revision/CAS
logic. Bespoke settings forms use `features/settings/AutoSave`: discrete choices write on change;
text waits for the shared typing pause or form blur. Inputs remain editable during their write;
a newer draft queues against that accepted answer. Invalid/IME/refused/conflicting drafts stay put,
and Undo uses the last confirmed snapshot, never a refreshed revision guessed for an old edit.

`DateField` takes `value` and `onValueChange`, field labels/errors and optional `min`/`max`. It uses
an explicit year-month-day text format and a localized calendar, with arrows, Home/End, PageUp/Down,
Enter and Escape. Invalid partial dates stay in the draft and cannot submit a form. Optional dates
can be cleared. `type="datetime-local"` writes the same local `YYYY-MM-DDTHH:mm` value as before,
shown with a space rather than a routing-style separator; callers retain their time-zone conversion.
Do not introduce browser-native date fields alongside it. Focus and bounds remain effective if a
field becomes read-only while its calendar is open.

`PromptScope` coordinates the existing Radix prompts: only the newest is visible; covered drafts stay in React Activity, with their
effects released. A nested ancestor retains its child's portal ownership. A workbench Drawer passes `prompt={false}` to keep layout
separate from a form and forwards the containing form's ownership. Shared cards/lists hide while covered and interactive cards retire
on return, preserving a confirmation rendered inside them. Focused Help is a reading entry in that order, not a modal. Global commands
remain reachable; a newer form pauses reading and closing it restores the article. A loading presenter uses `usePrompt(open, 'scope')`
outside its data hooks and puts them in `PromptScope`; replacement Dialog roots inherit its original opening order. Reappearance cannot
bring an older form above a newer one. Toasts wait behind prompts and focused reading, and full-value tooltips forget a covered source.
`Tooltip` also accepts controlled `open`/`onOpenChange`, so the phone's touch-hold names share its
existing keyboard/pointer presentation, prompt ownership and dismissal rather than a separate overlay.
No sizing/choice API, native cancellation guard or z-index token is replaced. `help-layers` supplies source regressions; integration must
establish actual focus, geometry and accessibility on the stamped build.

Built so far, each with its states, its focus ring and its keyboard: `Button`, `IconButton`,
`Input` (with `Field`, the frame every form control stands in), `Checkbox`, `Switch`,
`SegmentedControl`, `Dialog`, `ConfirmDialog`, `Drawer`, `Popover`, `Menu`, `ContextMenu`,
`Tooltip`, `Toast`, `Breadcrumbs`, `Tree`, `Avatar`, `Badge`, `StateChip`, `StatusDot`, `Kbd`,
`CopyButton` (and `copyText`, the one way anything is copied: it says "Copied", or what the caller says was copied, or that the browser would not let the page), `RelativeTime`, `Skeleton`, `StateLine` (one compact status sentence and one next action), `EmptyState`, `ErrorState` (catalogued failure words and an explicit recovery workflow, with no raw codes), `InlineAlert`, `Banner`,
`Tabs`, `Textarea`, `Select`, `TagChip`, `UserChip`, `AvatarStack`, `Table` (a row can carry a menu of
acts: from the button at its end, under the right mouse button and with the menu key), `Pagination`,
`PermissionGate` (what a person may not do, switched off with what it takes, or left out), `EmojiPicker`
(the platform's own emoji, in groups and searched by name, with the keyboard moving through a grid that is
one tab stop; `ReactButton` opens it under a message or a comment), `Meter`
(how full something is, in one control and four looks: a bar with its figure written beside it, the
same in a cell of a table, a small ring for a header, and the bare bar of work under way; a warning
from 80 % and danger from 95 %), `EnvironmentDot` (how an environment stands, in one colour and one
set of words wherever it is drawn), `RadioGroup` (one choice whose options each say what
choosing them comes to, and why one cannot be chosen), `FieldForm` (a form drawn from the fields a
driver declares: text, secret, a file's text, choice, boolean, date, maps), the parts of the
workbench (`Workbench`, `ViewGroup`, `ViewTab`, `Sash`, `StatusBar`), and what a conversation is
drawn with: `VirtualList` (rows of any height drawn only where they are in view, every one of them
when they all fit, beginning at the end and holding it while rows are measured, which it says by being busy), `Markdown` (read into
elements, never into markup, with a caret while text arrives), `CodeBlock`, `AnsiText`, `DiffView`
(one column or two, the changed words marked), `JsonTree`, `PresenceStack` (who is looking: a few avatars and how many more, the names under the
pointer and read aloud), `SplitButton`,
`LockBadge`, `ByteSize`, `TimeOfDay`, `LimitPrompt`, `Palette` (one field with a list under it, over the page, which the finder is drawn with:
the list is the caller's and nothing in it is filtered or sorted by the control), `HelpHint` (the "?" beside a term, a choice or a
field, with its card: what it is, which one to choose, where the value in front of the reader is set; it
uses the same bounded, internally scrolling popover on a small screen, and a form control takes it in its `help` slot beside
its label), `HelpPanel` (the frame of the help view), `ProvenancePill` (where the value in force comes from: the
shipped default, a place above, this place, or a place above that locks it) and `QrCode` (a link to scan, drawn from
`lib/qr.ts` without a library); and, for the views of the workspace, `CodeView` (a small editor on CodeMirror:
highlighting, folding, find and replace, several cursors, lines that can be lit; fetched when first used) and
`Sparkline` (a small chart of one or two series with a limit drawn as a line); and `TimeSeriesChart`, which draws a
few lines over a span of time with uPlot (loaded when the first chart is shown), in the colours `--chart-1` to
`--chart-8` of the active theme, with a crosshair that the arrow keys also move, a legend of what each line is now
and the most it has been, and the same values as a table (`timeSeries.ts` lays the lines on one list of moments);
the words it says are the caller's. `Preview` shows six initial lines or rows and expands in the page
with Show all; `TextPreview` applies it to read-only setting text. These previews never own a scroller. A `Button`
can be `blocked`: it stays in the tab order, says why it does nothing, and a press only shows the
reason. A list that opens from a control inside a dialog stands above the dialog. A `Tree` is given its rows
already laid out and draws them in runs of a few dozen (where a run begins is decided by its first row
alone): a run none of whose rows changed is passed over as one, and one far out of view is left out of
the browser's layout and painting, so that a change to one row of two thousand costs about the drawing
of one run. A `Tree` can also be one of ticks: a row that carries `tick` has a box (ticked, empty, or half ticked for a row
whose rows below are some ticked and some not; `tickLocked` for one that may not be changed), and the tree asks `onTick`
when the row is pressed or has the space bar pressed on it. `/dev/ui`
shows them all in a development build, and an accessibility test asks axe about that page in both
themes. Its example words are loaded only with the gallery; the application catalogues carry no gallery namespace.

`NodeIcon` is the icon of a place wherever one is drawn: the glyph of its kind (an org, a project, a folder, a topic, each
its own; a thread and a document a quiet one) in the colour of the kind or in the one of the twelve somebody gave the
place, with a check in a topic that is done. The twelve are drawn as they are on the light grounds and mixed with white on
the dark ones, so that a glyph reads on every ground of both themes; the kind is said by the shape, never by the colour alone.

Built for the permission page: `Combobox` (a field that opens a list of groups of options to pick from, with the
keyboard and the combobox pattern for a screen reader), `HolderChip` (a role, a group or a place as a name and a
count, with its people fetched a page at a time when it is opened), `LockBadge` (a lock and where it was set), and
`HistoryLines` (what changed, by whom and when).

`TimeZonePicker` uses that same `Combobox` and `Choice`: this device, the zone in use and UTC come first,
then region groups. A city, region, current abbreviation or signed UTC offset narrows the list. Each
name has its current offset and local time beneath it. Arrow keys, Home, End and Enter reach every
result; Escape leaves the choice unchanged. The closed field keeps the chosen name apart from the search.

- `PersonCard.tsx`: A person's quick actions and kept profile, including inspection beside a choice.
- `AgentCard.tsx`: An agent's identity, its thread, environment and model, read only when its card opens.
- `ReferenceCard.tsx`: A reference chip's card on hover or focus: the thing and the part it names, with Open, Open beside and Copy reference; read only when it first opens.
- `AgentChip.tsx`: A known agent's name opens its own card wherever it stands outside another control.
- `AuthorChip.tsx`: A human or agent author uses the same identity card wherever its name is shown.
- `pressed.ts`: Whether a name stands alone or inside something pressed itself. A `UserChip` that stands alone opens the person's `PersonCard`, and an `AvatarStack` the list of everybody in it (its "+3" too); a choice can own an enclosing inspection card without nesting a button in its selection control.
- `actions.tsx`: Canonical names and signs for application acts, kept separate from values and states. */
- Table context content can be rendered lazily so the same live entry list serves its button and context menu. Toast actions accept their icon, and pending notices remain until their caller completes them. Selected segments retain an underline under forced colours.
