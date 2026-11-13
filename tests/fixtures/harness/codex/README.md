# Codex protocol compatibility corpora

Both version directories use the shared `Manifest`, `Scenario`, `Frame`, `Corpus`
and `replay_io` types. Every required shared scenario has an explicit decision.
The immutable published app-server and config schema URLs and checksums are in
each manifest and in `extensions/agents/codex/schema/<version>/sources.json`.
These are authored protocol fixtures, never recordings or live vendor results.

`upgrade_notifications` feeds ordinary notifications through the production RPC
reader and public mapper. `upgrade_controls` runs native requests, approvals,
settings and lifecycle methods through the actual adapter and its launcher pipes.
Their common module only dispatches cases to those existing seams and loads the
shared corpus; it contains no parser, matcher or alternate replay engine.

Control scripts include their full startup exchange. The shared replayer binds
`codex-home` to the test's actual owned temporary `CODEX_HOME`. The launcher does
not rewrite frames or construct a private handshake. Both sides share one manual
clock; native replies and completion events determine the observation boundary.
Pipe ownership stays with the fixture until the adapter closes it. Only a scripted
Exit is process loss; unexpected trailing adapter output fails the case.

The current adapter refuses an actual 0.165.0 startup and accepts 0.167.2; the start
cases assert those real results. Other 0.165.0 control shapes run through the
accepted 0.167.2 actor, while notifications use their published dialect directly.
This covers legacy wire parsing without overriding the build policy or claiming
that an older binary was qualified to start.

Each managed pin has a separate actor scenario. Its one incorrect effective value
must produce the exact pin's mismatch marker and `harness_diverged`, with no new
turn or autonomous action. The manifest pin inventory must equal the production
pin map. Missing optional native values remain unknown in separate adapter tests.

Native plan approval is unoffered on both schemas. Native terminal End is also
unoffered on both: neither list nor terminate is probed, and the unavailable cause
is retained. Ordinary turn interruption and session close remain supported.
The 0.165.0 command schema additionally lacks humanCommand checkpoint and
prompt-user callbacks. The explicit synthetic future-build method-not-found case
stays in the adapter's separate fixture directory; it is not native evidence for
either pinned build.

Search, open page and find in page share WebSearch with its reported WebAction.
An absent address, result, HTTP status, page body or charge is never invented.
Compaction phase/item/idle cases do not infer a native trigger that the protocol
does not report. Unknown events are counted, unknown approvals can be cancelled,
and malformed text becomes an error without enabling raw frame logging.

All event order and counts use the shared assertions. Stable object subsets are
explicit; arrays stay exact. Notification cases ignore no events. Control cases
ignore only separately produced process-status and model-performance event kinds,
never compatibility markers or errors. Engine tests separately own admission,
current authority, background continuation input and usage accounting.
