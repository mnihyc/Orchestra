# Hints and help

The help answers where the question arises, in a sentence or two, and leads further only when
asked. A hint is the "?" beside a term or a choice (`../../ui/HelpHint.tsx`, filled from the catalogue by
`Hint.tsx`); the help view is a view of the workbench like any other and a page of its own at `/help`. Articles
are the files of `docs/user/` and the provider interface pages of
`docs/development/extensions/providers/`, built into the bundle (`../../help/`); nothing is fetched from
another site, and no model is asked.

`screenWords.ts` resolves the most specific route description, including resource detail screens.
The shipped-content gate requires its sentence in both catalogues and a readable article for every
framed route. Help and Finder offer only readable articles; an absent or draft article address uses
the factual missing-address state with All articles, never an unfinished-product placeholder.
An authored English body is named as English without promising a future translation.

On its own page, Help supplies its sections to the visible Navigation view through `navigation.tsx`.
Each section has a real `/help?part=…` address. Closing or hiding Navigation restores the same page's
section tabs without replacing its active panel or draft. Article routes keep the matching section
selected from the guide metadata the visible page already reads; the sidebar starts no guide reader.

The help view has contextual screen help, search, articles, setting entries, tutorials, editable shortcuts and
the notes of this build. Build your own groups the five provider implementer pages, with generated
schema assets and namespaced relative links. Every registry entry has a shipped value, an authored example, its schema in human
words, scopes and merge behavior. Every built-in title and description has exact-language English and
Chinese text; the catalogue and content gates require both for each generated registry key. An entry opened from a control rereads its current source; hidden sources
have no link, and a refused refresh does not expose cached source metadata. Source links take the exact key
to the current Defaults editor; a thread source uses that same editor in a dialog. Help is a reading view. When asked from a
prompt, it temporarily replaces the work screen; React Activity retains the work and deep drafts while releasing their effects and
focus traps. Back to work or Escape resumes them. The Help view is never another Dialog, and a newer prompt hides its reading surface.
Translated article front matter assigns shared section ids; a question opens the same destination in either language. Question destinations are validated at build time; their links are ready before the article bundle arrives. The reading view loads prose when More is chosen.
The reading view participates in the same opening order. Finder, global commands and global form hosts remain live outside the paused
workbench, so they can interrupt reading; closing them restores the article's scroll and focus. Asking for Help again above a newer
form returns to that form's draft, with older forms still behind it. F1 uses this same opener, including on a narrow screen; following an
explicit work link or a navigation shortcut leaves the reading view normally.

Keyboard shortcuts use one searchable table here and in the wider Ctrl+K/Keyboard shortcuts sheet.
Names, current physical bindings and context are searchable locally; Change/Remove/Restore appear on
row hover or focus and remain visible on touch devices. Conflicts keep their native refusal, Escape
returns to Change, and leaving a half-recorded sequence cancels it. Cached personal state is saved
serially without dropping unknown fields; failed writes keep the accepted keys and offer Retry.

The shared PromptScope gives the newest form/confirmation/palette/sheet the visible prompt. Covered ancestors retain their child portal;
unrelated prompts pause without losing drafts. Workbench layout drawers explicitly are views, rather than competing forms. Older popovers,
pickers, context menus, people cards and tooltips cannot cover a newer prompt. Popover subtrees survive a child prompt and the stale card
closes on return. A presenter can retain a logical prompt scope across its loading and loaded forms, so a completed background read
cannot replace a newer prompt. Background toasts wait while a prompt or focused reading view is active; automatic full-value tooltips forget the covered source. Native Radix
focus, dismissal and cancellation policy remain in use; no z-index escalation or new network request is involved.

`Tutorial.tsx` is mounted once in the shell and keeps a step across screen navigation. It frames a visible
control, waits for the person or Next, and keeps progress/finished ids under the person's help state with
a device fallback. The four decision walks offer the reader's actual owned threads/environments and
labelled, noninteractive examples when no control qualifies. Every step has a destination and an example;
Next can finish the illustrated lesson without creating a request, port or grant. A Help view reserves
normal layout space for its player; related lessons follow the article in its scroll region. Help hides
its own contextual-screen tab and a lesson does not link back to the article already open. Guide buttons
use their distinct titles, once per article and anchor on the active screen. Repeated cards share the
entry, preferring the guide's named link, then a visible-control group, then the first individual hint.
The next entry takes its place when that entry is removed or its tab hides; a modal is its own
active surface. Sections offering several related guides use one `GuideRail` question with a short answer and the actual article links;
choosing an article closes that question. Environment Network/Settings and template configuration consume it. The common fake/native accessibility assertion rejects duplicate visible destinations
or accessible names. Named links, bare hints, setting hints and shared Choice guide callbacks
register their actual destination in `lib/guideEntries.ts`. A model picker has one question
listing only its visible settings, with a separate full-entry button for each and the actual
setting target retained. Other related setting questions share the panel's named guide; `GuideSettings`
keeps their full registry entries available after the guide's prose. Guide context contains only its
article and originating setting target, is cleared on another opening, and lets an entry reread its
source under current authority. Context explanations
with no guide destination keep their own cards.
The player changes no business object. `Spotlight.tsx` subscribes to movement/resize and ignores
hidden copies. The build's `help-content` gate and its regression tests reject a missing control, article,
heading, language text or screen, and a decision guide that no product control links to. The
provider pages also refuse a missing schema asset, duplicate article or unlinked interface page. The guide and
What's new remain build-local; no documentation site or model is contacted.

- `HelpView.tsx`: The help view: what there is for the screen in front, search, the articles, the tutorials, the shortcuts and what is new in this build.
- `Article.tsx`: One article of the user guide as it ships with the build: its headings, links that stay in the view, links that open a screen, and its address.
- `ProviderGuideLink.tsx`: Opens a provider/user/interface guide from a real control without leaving the work. */
- `GuideLink.tsx`: A control, card or panel opens its version-matched guide beside the work. */
- `ArticleGuideLink.tsx`: A guide is named by its title and occupies one entry for its article and anchor on the active screen. */
- `guideEntries.ts`: Feature and shared-control guide entries use the same registry.
- `CreationPermissionLink.tsx`: The empty place's creation question opens its actual permission row, without changing the page. */
- `TemplateGuideLink.tsx`: Templates and their settings lead to the same first-template guide as Help and the finder. */
- `HelpOverlay.tsx`: A normal reading view replaces paused work and returns to its retained draft; the legacy module name creates no Help modal.
- `SettingEntry.tsx`: Every built-in setting has its own searchable help entry, including its schema, default, example and live source. */
- `SettingHint.tsx`: The same setting help link beside an editor row or an in-place control. */
- `SettingHelpGroup.tsx`: One question covers the settings actually present in a picker, with each full entry still reachable.
- `GuideSettings.tsx`: A decision guide keeps its related registry entries reachable after repeated questions are coalesced.
- `SourceJump.tsx`: A setting whose source is a thread opens that thread's exact existing setting editor. */
- `WhatsNew.tsx`: Release notes are a part of this application's bundle, with links into the same build's help. */
- `settings.ts`: Build-local setting entries: the same keys, schemas and examples the server's registry serves. */
- `tutorialState.ts`: One active walk survives screen/view navigation; transient choices and Help docks are not persisted. */
- `TutorialPlaces.tsx`: A tutorial opens a reader-chosen owned place; enumeration starts only for a step that uses it. */
- `TutorialExample.tsx`: Noninteractive cards explain the same controls when a reader has no qualifying work yet. */
- `Hint.tsx`: A hint beside a term, a choice or a field, made from the catalogue by its id.
- `hints.ts`: A hint by its id, in up to four parts: what this is, which one for me, where it is set, and more; made from the catalogue and the registry for every row and every setting, and from a descriptor's own text for what an extension declares.
- `Tip.tsx`: A pointer shown unasked the first time something is used, which the reader can close for good.
- `Dismissed.ts`: The tips a person closed for good and the tutorials they finished, which follow them.
- `KeyList.tsx`: The list of keys: what is bound at this moment, and the few keys that the controls themselves handle.
- `open.ts`: Opening the help: beside what the reader is doing, or as a screen of its own where there is no room beside.
- `state.ts`: What the help view shows at the moment: the article it was led to, the heading in it, and the words typed in its search.
- `mounted.ts`: The hints that are on the screen in view, so that the help can list them: a hint says that it is here while it is drawn, and that it is gone when it goes. */
- `register.ts`: Registers Help as a workbench view under its stable identity.
- `WhereSet.tsx`: Where the value in front of somebody comes from, as a link to the place that sets it and, where it is locked, who locked it.
- `Tutorial.tsx`: Short walks on the real screen. Only the person clicks, writes, grants or sends. */
- `GuideRail.tsx`: One section entrance with the distinct native guide links inside its short answer.

Reading-view regressions are component `help-layers` and `guide-rail`, `screens-help`, and browser `start-help-switch`.
They cover deep draft/focus retention, nested confirmations (including inside a card and through a layout sheet), Help asked on Help,
global interruptions and resumed article scroll/focus, delayed presenters staying behind newer prompts, older prompts/popovers/toasts,
actual article destinations/source targets, 1440/400 and both languages/themes. These sources require
the component suite, browser suite and a walkthrough of the production application for validation.
