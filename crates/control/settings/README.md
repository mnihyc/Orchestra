# orch-settings

Settings evaluation over the shared catalogue.

It reads the rows that are set along the chain of scopes for a place (the system, a node, an environment, a provider's runtime) in a number of statements that does not depend on how many keys are asked for, gives them to the contract's rules, and answers with the value and where each part of it comes from; the defaults that the environment, a model or the seeds supply stand under every row. It writes a value at the system or at a node, or takes it back, with its schema, scope, lock and revision checks and its audit row in the caller's transaction, says what a write asks of the caller without deciding it, and keeps secrets sealed to their rows, listed by name down the hierarchy and opened for the engine alone. It is tested on a real PostgreSQL: each merge mode, locks and what survives them, provenance, supplied defaults, the own values of an environment and of a provider, refusals, stale revisions, the lines of history, secrets, and a count of statements.

| Module | Planned responsibility |
|--------|------------------------|
| [resolver](src/resolver.rs) | Hierarchy traversal down to the thread, the environment or the provider concerned, and environment-supplied defaults. |
| [merge](src/merge.rs) | Replace, union, intersect, map, append and minimum. |
| [locks](src/locks.rs) | Locked ancestor handling. |
| [validation](src/validation.rs) | Schema, scope and current eligibility checks. |
| [provenance](src/provenance.rs) | Explain effective values and their sources. |
| [supplied](src/supplied.rs) | Defaults that something other than the registry supplies. |
| [write](src/write.rs) | Writing and clearing a setting in the caller's transaction, with its audit row. |
| [secrets](src/secrets.rs) | Secrets of a node or an environment: sealed, listed by name, opened for the engine. |
