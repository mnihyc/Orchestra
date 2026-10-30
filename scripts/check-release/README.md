# The proof of a release candidate

`run.py` runs the whole proof on the exact commit that would be released and keeps the record:
the commit and whether the tree is clean, the toolchains, and for every step its log, its exit,
its time and what it ran against (the source, a real service or program, or the doubles of the
tests). It goes on after a failure, and names the failing test programs and tests from cargo's own
lines.

```sh
python3 scripts/check-release/run.py --output-dir .tmp/release-proof --with-deploy
```

The steps, in order: `deny` (`cargo deny --locked check`), `fmt`, `clippy`, `doc`, `contracts`, `connector` (the connector the tests
start), `rust-tests` (every test program, against a real PostgreSQL: give its variables in the
environment), `web-typecheck`, `web-lint`, `web-unit`, `web-build`, `browser` (the browser suites,
against the fake API of the web application's tests) and, with `--with-deploy`, `deploy` (the
deployment scenarios on Docker). `--only` and `--skip` choose steps; `--wrap` puts a gate in front
of every heavy command on a shared machine; `--install` runs `npm ci` first.

`summary.md` and `results.json` are written into the output directory. Exit 0 means every step
that ran passed.

Dependency evidence includes the cargo-deny version and SHA-256 of `deny.toml`, `Cargo.lock` and the
npm lock. A reported earlier check does not attest to a different graph or policy. The dependency
gate checks the whole configured graph, including advisories; a round's bans/licences/sources
check alone does not attest to its advisory database. Keep the dependency log beside the result.
