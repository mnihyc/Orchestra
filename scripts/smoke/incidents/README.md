# Incident procedure checks

`bash scripts/smoke/incidents/check.sh --stack DIR --project NAME` checks an
existing deployment's local inventory and `orchctl doctor` status. It starts,
stops and removes nothing. Use a disposable release stack for the recovery and
cleanup steps in [the incident guide](../../../docs/operator/incidents.md).

Record the release manifest/build, Compose project, selected system labels and
outcomes of these cases:

- Healthy startup gives a passing local doctor; a stopped dependency is named
  as failed and returns nonzero.
- A reset URL permits one password change, rejects reuse, and contains no secret
  in the safe audit receipt. Clearing a lost second step is an explicit choice.
- Suspending the last administrator is refused; ordinary suspension ends that
  person's sessions and resume restores their ability to sign in.
- Link reconnection catches up once; a foreign stream report changes no content.
- Updater Recover finishes a journaled operation and a repeated Recover changes
  nothing.
- Product removal/reconciliation affects only the selected runtime/system;
  retained volumes and a second disposable deployment survive the cleanup.

The writer records this procedure without running services. The release
runner retains the actual transcript and status for each case.
