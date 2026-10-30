# External driver example

This Python 3 program creates Linux process groups through the driver contract.
It keeps units and their launch files under `--base/units`, persistent workspace,
home and state directories under `--base/volumes`, and lists units by the system
label. Removing a unit keeps its volumes; only `remove_volume` erases a volume.

The default command is `orch-connector run --managed`, with the contract's launch
variables. A template can replace `command`. The driver reports no confinement,
resource enforcement, network restriction, mounts, harness installation or image
sources. It has the permissions of the account that runs it and supports Linux
only; it uses Linux process start ticks to avoid signalling a reused process id.

Register it on the node with a base reserved for this driver:

```toml
[[plugin]]
kind = "driver"
name = "processes"
command = ["python3", "/opt/orch-plugins/driver.py", "--base", "/var/lib/orch/process-driver"]
```

For a disposable lifecycle check, save `{"command":["sleep","600"]}` as
`driver-spec.json`, then run:

```sh
orchctl plugin check --spec driver-spec.json python3 examples/plugins/driver/driver.py --base ./driver-check
```

The check creates, starts, stops, recreates and removes its own unit and volumes.
The example's `--wrong-reply` deliberately returns the wrong runtime id: it fails
the named `driver.create` rule and gives a failing exit status. The conformance
tests run both forms through the actual plugin bridge; the process log never
shares the transport's standard output. A plugin restart leaves units running,
so the node can reconcile them from their saved identities.
