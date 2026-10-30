# Accessibility validation

The shared screen sweep also calls `navigation.ts`: every visible section list marks exactly one
current destination matching its controlled section, including the compact phone presentation.
Covered/inert lists wait until shown. `e2e/workbench-navigation.spec.ts` supplies real-route and
negative auditor fixtures; the integrator executes these sources.

- `trace.spec.ts`: Keyboard phone filters, retained content and explicit confirmations with axe in both themes.
- `admin.spec.ts`: Management with axe in both themes: each section, the refusal, and the dialogs that make a person, a link, a provider and a capability.
- `change.spec.ts`: The change wizard and side threads with axe in both themes: each step, its blockers, its progress and its end.

Every screen that is built, asked by axe in both themes whether anything on it is a serious
barrier, with what opens above the page asked while it is open (the screens that hold or name
folders among them: a project with its folder cards, a folder, a topic that lies in folders, the
dialogs that make and move one, on a small screen and in Chinese); and the workbench worked by
keyboard alone. The states of people's chats are asked in the same way: a conversation with its
menus, emoji and people to mention, the Messages screen and the dialogs of a new message and a
group, a selection and the dialog that forwards it, a guest's view, an admin reading, a small
screen, and Chinese. The resources screens are asked the same way: each list with its row of kinds, the act at the
end of a row and the menu it opens, rows ticked together, the Edit dialog of each kind, a hint open as a card and as
a sheet, the dialog that enrolls a machine again, and the lists and pages on a phone. The finder (empty, with results of
every group, with nothing found, as the card of a person), the search page with its results, a picker and a hint's card,
the bar that finds in a conversation, and the help beside a screen, in its tabs and as a page, are asked in both themes.

The pages of settings are among them: a person's own, each section of an org's, a project's and a
topic's, with their dialogs open, for somebody who may change everything and for somebody who may
only read.

What scrolls takes the keyboard, in both themes: the menu of a place opened by the pointer and the list of keys asked from the tree, in a window that is not tall, are a tab stop while they scroll and are named for what they hold, and what does not scroll is no stop.

Add checks with product behavior; do not assert file layout, import choices, component shape or
other implementation details.

The left bar in both themes: as it comes and with everything a row can carry (unread threads, a chat that
is unread, a mention, a quiet chat, a topic that is done, documents, folded sections with what waits in
them), with chats in the tree, with the tree's own menu, a place's menu and the dialog of an icon's colour
open, narrowed to a place with the line of a thing outside it, with the places that stay while it scrolls,
as the Resources view, and in the drawer of a phone; and each of the twelve colours of an icon measured
against every ground an icon stands on, in both themes.

The permission page in both themes: the short and the full form, a place's People and Defaults, what one person holds,
what a change reaches, the refusal with its request, the bypass, and an invitation to one place.

Giving in both themes: the short form of a place with what it covers and a hint open beside it, a reader's form with
its choices disabled, a place that keeps sharing to its owners, the Agents tab with its dialog by area and as a copy
and the confirmation of a change, a copy past the limit, a request waiting for its approver, the form for several
things, an environment's page with its rows by area and who reaches it, and a person from outside staged with the ticks
that go with naming them.

Taking things away in both themes: the dialog of a thread (with a turn that is none refused), the dialog of a place all ticked,
half ticked with all content/date options visible, past what the place allows and where it allows nothing, and with a row greyed and its things counted,
the menu of a thread that may be read and not exported with the form that asks for the right, the list of one's exports with
an archive of every state and on a phone, and the notice of a job that ended.

Starting work in both themes: the inline starter of a topic and of a project where the defaults are whole, where one is missing and a
choice was made with the way to keep it, of somebody who may only ask with the request open, and where no harness has reported; and the
dialog with the way that is not allowed, with the form of a new environment above it, where none may be made, and on a phone.
- `day100.spec.ts` — own-part fields, status review and tag definition in both themes, with Chinese phone layout and keyboard/forced-colour selection.
