# restore scenario

A system with a person in it is backed up (the dump, `config/` with the secrets, `.env`, `runtime/data`,
`runtime/node`), lost, and put back on a clean directory of the same release with
`update.sh first-start --restore-database`. The same person signs in with the same password, the authority
is the old one, and the bundled node reconnects with the identity it kept. Run it with
`scripts/smoke/restore/run.sh`.
