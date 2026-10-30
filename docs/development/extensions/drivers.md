# Driver development

Implement [`EnvironmentDriver`](../../../crates/contracts/driver/README.md).
The node supplies a `RuntimeSpec`: its pool and identity, the driver-specific
template spec, resources, network requirement, connector roles, enrollment
inputs, tools, mounts, volume references and system labels. Declare a schema
for each pool you support; environments and provider runtimes have different
storage requirements. The driver must start the connector with the launch
variables defined by the contract, keeping enrollment material private.

Creation is idempotent by the unit name. Inspection reports running, stopped,
absent or failed. Listing returns only the requested system's units, so a node
restart can reconcile them. Stop honors its grace, removal erases the unit
and keeps volumes, and explicit volume removal erases only that owned storage.
The same volumes must survive a recreate. Never use another system's units
or volumes as a cleanup shortcut.

Confinement, enforced resource limits, mount placement, private views, image
sources, imports, suspension and restricted networking are separate declarations.
An open-only driver refuses a restricted request before starting anything.
A restricted driver needs real positive transport paths and negative network
paths, including restart and policy narrowing, before it can advertise them.
Proposing an imported template creates no unit and consumes no enrollment.

Run `orch_conformance::driver::run` with a valid base spec on a host reserved
for the check. It creates fresh identities and volumes, checks lifecycle and
reconciliation, then removes its own resources. Your native tests also verify
that file contents survive removal/recreation and that unsupported operations
produce their declared refusal.

The [external process example](../../../examples/plugins/driver/README.md)
demonstrates the actual plugin bridge on Linux. Save
`{"command":["sleep","600"]}` as a disposable spec and run:

```sh
orchctl plugin check --spec driver-spec.json python3 examples/plugins/driver/driver.py --base ./driver-check
```

Its `--wrong-reply` returns another runtime id and fails `driver.create` by
name. It declares no sandbox, mounts, image installation or restricted network;
it is a small process implementation, separate from the reserved built-in host
driver and the implemented Docker driver.

The [driver boundary](drivers/boundary.md) lists shared responsibilities and
optional support.
