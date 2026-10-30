# orch-connector

Agent, provider and environment roles on managed or connected machines.

The program enrolls a machine into a system with a token
(`enroll --url … --token …`), holds its session and does what the system asks within the owner's limits (`run`, or
`run --managed` in a runtime the system made, which enrolls from the launch variables when the data directory holds no
identity), shows the machine (`status`) and shows and sets the owner's limits (`policy show | set | delegate |
undelegate`). It hosts the scripted harness (`orch-adapter-fake`) and the real Codex and Claude Code for the agent role,
serves the model calls of the harnesses beside it, with the key of a provider it holds, for the provider role (in the
Responses and Messages formats), renews its certificate when it is due, and stops on an interrupt or a termination
after ending its session. Logs go to daily files and standard error; under launchd ordinary logs go
only to the daily files, with standard error retained for panics. `provider set-key --provider <id>` reads a key from standard input and keeps it on this machine, sealed
to it, for a provider that is started here without a key: the key is never shown and never sent to the system.
`console` offers the same local controls; `service` manages the per-user service, `env` offers local
directories, `mcp import` asks Orch to file a copy as this machine's owner, `update` verifies and
installs the system's recommended build, and `doctor` reports connectivity and configuration.
The running connector rereads policy changes; changing its roles requires a restart.

The tests run the program itself as a process against a system in memory behind sockets on the loopback address:
`cargo test -p orch-connector --test program -- --nocapture` shows each step with its time.

`tests/raw_restart.rs` runs the real program four times with three termination signals and fresh
adapter-format diagnostic frames between runs. The receiver commits each frame and its stream cursor
to PostgreSQL before acknowledging it; all twelve frames remain in order, once each.

The role-enrollment case in `tests/program.rs` runs the program with a choice-bound token against
the loopback enrollment fixture. It checks the saved roles, section handover and dedicated fact,
the local roles command, unchanged identity and refusal of Node before any request.

`src/composition.rs` chooses everything concrete (the adapters, the two wire protocols, the clock, how the gateway is
dialed) and starts the
connector from the data directory; `src/arguments.rs` is the command line and `src/local.rs` the commands that only
read or change the files of the data directory.

`src/enrollment_options.rs` validates local enrollment role/handover choices. `enroll --roles
environment,agent --delegate environment --dedicated` writes the accepted roles, role-section
delegation and local dedicated fact only after trusted enrollment. `--delegate-policy` hands over all
keys. Node runs in `orch-node` and is refused here. `roles show` reads the local configuration;
`roles set environment,agent,provider` changes it locally. Restart the connector to activate a new
role set; the authenticated reconnect reports it. This is never a policy act from the system.

`src/ports.rs` implements the read-only `ports list`, `ports address <port>` and
`ports whois --peer <ip:port> --listener-port <port> --transport tcp|udp` commands. Run them
inside an environment: they use its `ORCH_PORTS_URL` and, on a connected machine, its
separate `ORCH_PORTS_TOKEN`. They print the canonical JSON projection, including `null` for
a definite peer nonmatch; a temporarily unavailable signer is a refusal, never a guessed identity.
`ports list --look --limit 20` asks for a current observation and keeps the server's continuation.

The connector loads provider, translator, adapter and executor programs from `config.toml` and the template's
`ORCH_PLUGINS` JSON array. Duplicate registrations and names of built-ins are refused. External adapters and
executors are offered beside the built-ins; provider programs use the same source registry for both keeping
roles. Machine facts carry each program's current state and a provider program's negotiated declaration.

The machine page's Programs tab also registers agent adapters and provider sources at run time.
`agent.programs` and `provider.programs` must be handed over and enabled; a locally kept value
wins under Full Access. Register validates the program before persisting it in that same file;
Test calls its declaration and optional models, Remove only retires Orch registrations, and
Reload applies the operator's local file. The complete updated facts are returned before the
next profile/provider request. A registry change refuses while a turn is running.

| Module | Responsibility |
|--------|----------------|
| [uninstall](src/uninstall.rs) | Plans one instance's service, shared program and optional purge of its recorded owned folders. |
| [programs](src/programs.rs) | Owns the existing supervisors, live adapter/source factories, persistent registration and bounded interactive declarations. |

It plays the environment role too: the files, shells, processes, executors and tool servers of an environment on this
machine, with Orch's own executor and the exec server of Codex to offer, and the places the driver gave when it runs
in a runtime the system made (`environment_parts` in the composition). The role is tested in `orch-connector-core`,
on the in-memory hub.
