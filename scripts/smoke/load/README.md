# Candidate load proof

Supply `ORCH_SMOKE_RELEASE_A` and the already-built `ORCH_SMOKE_MOCK_MODEL`, then run
`bash scripts/smoke/load/run.sh`. It uses the released stack and connected connector described in
[link-load](../link-load/README.md). The frontend's pinned Playwright dependency and its Chromium
installation must already exist; this runner installs and builds nothing. Ports are 41410–41413.

The test creates 100 real threads. One input per thread is sent from a common barrier; its fake
adapter deliberately holds the turn for 30 seconds so that a single observation must find all
100 running together. The fake model and adapter are named doubles, while the HTTP, storage,
engine, connector and realtime paths are actual candidate programs. Each input, turn and unique
output must occur exactly once. Failure, filtering or a missing overlap cannot pass.

Thirty independently launched Chromium processes sign in through the real UI and warm the cache
before load starts. During the overlap they reload a thread, type without sending their draft,
and observe actual text deltas. No request, socket or permission is mocked. The observer records
frame-to-visible-DOM/next-paint opportunity, real input-to-next-paint opportunity, page errors and
Chromium's JS heap. It does not replace a pixel recording or a 10,000-item scrolling benchmark.

`budgets.json` is copied into every evidence run. Warm thread 1500 ms, local typing 16 ms,
delta-to-paint 50 ms and renderer heap 300 MiB are the published frontend budgets. Acknowledgement
p95 1000 ms applies the slow-link acknowledgement ceiling conservatively to this LAN fixture.
Eight GiB is this fixture's base-stack allocation, excluding the thirty browser processes;
the peak is measured, not inferred from configured limits. Docker CPU, memory, PID and PID-1
descriptor samples accompany the work log. A 300-second completion guard detects hangs and is
not a claimed response time. `ORCH_SMOKE_BUDGETS` can declare a different fixture allocation, and
the resulting evidence always records exactly which budgets applied.

Evidence is retained under `.tmp/deploy-smoke/evidence/<run>/load`, including every phase exit,
candidate identity, overlap snapshot, counts, latency samples and resources. A nonzero browser or
thread exit fails the scenario. The helper unit tests validate evidence rejection; they are not
release load proof. This runner has not been executed by its writer. The separate 10,000-item
scrolling case and the [24-hour soak](../soak/README.md) need their own retained results.
