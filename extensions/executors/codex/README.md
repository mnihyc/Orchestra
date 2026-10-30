# orch-executor-codex

Codex exec-server hosted inside a thread's environment.

`CodexExecServer` runs the vendor's own `codex exec-server` from the environment's tool cache, on a loopback port
the system gives and behind a token, with an environment of its own making and a home for the program under the
executor's own directory. It reports the build the server says it is, and the sandbox modes that the vendor's own
`codex sandbox` is seen to enforce in the place, by a probe that checks what a command could write. The unit tests
use stand-in programs; the integration tests run the real build, only inside Docker from the image of the
harnesses, with and without the permissions its sandbox needs, and are skipped with a message where Docker or the
image is missing.

| Module | Planned responsibility |
|--------|------------------------|
| [descriptor](src/descriptor.rs) | Executor availability, the build it has to be, and the variables Codex's exec server gives each command for attribution. |
| [server](src/server.rs) | Start and stop the vendor's own exec server on loopback, of the build the harness needs. |
| [sandbox](src/sandbox.rs) | Probe which harness sandbox modes the environment can enforce. |
