# Status items

One registry supplies live values to the status bar, thread header and live line. Their visibility,
place and order stay in the person's local workspace. Resource values open the shared resource card;
clock updates use existing feed data and never issue requests. Stream estimates read numeric counters
maintained when rows and deltas are admitted; status placements never traverse or join held history.

| Module | Purpose |
| --- | --- |
| `registry.ts` | Item words, available places and values from reported or explicitly estimated facts |
| `arrangement.ts` | One validated presentation model for visibility, placement and order |
| `useValues.ts` | Existing thread/resource cache and feed readers for the selected items |
| `StatusPlace.tsx` | Shared context menu and inspection behavior for all three places |
| `StatusStrip.tsx` | Fitting items and accessible overflow on one or two lines |
| `Customize.tsx` | The common checkbox, drag and keyboard order editor |
