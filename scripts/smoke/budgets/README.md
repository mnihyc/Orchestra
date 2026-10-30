# Query scale probe

Run against the disposable PostgreSQL test service with `ORCH_TEST_DATABASE_URL` set:

```sh
python3 scripts/smoke/budgets/scale.py --output .tmp/budgets-scale --holder-candidate path/to/forward-holder-migration.sql
```

This compiles no Rust. It creates and drops one named database, bulk-loads a million users, groups, memberships and documents, installs the real holder function, and prepares the real group/count/document SQL. It records a thousand logical threads, ten billion logical token counters and a million synthetic items. Tokens are counters rather than ten billion materialized strings.

The relations retain the columns and indexes used by these queries; this is an isolated query-plan and eligibility probe, not full production-schema or timeline acceptance. The final large-world release gate must also exercise the actual API and browser.

The document fixture deliberately gives every document the same update time. The probe checks 31 returned candidates, bounded indexed work after the candidate, and preservation of all group people, active status, suspension and deleted-root eligibility. Read plans and buffers; elapsed time under shared load is not an acceptance substitute. The synthetic tail only checks the fixture's own index.

Output stays in the requested directory. Request/statement timeouts and `finally` cleanup bound the probe; no other database or container is changed.

The production upgrade still needs its allocated forward migration. The exact candidates
are `holder-candidate.sql` and `document-page-index.sql`. The former parameterizes group
metadata before joining people; the latter supports the direct-place query in
`documents::list_beside`, including its cursor's id tie-breaker. They are probe inputs,
not automatically applied application migrations. Keep existing migrations unchanged.
