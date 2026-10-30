# orch-worker

Database and outside-world delivery/upkeep; no machine orchestration.

The program reads `orch.toml` and the secrets directory (`--secrets-dir`,
`ORCH_SECRETS_DIR`): the master key that opens what is sealed in the database
and, where the deployment has one, the key that signs Web Push messages. It
waits for the API to bring the schema up to date, then delivers the outbox
through the mail, webhook, Telegram and Web Push notifiers and runs the
upkeep, each job on its own interval (`[schedule]`) and under its own lock, so
that any number of workers may run. It reports itself in `component_heartbeats`,
serves what it measures at `[worker] metrics_bind` (`/metrics`), and, when asked
to stop, finishes the delivery it is making before it ends. The jobs of the
documents, the rate catalogue and the engine are not composed yet.
