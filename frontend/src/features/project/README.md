# Project topics and environments

Built: a project's screen (`/project/$projectId`) with one compact name/action header, List/Board/Activity,
one common filter bar and its topics immediately below. New topic appears once in the header, including
on empty and filtered lists; a refused action stays discoverable with its reason. Description and tags
open on demand, and New folder is a secondary place-menu action. Topics are filtered
by status, tags, attention, participation and search, sorted and paged; the board's
Done column takes cards by drag and by menu; rename, description, archive, delete with restore, move,
and its Activity. A folder, which is a project that lies in a project, has this same screen with a
folder glyph: compact folder rows follow its topics, using names and counts already held in the live
tree, without a detail request for each row. Search, attention, participation and archives reach
folders as far as they apply. The list and the
board show the project's own topics; its Activity takes in its folders and says so ("Web and the 3
folders in it"), and names the folders a line was in. What is shown is in the address. The project's
provider default remains the existing project setting, chosen from usable providers by their readable
names. This page mounts no provider/default/starter form. Readme and usage follow the collection.

- `Project.tsx`: A compact project header and one filter bar above its topics.
- `TopicList.tsx`: Topics with tags, state and activity.
- `TopicBoard.tsx`: Attention, running, idle and done columns; visible drop targets stay in place during a drag while the page can still be scrolled explicitly. Failed changes offer their access and request details in a dialog.
- `FolderCards.tsx`: The folders of a project, or of a folder, as compact rows following its topics.
- `Move.tsx`: Move to…: a project taken to another org of its tree, with who gains and who loses what said first.
- `Activity.tsx`: A project's Activity: the lines of its own history and of the management of its topics.
- `ProjectStar.tsx`: Stars the current normalized view or the place alone, and opens the saved view with an ordinary address.
- `viewState.ts`: Shipped parameters, normalization, explicit-address precedence and currently readable saved tags.
- `TopicPath.tsx`: Actual folder paths with nonlinks for concealed ancestors.
- `selection.ts`: Loaded-row range selection, compatible status guards and confirmed-success-only inverses.
- `TopicSelection.tsx`: One frozen review, per-row pending/success/refusal/uncertainty and one revision-checked Undo group.
- `PlaceReview.tsx`: Selected move, archive, unarchive and delete plans, sequential progress, retained partial results and explicit uncertain-answer reads.
- `ReviewConsequences.tsx`: Complete subtree/work/share/link, access/settings and deletion/recovery facts with the common place labels.
- `reviewBatch.ts`: Each act's own fence and successful receipt, with one guarded inverse for each confirmed result.
- `Topics.tsx`: Paged topics and a selection result which remains available when changed rows leave the filter.

Page selection is the table-header checkbox (including its mixed state), with no separate empty
toolbar or loaded-count sentence. The review component stays mounted after clearing a successful
selection, so its confirmed results and revision-guarded Undo remain available. Personal saved views,
explicit URLs, folder paths, paging and existing board acts retain their native behavior.

Regression sources: creation-once, place-collection-rows, project-page-selection and
project-provider-default component cases, folders/project-views units and the place-collection-layout,
places/folders/project-views and starting-work browser cases. They cover both languages and themes,
1440/400 geometry, whole names, keyboard details, named defaults, guarded saves and retained results.
Authored only; the integrator runs the gates, request proof and fresh screenshot review.
