# Keep implementation choices behind drivers

```sh
python3 -I scripts/check-driver-boundary/check.py
```

The guard runs parser regression cases, then scans production Rust and browser
code outside driver implementations. It rejects literal implementation-name
choices in comparisons, conditionals, matches and switches. Registered factory
names, comparisons between dynamic identities, source formats and ordinary
resource kinds remain valid. Explicit driver identity expressions also catch
new external names that have no implementation directory yet.

Shared contract/database types, serialized runtime state and migration columns
may not gain Docker or daemon-specific identifiers. Plain local implementation
handles are outside the stored-data check. Field renames count; read-only aliases remain
compatible. The existing `BuildSource.dockerfile` source format is the exact
exception, rather than a blanket allowance for implementation-specific fields.
Comments, fixtures, generated browser types and test-only modules are excluded.

`orch-testkit` runs this check as its `driver_boundary` integration test, so the
ordinary Rust test and release workflows include it. The scanner is a structural
regression guard, not a language type checker: new identity-selection syntax
needs a failing parser case when it is introduced.
