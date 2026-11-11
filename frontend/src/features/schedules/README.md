# Scheduled inputs

People's authored future inputs and agent continuations use the native schedule definitions, topic projection, occurrence history and state operations. Scheduling freezes an input and creates no immediate turn. Definitions stay author-private; later execution keeps the author's current access and limits.

- `Schedules.tsx`: Shared pending continuation lines, thread lists and native cancellation.
- `facts.ts`: Join schedule and occurrence facts by their actual identities.
- `WakeMarker.tsx`: Native occurrence provenance for a started turn.
- `timing.ts`: Explicit once/daily/weekly calendar entry and preview, including gaps and folds.
- `Editor.tsx`: Private whole-draft schedule creation/editing with explicit timing and native setup choices.
- `ComposerSchedule.tsx`: Current-reader snapshot capture and exact accepted-draft clearing.
- `HumanSchedules.tsx`: Author edits, revision-checked pause/resume and lazy occurrence history in shared lists.
