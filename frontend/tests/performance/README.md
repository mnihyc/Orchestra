# Performance validation

`request-budget.spec.ts` covers every shipped route against the fake API, with an inventory guard for
new routes. It counts every `/api/v1/` HTTP request apart from the live socket: at most eight for a cold
route, none during sixty seconds idle or a focus/online signal, and one profile act without list reads.
Another person's native rename must update its visible row without HTTP. Browser time is advanced for
the idle interval; the budgets are counts and are never scaled with machine load. Its opt-in native
fixtures and event projections live in `tests/doubles/request-feeds.ts`. The integrator executes this
with the ordinary serialized Playwright batch; authors do not run it separately.

A conversation of five thousand items in a real browser: how soon it opens at its end, how many rows
and elements the page holds however long the conversation is, how smoothly it scrolls from the first
item to the last and back, how much memory it holds, and how soon a piece of text that arrives is on
screen. The numbers measured are written into the report of the run, and the budgets they are held to
are several times what a quiet machine measures; those in milliseconds, and the time a test may take,
are multiplied by how much slower than a quiet machine the machine is at that moment, found by timing a
fixed piece of work in the page (frames by that to the power of one and a half, as frames are late by
more than the work is), so that a busy machine is not taken for a slow page, and a test waits, up to ten
minutes, for the machine to be no more than two and a half times slower before it measures: the page's
own limits are in seconds, which a machine far slower than that does not leave it. The rows and
elements held and the memory keep their budgets however busy it is. Bundle budgets are not asserted yet.

The finder over ten thousand names in the fake API: from a key to the rows the page holds (commands, settings, help), and to
a name that only one of the ten thousand has, which includes the pause the finder waits after the last key; the figures
are written into the report, and the budgets are multiplied by how much slower than a quiet machine the machine is.

The left bar's tree with two thousand nodes, folders to three deep among them, five documents beside
every project and folder, and every row open: how soon it is drawn once it has arrived, how long after
that the documents are in it, what the page holds, and what a thread that changes its state
costs, for a row far out of view and for a row in view (one after another, and fifty at once). That cost
is the CPU time of the page's main thread from the change to a pause after the rows show it, read from
the browser's own metrics, less what the same pause costs at rest: the thread's own clock is not
stretched by a machine that is busy, as the time between frames is. The specification's figure of
16 ms a frame is for the production build on a quiet machine; the budget here is that figure times how
much slower the core is just now (a fixed piece of work, timed by the same clock), times what the
development build of React costs when the server is not a production build (`ORCH_PERF_BUILD=production`
says it is, which the Playwright config sets when the tests run against the built application, as they do unless `ORCH_E2E_DEV=1`). A row in view is held to two frames, its drawing and its first painting. The test turns
its own recording (trace, screenshots) off, since that is work of the same thread.

Measured on a machine with ten processors all busy (a core 1.3 times slower than a quiet one), production
build: a thread far out of view, 12 ms at the median and 26 at the 95th percentile; a thread in view, 18
and 33; fifty at once, 127; the first drawing of every row, 5 s. The development build: about five to
seven times that.

With five documents beside each of forty places (2 272 rows, 210 of them documents), measured on the
development build with the machine 2.7 times slower than a quiet one and its core 1.7 times: the tree drawn in
26 s and the last of its documents 30 s after it; a thread far out of view, 204 ms at the median against a
budget of 265; a thread in view, 350 against 531; fifty at once, 996 against 2 654; 41 thousand elements and
415 MB held. The production build has not been measured with documents in the tree.

Add checks with product behavior; do not assert file layout, import choices, component shape or
other implementation details.
