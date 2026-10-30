# Thread state at scale

The probe reads the current thread-display SQL and holding-job predicate from the source.
It uses one PostgreSQL session with temporary relations, explicit statement and process
timeouts, and bounded working memory. The relations disappear when that session ends.
No application history body is returned to the probe.

Set `ORCH_TEST_DATABASE_URL` to the test PostgreSQL database, then run:

```sh
python3 -I scripts/thread-display-scale/check.py --tree . --output .tmp/thread-display-scale --rows 1000000
```

The corpus contains 1,000 selected threads plus a million unrelated metadata rows,
a million turns and inputs, a million approvals, schedules, jobs, read positions and recoveries,
and a million item weights totaling ten billion tokens. Half the historical inputs
have known acceptance sequences; half use the imported-history timestamp fallback.

The probe records baseline and indexed `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` plans.
It applies the current forward indexes only inside its temporary transaction. Assertions
check selected row counts, bounded topic pages, SQL attention totals, selective index
scans rather than unrelated history scans, and unchanged failure semantics after later
known-sequence and imported inputs. Every plan must avoid the item-body relation.
Execution times are recorded for diagnosis; they are not the structural acceptance test.

`frontend/tests/unit/thread-display-scale.test.ts` checks the number of metadata lookups
for 1,000 threads through 1,000 nested places and checks cycle termination and cache reuse.
`frontend/tests/component/thread-recovery-display.test.tsx` checks that 1,000 icons share
the same timer count as one icon, add no network reads, and release those timers on unmount.
