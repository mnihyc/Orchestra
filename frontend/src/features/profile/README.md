# A person's page

The page at a username reads only the pins, memberships and work the API exposes to its current reader. A closed page opened through member management displays the inspection band. Your page edits the person's words, HTTPS links, privacy and ordered pins through their ordinary requests.

- `Page.tsx`: Username page, pins, inspection band and reader-shaped contents.
- `Identity.tsx`: Identity, current local time, links and ordinary direct-chat entry.
- `Activity.tsx`: UTC calendar with accessible counts, the complete table and recent visible topics.
- `Pins.tsx`: Favourites, recents and authorized name search, with drag and keyboard ordering.
- `Settings.tsx`: Personal settings and the page preview.
- `CardDetails.tsx`: The same visible identity and inspection facts in the shared person's card.

The calendar and card have component proofs; the fake profile world and browser proofs exercise page privacy and editing. The integration round runs these checks.

The public name is its one visible page heading. An unavailable page keeps the same private-safe
message with a heading and a way home. The settings preview uses its own pin heading under Page preview.
Profile and pin saves share a per-person mutation key, freeze their submitted values and update the
canonical profile cache from the accepted answer. The other edit controls wait so the two complete
profile answers cannot overwrite each other. A refused draft stays editable.

Pin search uses the shared typing pause, retains visible recents and rejects stale result choices.
Candidates and selected pins use the common shortest place label from their returned paths and cached
orgs, without loading another org's tree. Compact reorder icons align with the first line. Lookup failures
offer a deliberate retry, and the pin description makes clear that pinning grants nobody access.
