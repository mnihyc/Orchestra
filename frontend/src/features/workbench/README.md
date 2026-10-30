# The workbench

Hidden desktop side bars can open temporarily from their edge, rail or shortcut through
`SlideOutBar.tsx`, using the shared drawer for focus and Escape. A pin docks the bar; choosing a row or
leaving it dismisses the temporary view without changing Main's size. Tab pins live in layout format
4, remain first in their group, and survive bulk close, Reset and layout changes. Both drag sensors
use the same boundary-aware layout operation. Older layout formats migrate with their valid targets.

Each feature registers its working views. `shippedViews.tsx` registers independently targeted full
content pages and Navigation. One shell, router, query client and websocket serve the persistent
workspace. Every main tab is closable, including the initially opened page. Closing its presentation
never stops an agent or deletes its resource. Retained inspectors render their own unavailable or
incompatible-target state; focusing another page never silently removes them.

Side bars hold named containers of related views. A container draws its views as stacked sections
with live summaries, disclosure buttons, Hide and Move to menus, and borders between open sections.
When one section is visible, its body fills the container and its actions move to the bar's
title. The title carries that view, and its menu keeps Hide, Move to and all section visibility choices.
An optional view `add` component replaces the generic Add-view button in that title; Resources uses
it to create a resource. Multi-section containers put it in that view's section header. Other views
retain the common chooser, which also remains in the finder.
The activity bar selects the left container and toggles it when pressed again. An org icon opens
that org's home and Navigation from another page; only that home already in view toggles its panel.
The right switcher keeps each icon and full translated name visible, wrapping controls and long names
as needed. Medium layouts expose those named controls above their sheets. Phone layouts use one
row of icons: eight 44px targets fit at 360px with the strip's side padding. Every icon retains its
accessible name and shows it on focus, pointer rest or a touch hold; holding reads without opening.
The shown view is marked, and pressing it again closes its sheet. No overflow menu hides a choice.
Controls remain reachable while a sheet is open; its top follows the measured chrome height.
Only that left activity has an accent background; the current page uses a separate left accent bar.

Inline controls keep short descriptive names at ordinary control height, without captions, cards
or per-option guides. Browser/display launch names include their live state, such as Desktop · Open.
Help has one entry in the rail. Section headers, layout menus and the status bar do not repeat it.
The Views chooser shows each name with its container at the right, without description cards or a
generic Commands heading. Registered explanations remain searchable. Resource lists is distinct
from a thread's Resources view. Arrangements and all-tabs menus keep names only, with dirty marks
and Close where applicable. A current tab has no Show act; every main content page has Close.
Navigation's native place actions and section/layout choices share one menu trigger; the native
entries still mount lazily when that menu opens. A view may contribute that composition through `menu`.

A page tab uses its name, with the shortest useful parent qualifier when names collide. Its authorized path is available in its tooltip and menu. Breadcrumbs show only ancestors,
and disappear on a root page. Below 600 pixels the full nearest-parent name is the back link; wider
paths wrap rather than hiding ancestors in a menu. Navigation's Team chat shortcut opens the single
Team view, keeping the current page. A topic's Documents page has no duplicate Documents section.
Every settings scope, Help and Usage supply their section links inside their page frame. Global
Navigation retains the person's chosen organization tree and visibility. Place settings
share their page's canonical identity read. Topic Settings
uses an explicit address; plain topic links resume content, including when an older device saved Settings.
Exact setting links retain their section. Org home/settings share the same identifier-to-address
resolver, preserve search/hash on redirect and reuse the native Org record in the sidebar and page.
Usage's default link explicitly excludes other views when the router matches its search. Every
section list has one current item; Management while Off marks Administrator access, before its
sections are opened. Its native enable action stays in the page. Every bar's view-actions trigger
uses the same Actions identity and ellipsis sign, independently of its translated title.
Page-local section navigation stays available independently of the global Navigation view.
Layout toggles use short area names. Keyboard shortcuts open the existing wide searchable table,
with binding edits kept in its rows and the same refusal/retry behavior.

The layout format is version 3, migrating versions 1 and 2. One per-user workspace retains
main groups, sidebar and bottom panels, sizes, targets, inspector scope, focus and closed views.
Normal navigation activates a matching content tab or appends to the focused main group. Explicit
Open in new tab creates another instance; Open beside creates a full view in a right-hand group.
Stable tab IDs own mounted bodies. Duplicate threads share the existing user/thread composer draft;
reading windows, expanded items and scroll anchors belong to each instance. The focused content tab
owns the URL. Existing-tab focus replaces it; resource navigation pushes history. Back/Forward
preserves other tabs. Close-all uses the neutral workbench entrance and does not recreate a route.

Inspectors follow the focused compatible content or have an explicit fixed target. Focusing an
inspector leaves the content context alone. Detaching a sidebar panel fixes the resource it displays;
its menu can select another target or restore following. Fixed inspectors survive conversation close.

The Layout menu uses the existing split tree for equal 2×4 and 3×3 starting grids. Tabs are assigned
in spatial reading order, then strip order; overflow remains in the last cell. Explicit grid cells
remain empty drop targets when closed. Manual empty splits retain their normal collapse behavior.
Focus mode hides the surrounding chrome as a display override. Tab menus, the palette and the empty
workbench retain an exit. Phone presentation does not replace the saved desktop grid. Each body uses
its own container width for the existing compact controls.

Tab menus name the addressed tab or group explicitly. Layout/palette close commands name all main
tabs or all except the focused main tab. Reopen restores the last closed identity/target/recoverable
state. Ordinary drafts are retained; genuinely unsaved loss uses one consolidated Save/Discard/Cancel
flow. Failed saves keep affected content reachable. Conversation, Code and Review are explicit
arrangements. Reset restores the standard frame and consolidates existing main tabs without closing
resources or discarding drafts. There is no Keep open action: ordinary tabs already survive navigation.

Every offered contextual view is in the finder's Open view chooser and in Add a view. Choosing it reveals its
saved container and section and sends the keyboard there. Headers and tabs can be carried by pointer
or keyboard; their menus offer the same destinations. Main and bottom groups keep one scrolling tab
strip and a searchable tab locator with authorized paths, dirty marks and Close. This preserves usable
content height in a small grid cell even with many tabs. Page/dialog tabs and native navigation links
retain their existing wrapping presentation.

The frame's panels never scroll. ScrollArea makes each scrolling view a containing block, and
intentional inner code, table, diagram and choice-list scrolling declares its purpose. The screen
accessibility sweep rejects undeclared nested scrollers. Phone sheets draw the same containers and
sections; browser/display lists launch the existing viewers, and file targets survive sheet changes.
Read-only setting values and chart tables preview their first lines and expand in the page with Show all.
The Git status part appears only when the workspace reports an available repository.
Kept views announce protected openings only while a visible slot holds them. Cached reopenings get
a fresh nonce; same-commit moves retain it. Each tab has a separate valid opening nonce, independent of its local tab ID.
An unseen view renders no body. Once shown, Activity preserves its draft while hidden, removing
its effects and query/subscription observers. The shell primes its existing caches from one bootstrap
before mounting readers. Avatars never load other orgs' chats; Navigation waits for the addressed
place's authorized org before starting only that org's data. Code intent loads route chunks without
running their admission or data loaders. The shell stays mounted around the main area's skeleton.
The root shell's `ShellControls` owns Find and the single keyboard dispatcher before admission,
route code or bootstrap can finish. An opened picker can search while the page waits; org metadata
and personal keys still come from bootstrap or its existing cache. The same picker/input survives
the full frame mounting, and session identity changes retire it. Other commands join the same book.
The first keyboard stop skips to main; F6 and Shift+F6 traverse the visible areas. Terminal creation
travels in the tab/section title row, with one scope question beside it and expandable retained output.
An empty terminal shows one state: the native refusal, an opening failure, or the empty invitation.
The Environment container uses short native rows, with network, credential and MCP explanations on demand.

- `layout.ts`: Values and immutable changes for containers, sections, main groups, splits and bottom tabs; the versioned reader.
- `placement.ts`: Built-in starting containers and section order, also used for old layouts.
- `views.ts`, `containers.ts`: Named registrations, contextual offers, body, summary, actions, icon, title and help text.
- `choiceWords.ts`, `choiceHelp.ts`: Familiar names and popup descriptions, retaining extension contributions; layout menus have no separate guide entry.
- `Arrangements.tsx`: Shipped layouts, frame dimensions and the arrangement menu.
- `Board.tsx`: The desktop frame, tab groups, drop zones and stable portal homes shared with phone sheets.
- `ContainerSwitcher.tsx`, `Sections.tsx`: Fully named, wrapping controls and stacked, expanded, hidden and resized sections.
- `MoveView.ts`, `Tabs.ts`: Pointer/keyboard carrying, destination labels, and tab menus.
- `OpenViews.tsx`, `openView.ts`: Contextual chooser commands and one reveal/focus path.
- `viewOpening.tsx`: Visible-slot lifetimes and opening nonces for kept bodies and cached private reads.
- `viewIdentity.ts`, `actions.tsx`: Stable view identities and live actions carried into the active tab/section header.
- `persistence.ts`, `presentation.ts`, `store.ts`: Per-user workspace transitions, recoverable presentation and captured close plans.
- `PageView.tsx`, `pages.ts`, `pageContext.tsx`, `PageRouting.tsx`: Full typed route adapters and source-owned URL/history reconciliation.
- `panelScope.tsx`, `instance.tsx`: Independent view context and explicit inspector targets.
- `CloseViews.tsx`, `workspaceActions.tsx`: Consolidated loss protection and shared named layout/close commands.
- `Summaries.tsx`, `statusFacts.ts`, `StatusParts.tsx`: Existing-data summaries and thread connection, environment, workspace, isolation, near-limit and shell status.
- `TargetSections.tsx`: Browser/display launch lists; their viewers remain owned by those features.
- `Narrow.tsx`, `NarrowViews.tsx`: Width shapes and the same containers in sheets, with kept target-specific viewers.
- `NarrowChoices.tsx`, `OverlayChoices.tsx`, `chrome.ts`: Direct named container controls, arrow-key focus and sheet geometry below the visible chrome.
- `shippedViews.tsx`: Full content-page and Navigation registrations; closable legacy screen compatibility.

The following Chat sidebar changes with its addressed context. Full main-area chats have their own content address and tab lifecycle. The left Team chat shortcut
opens the same view and hands the caret to its composer when ready; it leaves the page and its draft in place.
An explicitly opened target keeps its own native permission checks on another screen. An admin-mode exit pauses
the old content while selecting the nearest ordinarily readable ancestor, without changing saved arrangements.

Unit coverage includes changes, migration, persistence, measured label compaction and status words.
Component cases check compact live target labels, visible navigation/options, kept-view opening requests, hidden updates, moves,
preserved drafts, route nonces, bootstrap hydration, Find during admission/bootstrap, persistent loading chrome, visible focus areas and travelling terminal actions. Browser coverage
includes pointer/keyboard access at 1280, 1440 and 1920 pixels and the minimum right-bar width, phone
sheets, wrapped page/tab strips at 400 and 768 pixels, a single phone icon row at 400/360 pixels,
focus/hold naming and medium-width visible names, org-home navigation/toggling, all-tab lists, saved version-1
choices, and the all-screen scrolling rule. These sources live
under `tests/unit` and `tests/e2e`. Named saved arrangements and general unsaved-text persistence are not implemented here.
