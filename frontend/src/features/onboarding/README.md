# First-run guidance

Only a team root's current owners read its setup projection. First steps appears after that read
proves an unfinished provider or team-members step; model, work and notification steps alone do
not keep it visible. Personal spaces never mount this card or its setup reader; their primary
action is New project and their native menu offers no New department. Chosen-solo team setup needs
no teammate. Departments,
non-owners, complete setup and unknown observations have no card or setup skeleton.
An org's count can include its owner; a count above one already proves a teammate exists and
prevents a stale invitation step from reopening the card.

The org page supplies its name. Compact mode actions share one wrapping row; their question is
not repeated in progress, and the page keeps its own project-creation action. There is no check
timestamp or Refresh. The optional Start here guide and dismissed Show control follow the same
eligibility; guides still open ordinary documents. Choices remain in portable personal state.
Progress rows use the card's width: below 32 rem they stack, leaving room for two compact
creation controls plus a readable label. Status icons and first labels share the control height.

The typed `/me/first-steps` read derives current permitted stored facts; its atomic PUT merges
the person's setup choice into help and navigation state, preserving unrelated values.
That answer updates the screen directly. Small `state` events name only the changed `child` of
help/sidebar, preserving unrelated portable choices in other windows without a read or polling.
Component tests (first-steps-visibility.test.tsx) cover owner/root/missing-step scope and compact,
nonrepeating controls in English and Chinese, light and dark. Browser sources cover keyboard,
phone layout, scoped guide navigation, dismissal and reload without Refresh. These sources are
unrun by the writer: the integrator owns rendering at 1440/400, contrast, request counts and gates.

- `Checklist.tsx`: Guided setup for a new team's root: observed provider and teammate steps, with portable choices and ordinary document links.
- `state.ts`: Portable first-steps choices, observed setup eligibility and safe org landing.
- `RoleSummary.tsx`: Current invitation defaults, without promising authority on a model or an overridden page.
- `save.ts`: First-steps choices saved through the canonical atomic preference request.
