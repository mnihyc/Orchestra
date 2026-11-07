# Work left running by a thread

Native commands come from the thread's live session roster. Reviews and counts stay in memory, and End jobs retain only the captured identities and factual outcomes; reconnect and completion read current facts again.

- `Running.tsx`: Current native commands in a desktop dialog or phone sheet, exact reviewed End and job outcomes.
- `live.ts`: Ephemeral native counts discarded on disconnect, reconnect or permission loss.
- `review.ts`: Captured paging, complete-count confirmation and persistent mutation outcomes kept apart from live rosters.

- `BackgroundWork.tsx` and `backgroundWake.ts`: recorded, reader-masked names, states and effective/pending wake notices in the header. Native transport loss does not invent task loss.
- Recorded task/run identities are not End command identities. A header or child-chip End opens the existing paged HTTP review, maps that same task to the new review's `end_id`, and confirms the existing native End job. Identity changes, a lost task, missing End capability or an absent mapping cannot redirect the request to later work.
