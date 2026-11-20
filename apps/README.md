# Executable entry points

The programs of Orch. Each one only composes the libraries and the built-in extensions: reusable behavior lives in the library layers, and a program stays small.

| Program | Package | What it does |
|---|---|---|
| [api](api/README.md) | `orch-api` | The HTTP, realtime and internal backplane API. It hosts the durable engine |
| [gateway](gateway/README.md) | `orch-gateway` | Authenticated machine sessions, routing, tunnels and the relay between machines and the API |
| [worker](worker/README.md) | `orch-worker` | Database upkeep and delivery to the outside world; it orchestrates no machine |
| [connector](connector/README.md) | `orch-connector` | The agent, provider and environment roles on a connected machine or in a managed environment |
| [node](node/README.md) | `orch-node` | The lifecycle of managed environments on a host, through the drivers |
| [ctl](ctl/README.md) | `orchctl` | Administration, bootstrap, diagnosis, usage and plugin checks from a command line |

## Build and test

```sh
cargo build -p orch-api                # any package above
cargo run -p orchctl -- --help         # the command line of an operator
cargo xtask verify                     # format, Clippy, build, docs and dependency checks
```

Each program's README describes its behavior and how to test it. See [CONTRIBUTING.md](../CONTRIBUTING.md) for the checks every change passes, and the [component inventory](../docs/development/components.md) for where each part of the code lives.
