# Operate skills and live work

## Skills

Skills are versioned server content. Published versions include a file manifest
and stored blobs; backups must keep the database and its corresponding blobs.
Use the normal backup and restore procedure. A draft is separate from a version.

Give skill management only to people trusted to publish executable content.
Use permits enablement, not editing. Review imported scripts and their origin,
then test in an ordinary bounded environment without production secrets.
Private source credentials are sealed; diagnose imports from the safe error,
host readiness and allowed network route rather than printing credential values.
For an archive imported from an address, the system setting
`outbound.allowed_private_hosts` names private hosts the chosen host may reach.
Direct connections check each redirect. If the node uses a proxy, configure the
proxy's own address restrictions as well.

An enabled update reaches a thread before its next turn. If Updating skills
persists, inspect the thread's work detail and its machine connection. Check
the selected published version, agent compatibility and delivery path. Do not
copy skill files into a provider's shared home to bypass a failed refresh.

## Counts, copies and cancellation

The plan measures selected locations in background jobs. Unknown totals remain
unknown until the read completes. A copy's live phase, byte/file counts and
last-change time describe actual reported work. Estimated rate and remaining
time do not establish readiness.

A cancel request is not proof of stopped I/O. Stopping remains until each
machine acknowledges its cleanup. For a stalled stop, inspect the named node
and command logs and retain the job identity. Do not start another operation
on the same data while the first is still stopping. Read the job's commit
outcome before deciding which location is authoritative.

Synced-copy Check is an explicit read of source and destination content. Its
result is dated and does not synchronize either side. A failed or cancelled
check has no equality result. Check the native read error and current access
before retrying; do not treat a missing fingerprint as matching.

## Server database updates

The edge can remain up while the API migrates. The browser's reconnecting
banner reports the active migration, steps, available row counts and elapsed
time from the update feed. A row count can be unavailable. If the update fails,
use the server's update log and the upgrade recovery procedure before restarting
services. Keep the paired database and blob backup for recovery.
