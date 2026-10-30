# orch-settings-contract

Settings catalogue and resolution data, without evaluation.

The registry is here as data: every key with its title, a sentence about it,
its group, a schema of its value, its class, the scopes it can be set at, its
merge mode and its default, with the check that a value fits a key and the
answer to who may write a key at a scope. `resolve` combines the values of a
chain of scopes by each merge mode and under locks, and reports where the
result comes from in the shape a reader of settings receives. It reads no
storage. Unit tests state each rule, among them that every default fits its
own schema.
Places can choose whether new nodes use their kind's icon colour or a random
palette colour through `nodes.new_color`.

| Module | Planned responsibility |
|--------|------------------------|
| [registry](src/registry.rs) | Registry keys, schemas, classes, scopes and shipped defaults. |
| [unit](src/unit.rs) | Named commands and template programs supplied to managed units after each start. |
| [merge](src/merge.rs) | Declared merge modes and lock semantics. |
| [provenance](src/provenance.rs) | Resolved values, origin and environment defaults. |
