# Candidate restart and fault soak

Run `bash scripts/smoke/soak/run.sh` with the packaged candidate and prebuilt model fixture specified
in [load](../load/README.md). The default **1800 seconds is a shortened rehearsal**. Set
`ORCH_SMOKE_SOAK_SECONDS=86400` for the 24-hour proof; both requested and observed monotonic
durations are retained. Nothing in this repository claims that either has already run.
Ports are 41420–41423. Only this run's exact Compose services, fixture connector, network and
processes are stopped, restarted or disconnected.

Twenty finite, warmed fake-adapter sessions receive uniquely marked work repeatedly. After an
input is accepted and its turn observed running, the runner rotates API/worker/gateway/connector
restarts and a three-second connector network outage. Recovery resends the same idempotency key,
checks the returned identity, waits for an explicit terminal outcome and compares the stored
output. At the end, an independent PostgreSQL query reconciles **every** accepted key and turn
with the retained work log. A named failure/interruption is retained as such and makes the
unqualified successful-soak check fail; it is never lost work or a fabricated completion.

Docker CPU, memory, PID and PID-1 descriptor samples are retained through the fault cycles.
After faults stop, queues must return to their warm baseline. The default fixture allocation is
8 GiB, with at most 64 MiB quiet memory growth and 16 additional PID-1 descriptors over its warmed
finite session set. These explicit fixture allowances admit small runtime cache/pool growth;
they are not published product SLOs or a mathematical proof that a longer run cannot leak.
The same immutable budget copy identifies every run, and an operator may declare a different
allocation through `ORCH_SMOKE_BUDGETS`. A 300-second recovery/queue guard detects hangs.

Results, candidate identities, exact fault times, process exit, ledger query, queue counts and
resource samples survive cleanup under `.tmp/deploy-smoke/evidence/<run>/soak`. A short run always
has `owed_24_hours: true`, even when every observed cycle passed. The helper unit tests check
that shorter durations, duplicates, missing work and silent pending turns cannot pass the
evidence reconciler; those tests are not substitute soak evidence.
