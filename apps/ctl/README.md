# orchctl

`commands/backup.rs` supplies the updater's database-only dump/manifest writer and metadata-only record repair. It holds one exported PostgreSQL snapshot for both inventory and pg_dump, validates the custom dump, publishes a protected manifest-led Overview record, and names excluded host bytes. `release-source` reads the same configured source as the API/worker without contacting it.

Administrator, bootstrap, diagnostics, consumption, channel and plugin commands, and one request to the API with a token.

`orchctl` is the operator's command line, one program. A command either works
on the host, against the database and the files of the deployment (marked
**host** below), or is a client of the API with a token, like any automation.
Host commands are run where the stack runs, such as `docker compose exec api
orchctl …`. They never start or stop a service. They read the configuration
the API reads (`--config`, default `/etc/orch/orch.toml`, or `ORCH_CONFIG`),
whose `database.url` may be given by `ORCH_DATABASE_URL` as everywhere else,
and, where a command seals or opens a secret, the directory of the
deployment's secrets (`--secrets-dir`, default `/run/secrets`, or
`ORCH_SECRETS_DIR`). Each does what the matching request of the API does by
calling the same functions of the same libraries; it only has no person signed
in, so what it writes in the audit trail is done by the actor `system`.

| Command | Where | What it does |
|---------|-------|--------------|
| `orchctl version [--json]` | anywhere | Shows the identity compiled into the program. |
| `orchctl provider login <id> --method <method> --field <secret-field> [--cacert <file>]` | anywhere | Reads an imported credential from standard input, signs the provider in with `ORCH_URL` and `ORCH_TOKEN`, and prints its sign-in state. The secret is never a command argument. |
| `orchctl compose-secrets --secrets-dir <dir>` | host | Makes everything a deployment's secrets directory holds, once: the certificate authority, the master key, the Web Push key, the first certificate of each service and the token of the bundled node. A directory that holds the whole set is left as it is, and one that holds only a part is refused. |
| `orchctl admin reset-link <username> [--clear-second-step]` | host | Prints, alone on a line, an address good for one use and for half an hour at which that person sets a new password: for when nobody can sign in and mail is off. With `--clear-second-step`, using it also switches the person's second step off, for somebody who lost their phone or their mailbox. |
| `orchctl admin grant <username>`, `revoke <username>` | host | Makes and unmakes a system admin. The last system admin who can sign in is never unmade. |
| `orchctl admin suspend <username>`, `resume <username>` | host | Suspends a person altogether, or lifts that: they can no longer sign in anywhere, every session of theirs ends now, their API tokens are refused while it lasts, and what they own stays theirs. The last system admin who can sign in is never suspended. |
| `orchctl usage catalogue <file>` | host | Loads a rate catalogue from a file, checked whole as an upload is: a file with one bad entry is refused, naming the entry, and changes nothing. It is in force at once, whatever it replaces and whatever its date, and the audit trail has `rate_catalogue.loaded`. |
| `orchctl usage catalogue --shipped [--model-rates <file>]` | host | Puts back the catalogue the release came with, which it looks for as `share/model-rates.json` beside the release's `migrations/`, then beside the program, then where a checkout has it. |
| `orchctl usage rebuild [--from <date>]` | host | Makes the usage rollups again from the ledger alone: every daily row from that day on (all of them without a day) and every monthly row from the month it is in. It is the very rule that keeps them as rows arrive, so what it writes is what the live tables would hold; it runs in one transaction in which the writers of the rollups wait. Nothing in the ledger changes. |
| `orchctl channel put <kind> --file <file> [--name <name>]` | host | Makes a channel of the system, such as its mail server or its bot, from a JSON file of the fields its kind asks for, the secret ones among them; or replaces the one there is. The fields are looked at as the kind asks before anything is written: what the far side refuses for good (a wrong password) refuses the save, and a server that cannot be reached now does not, and is kept with that as the channel's error. The secret fields are sealed under the master key as they are stored. A kind that can be several, such as a webhook, is told apart by `--name`. |
| `orchctl channel test <kind> [--to <address>] [--name <name>]` | host | Sends one sample through that channel of the system and prints what the far side answered. A mail server needs `--to`; a webhook posts to its own address. A bot and push reach a person through what they linked or gave, so their sample is sent from that person's own settings. The status is not zero when the far side did not take it. |
| `orchctl api <METHOD> <path> [--data <json>] [--cacert <file>]` | anywhere | Makes one request to the system whose address is in `ORCH_URL`, with the API token in `ORCH_TOKEN`, and prints the answer as it came back, whatever the status. The base path `/api/v1` is put in front of a path that does not begin with `/api/`; `--data` is the JSON itself, `@file`, or `-` for standard input. A redirect is not followed, plain `http://` is taken only for localhost, and `--cacert` adds an authority to trust, such as the one `compose-secrets` makes, for a system under its own. The status of the program is 0 for an answer in the 200s and otherwise the first digit of the status (3, 4 or 5); 1 when no answer came. |

A refusal that a person can cause is one line on standard error that begins
`orchctl:`, says what went wrong and what to do about it, and leaves the status
1; a program that is asked for something it cannot do does not print the text
of a panic.

`plugin check --fixture <file> <command…>` runs the transport and the implementation's contract suite,
including provider sources. `plugin add --name <name> --config-file <config.toml> --fixture <file> <command…>`
checks a connector program before appending its registration, preserving the machine's settings and comments.
An unsuccessful check leaves the file intact. Restart the connector to load a new registration.

`plugin check --address <url> --token-file <file> --fixture <file>` checks a provider address through the
connector's production source binding. Use `--auth-header <name>` for a named header, `--allow-http` to permit
unencrypted HTTP for this check, and `--send-credential` only to give that address the disposable fixture's key.
Tokens are read from files and never printed or saved in a program registration. Reset effects require an
explicit disposable reset in the fixture. An address has no credential presenter.

Every host command is tested as a person runs it: the built program against a
real PostgreSQL, with a reset link used through the API and shown to work once,
a suspended person's session and token refused, a channel's secret shown to be
in no row in the clear and to open only with the master key, and the samples of
`channel test` sent to a stand-in mail server that plays its side of the
conversation and a stand-in receiver of webhooks. `api` is tested against a
server on a loopback port, the API itself and a stand-in for the answers an API
does not give.

`orchctl doctor` checks a stack and the edge in front of it, and says for each check what it tried and what came
back. Run where the stack runs (`docker compose exec api orchctl doctor`, where the configuration, the database
address and the secrets are) it tries the configuration, the database, the API on its own listener with its
certificate checked by the name `api`, the gateway's heartbeat (which is also the proof that the backplane to the
API is up), the worker's and the node. With `--url` it tries the edge from wherever it is run: a page, an API call,
a realtime WebSocket held idle, a body of 8 MiB, the machines' upgrade, and one connection to each advertised
endpoint under the trust it is listed with. Its exit status is 1 when a line says `FAIL`, which is what `update.sh`
and the API's health check wait on. It is tested as a person runs it, against a real database, a loopback API under
a real authority, and a loopback stand-in for an edge. `orchctl checksum FILE…` prints the BLAKE3 hash of files as
`b3sum` does, for a host that has none: the updater verifies a release with it.


| Module | Responsibility |
|--------|----------------|
| [commands/provider](src/commands/provider.rs) | Provider credential imports from standard input through the ordinary authenticated sign-in request. |
| [commands/operator_storage](src/commands/operator_storage.rs) | The updater owns release and dump files; its protected report grants the API no file-writing authority. |
| [commands/plugin](src/commands/plugin.rs) | External plugin behavioral conformance checks, by the suites of orch-conformance. |
| [commands/plugin_address](src/commands/plugin_address.rs) | The CLI's provider address checks through the connector's bounded and authorized address client. |
