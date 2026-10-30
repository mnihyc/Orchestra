# orch-executor-contract

Executor contract for tool execution inside an environment.

The contract of executors, the programs that carry out a harness's tool calls inside an environment: the `Executor` trait (describe, available, start, stop), the descriptor each executor publishes (whether it speaks a protocol of its harness's own or offers its tools as an MCP server, its tool sets, the sandbox modes it can enforce, and the variables that carry a command's thread and tool-call ids), what an executor is given about its environment, and where a started one is reached, which must be a loopback address on a port the system gave. A test executor that cannot start where its build is missing exercises the trait; the two built-in executors' declarations are round-tripped as literal examples.

| Module | Planned responsibility |
|--------|------------------------|
| [executor](src/executor.rs) | Executor lifecycle and loopback endpoints. |
| [descriptor](src/descriptor.rs) | Availability, the protocol it speaks, its tool sets, sandbox support and attribution variables. |
| [context](src/context.rs) | Environment execution context. |

Tool-set reports carry `name` as their stable binding identifier, `title` and `summary` as English display text,
and optional `titles` / `summaries` maps by language tag. The manifest supplies both languages for built-in tools.
Older plugin reports can omit the new title fields; their display name is unavailable until declared, never inferred
from `name`. The API retains the declared text in its namespaced executor offers. This adds optional report fields;
stored offer identifiers and aliases keep their existing meaning.
