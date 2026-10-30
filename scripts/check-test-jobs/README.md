# Checking test jobs

`TestDb::fresh()` reserves database capacity before opening a PostgreSQL
connection. Cooperating processes on one host use the same lease directory
and server address. The default budget is 128 connections with 32 reserved
per fixture: four fixture connections, the largest secondary test pool of
24, and four for administration, listeners and headroom. This admits four
fixtures at once. Pools opened separately through a fixture's URL must fit
inside its reservation; increase the reservation when a test needs more.

Configure `ORCH_TEST_DATABASE_CONNECTION_BUDGET`,
`ORCH_TEST_DATABASE_CONNECTIONS_PER_FIXTURE`, and
`ORCH_TEST_DATABASE_LEASE_DIR` consistently for jobs sharing a server.
The default directory is `orch-test-databases` inside the operating system's
temporary directory. A budget change is refused while a fixture is active;
the configured budget is checked against PostgreSQL's usable connection
capacity. Address aliases and different lease directories do not share
admission, so use one address and directory for a test server. Tests waiting
for admission hold no database connection. `ORCH_TEST_DATABASE_WAIT_SECS`
defaults to 300 seconds, covering enrollment, attach and one job lease ahead
of a waiter. A timeout identifies the lease, its capacity and its records.

Run one job with Python 3 and `psql` available:

```sh
python3 scripts/check-test-jobs/run.py --output-dir .tmp/test-job -- cargo test -p orch-api-core --test chats_threads
```

Tests that launch a connector need its binary from the same tree, target
directory and profile. Prepare that binary before starting a whole API test
job, so database leases are held for fixtures rather than a cold build:

```sh
cargo build -p orch-connector
```

The runner assigns `ORCH_TEST_DATABASE_RUN_ID` and records it in `run.json`
before starting the command, captures unfiltered stdout and stderr in
`output.log`, and writes `result.json`. Its exit is the command's
exit, 124 on the job timeout, or 2 when a successful command leaves owned
databases or cleanup errors. A printed passing test summary never overrides
a nonzero command exit. Child processes remaining in the job's process group
are stopped before inspection. The job timeout defaults to 900 seconds and
can be set with `--timeout-seconds`; choose it for the whole selected job,
including deployed waits and cleanup, rather than repeating until it passes.

Lease records contain exact generated database names, run identifiers, pids,
connection reservations and cleanup results. They contain no database URL
or credentials. Normal destruction waits for forced database cleanup on a
separate thread with its own runtime. Forcing the database's connections
closed avoids waiting for borrowed connections whose tasks the destructor
is holding up. `TestDb::close().await` also returns cleanup
errors. Destructor failures are printed and recorded, and the runner makes
them fail the job. An unfinished ownership record also fails the job, even
when the database is already gone: cleanup was not acknowledged. Cleanup
takes up to 300 seconds on a busy test server.

After a stopped job, inspect its recorded run identifier:

```sh
python3 scripts/check-test-jobs/run.py --output-dir .tmp/test-inspect --inspect-run RUN_UUID
python3 scripts/check-test-jobs/run.py --output-dir .tmp/test-cleanup --cleanup-run RUN_UUID
```

Use the same lease directory and `ORCH_TEST_DATABASE_URL` as the job.
Cleanup refuses slots still held by live fixtures and removes only the exact
databases in that run's validated records. It reports every exact removal
in `exact_cleanup`. Records remain for diagnosis. A historical database
without ownership evidence is never selected by a prefix search.

Real PostgreSQL regressions in `crates/control/api/tests/ktestkit.rs` cover
competing processes with secondary pools, admission timeout diagnostics,
explicit cleanup, and a process that prints success but exits unsuccessfully
and leaves its database. Engine regressions hold domain time through the
deployed hosting waits and advance a controlled clock to the deployed
interruption deadline. A test deliberately expiring a wait configures that
wait explicitly; timeout diagnostics include the awaited event and its
observed state.

## Tests that must hold on a loaded machine

A test proves an order of events, not a speed. `repeat-under-load.py` runs named tests twenty
times in a row while four other `cargo test` commands loop beside them, after compiling every
candidate first, and exits 1 when any run failed:

```sh
python3 scripts/check-test-jobs/repeat-under-load.py --output-dir .tmp/repeat
```

Without names it runs the bounds the test kit corrected (the Git byte bound, the two shells of an
environment, the busy terminal, the two guarded-write orders); `--load` replaces the four load
commands and `--wrap` puts a gate in front of every cargo command. Give the test database's
variables in the environment, as for any test.
