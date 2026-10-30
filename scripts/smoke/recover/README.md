# recover scenario

The updater is stopped dead (`ORCH_UPDATE_CRASH_AT=<step>` kills it right after it recorded the step, as a power
cut would) at three steps, and `update.sh recover` is run: before any link moved (the update is undone and the
old release runs, with no previous recorded); with the new release started and not yet judged (the update is
finished and recorded); and, in a rollback, with the database about to be replaced (the rollback is undone and
the database is as it was). It also shows that another update is refused while a transaction is open, that
`status` says so, and that `recover` with nothing open changes nothing.
