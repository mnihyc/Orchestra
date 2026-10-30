# Private deployment credentials

Run `bash scripts/smoke/kfix-deploy-env/run.sh` with Docker and the smoke helpers' release inputs.
The scenario uses this checkout's updater with the verified release payloads. It checks an uncorrectable
password file before any Docker call, a 0644 file corrected on first start and update, a private rollback,
and a restore that keeps the password usable by the stack and unreadable by a second account.

`cargo test -p orchctl --test kfix_deploy` checks that doctor names both an exposed `.env` and an exposed
file in `config/secrets`, prints no credential, and passes when their owner protects them.
