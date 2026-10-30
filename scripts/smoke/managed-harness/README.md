# managed-harness scenario

A real harness at work in a managed environment, on a system started by the updater from a packaged release. Each
of the two built-in harnesses, Codex and Claude Code, runs a turn inside a unit made from the Default template on
the bundled host, and the key stays with a provider on another machine.

1. **The builds.** The release lists the default build of each harness for Linux (`harness_builds` of its manifest,
   from `deploy/harness-builds.json`: the address the vendor publishes it at and its SHA-256), and the system lists
   them as the default builds of the two harness kinds (`GET /harness-kinds`).
2. **A unit that has them.** `POST /environments` from the Default template: the job's `tools` phase has the host's
   node download both builds, check them against the hashes, and keep them in its cache; the unit is made with them
   mounted read-only under `/opt/orch/tools`; the connector in it finds them there, runs each once to ask which build it
   is, and reports them among the harnesses it has (the Codex exec server the template offers becomes available).
3. **A turn of each harness in the unit.** A thread is bound to the environment with the arrangement in which the
   harness runs beside its tools and the key stays with the provider (`key_separated`): the machine of a person
   (a container of the stack's network whose connector holds the provider role alone) holds the key, and the model
   is the scripted model server. The person wants to decide what runs, so each turn asks: the harness runs a command
   in the workspace, then changes the file it made (Codex with `apply_patch`, Claude Code with its edit tool), the
   items arrive on the realtime socket through the web edge, the approvals are decided over HTTP, the file is on
   the unit's disk, and the usage is in the ledger, counted by the forwarder on the provider's machine. Both threads
   name the arrangement: a Codex thread that names none is bound `separated` once the unit holds Codex (its exec
   server is then an available executor), and that runs the harness on the machine of the provider, which this
   scenario's machine could not do; the scenario prints what an unnamed thread is bound as, and does not judge it.
4. **How the harnesses were started.** What the adapters promise is looked at in the unit: the quiet switches in
   Claude Code's environment, its configuration directory and its setting sources, Codex's home and the keys of
   its configuration file that switch off updates, analytics, feedback and memories, no connection of the unit but its
   link to the gateway, nothing of a harness in the account's home, and nothing changed outside the volumes but
   temporary places.
5. **A real model, once each** (only with `ORCH_HARNESS_REAL_ENV=<file with OPENROUTER_API_KEY>`): a provider on
   the real interfaces is registered from the same machine, its key is given to the system over a pipe and printed
   nowhere, and each harness answers one small question (one request each; no thread is made until the provider is
   ready, which the machine that keeps the key decides by asking the upstream for its list of models, so that a
   refused key or an upstream out of reach costs no request to a model). Then the key is looked for where it must not be (the
   unit's files, the environment and command line of its processes, what the daemon says of it, the services' logs
   and the logs of the machine that keeps it), by a search that hands it to grep as a file descriptor and prints
   nothing, after a control that shows the search finds a key where there is one.

Then the topic is deleted, the environment is removed, and its container and volumes are gone.

This scenario also accepts the opt-in native/import subscription gates described in [managed-provider](../managed-provider/README.md). It sources that same proof helper, uses this released stack and Default environment, and requires fresh managed subscriptions for both harnesses. Imported bodies come only from the explicitly supplied private JSON files; model names are explicit. The two real turns must leave distinct counted turn numbers across the API and runtime restart. Native mode additionally proves cancellation and a rejected/expired flow before the human-authorized sign-in. These gates are skipped by default and are run by an operator with dedicated authorized test accounts.

```sh
ORCH_SMOKE_RELEASE_A=/path/to/a/release scripts/smoke/managed-harness/run.sh
```

The release has to be one whose manifest lists the harness builds (any release packaged from this tree does), and
the host of the node needs access to the internet: the node downloads the builds from the vendors' addresses, about
420 MB the first time (a host keeps what it downloaded, so a second environment needs nothing). Besides what the
other scenarios need it needs `jq` and `node`, and builds `orch-mock-llm` with the toolchain it packages with.
Ports 41300 (the web edge, on the loopback), 41301 (the pinned listener, on the Docker bridge's address) and 41302
(the scripted model server, on the bridge's address), which `ORCH_SMOKE_PORT` moves. At the end it takes down the
stack, the machine of the person and the model server, removes the units the node made for it and their volumes, and
the directory it worked in.
