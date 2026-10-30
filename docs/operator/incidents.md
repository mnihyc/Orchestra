# Incident diagnosis

Start with the visible failure and the component responsible for it. Keep the
thread/job id, time, release/build and component status with the incident;
credentials and reset URLs do not belong in the incident record. This release
runs one API process, which owns live terminal/browser/display tickets.

## Establish the deployment's state

From the deployment's directory, use its own Compose project:

```sh
docker compose ps
docker compose exec -T api /opt/orchctl/orchctl doctor
docker compose logs --since 15m api gateway worker node
```

The log window is a starting view of the incident, not a retention policy;
expand it to the observed start time when needed. `doctor` reports each check
and exits nonzero for a failed one. Its optional `--url` checks the actual HTTPS
edge, large bodies and idle sockets. Follow [the edge guide](edge.md) for that
invocation and its current limits. Readiness does not prove a model answered
or an external notification was delivered.

`scripts/smoke/incidents/check.sh --stack DIR --project NAME` performs the
read-only local Compose/doctor inventory. It requires the existing deployment
and starts no services. Its output is one piece of evidence, not a repair.

## A machine or thread went offline

Read the machine page for its current connection, facts, certificate and local
policy, then the thread's binding and blocked reason. A connector reconnects
with its recorded stream/cursor and command journal. Do not enroll it as a new
machine to repair a short link outage: Enroll again, when offered, preserves
its registered identity. An expired/revoked certificate needs the enrollment
or renewal procedure appropriate to that machine.

Only the machine owning a stream can update it. A refused foreign report is
acknowledged and discarded and leaves a bounded administrative record. Check
the segment's immutable harness-machine identity when diagnosing late events;
do not repair this by weakening source checks.

If a turn is held, inspect pending inputs and the job that holds it. Stop holds
the queue; Continue releases it. A native command may survive a failed stop,
which the thread reports. Use its running-work/process/terminal views to check
that process before retrying an effect. A retried remote command must retain
its original identity to use the connector's deduplication.

## A provider needs attention

The provider page distinguishes missing setup, offline, exhausted and a vendor
refusal. Read its actual credential lapse, plan windows and last safe error.
An unavailable or unknown fact does not mean zero credit. A subscription's
renewal happens where that sign-in is kept; an external fresh token is imported
through the provider's normal method. Keep a single renewing login copy where
the method requires one.

An exclusive handover must be returned before its environment can stop. A
failed return keeps that refusal visible; the manager can repair it or use the
explicit declaration of loss, with its named effect. A provider reset is only
for its manager after the browser confirmation names the reset. A retry of
its request identity does not consume another reset.

## Nobody can sign in

If the person's normal recovery cannot work, a host operator can create a
one-use reset URL through the same server library:

```sh
docker compose exec -T api /opt/orchctl/orchctl admin reset-link operator
```

Replace `operator` with the account's actual username. Give the printed URL
privately to that person; do not redirect it to the ordinary incident log.
Add `--clear-second-step` only when that person's second step must also be
removed. Creating the link leaves a safe audited receipt, and using it consumes
the one-use token. A suspended person remains suspended until the suspension
is lifted.

The host CLI also supports `admin grant`, `revoke`, `suspend` and `resume`.
It refuses removing or suspending the last system administrator who can sign
in. Run it with the deployment's configuration and secrets, and record the
actual administrative act. Host recovery is not a way for an API token to gain
browser-only administrator authority.

## Storage pressure and cleanup

Inspect the affected environment/provider, job and host before reclaiming data.
Workspaces, homes, provider state, browser profiles and mounts have distinct
custody. A database backup does not include every host's directories. A removed
environment may intentionally retain volumes until its purge date; those bytes
can still support authorized inspection and restore.

Use the product's Remove/Purge or update-recovery action with the displayed
scope. For host inventory, units carry `orch.system`, `orch.runtime_id` and
`orch.kind`. Select only the system being diagnosed:

```sh
docker ps -a --filter label=orch.system=example --format '{{.ID}} {{.Names}}'
```

Replace `example` with that deployment's system name and compare its runtime
ids to the server's resources. Orphan reconciliation and explicit volume cleanup
protect live successor references. Do not reclaim storage with a whole-host
prune: the Docker daemon can contain unrelated deployments and retained work.
Use [backup and restore](backup-and-restore.md) and [upgrades](upgrades.md) for
their ownership and recovery rules.

## Updates, dependencies and delivery failures

If an update was interrupted, retain its journal and run the updater's Recover
operation. Do not manually retarget payload links while a transaction is open.
The release manifest verifies payload hashes; stock image digests and the
native/platform/dependency gates have their own checks. A locally packaged
release is separate from an artifact fetched from a release site.

For a failed notification, inspect its route/channel and safe delivery receipt.
Retryable, gone, rejected and uncertain outcomes have different effects. The
worker honors the configured private-host policy at each connection and
redirect. Fixing the channel does not fabricate proof of an earlier delivery.

Close an incident with the candidate build, the observed cause, the exact
repair and a repeated visible outcome. On a disposable deployment, verify the
reset-link's single use, a reconnect, updater recovery and that label-scoped
cleanup leaves another deployment untouched. Those are the operator procedure's
acceptance checks; a written checklist alone is not a passing run.

## A screen opens slowly or a card does not fit

Record the build, screen, language, theme and viewport width. Compare a fresh opening with returning
from another screen. In browser network diagnostics, separate data reads from static assets and sign-in.
Keep request methods, route names, status and timing; remove tokens, cookies and private content.

The first thread or resource screen shares its opening reads. Closed navigation and Documents views
wait until shown. An expanded navigation can read more than one bounded page. A retained label or an
already open card never replaces the current reader's access check. Follow a refusal in the interface;
do not repair slow opening by disabling authority checks.

For slow role counts, capture the query plan and buffer reads at the reported audience size.
Compare the first read on a new database connection with repeated reads on that connection.
Record machine load with each measurement. Repeated indexed reads can still cost time; a plan
with no disk reads does not prove that the query is cheap. Apply the forward database migrations
so existing installations and fresh databases use the same holder expansion.

Cards, menus and help use the available viewport and scroll internally when long. Reproduce both
pointer and keyboard behavior. Check the whole value on hover or focus, Escape and return focus.
Include the affected control and its first name line in a layout report. Record whether advanced
options were folded or open. Controls inside folded options become reachable when the section opens.
Literal source lines and an author's repeated document heading are authored data.

Escape closes a full-value tooltip before the inspection beneath it. A further Escape closes the
inspection. The underlying dialog stays open until its own dismissal.

A tab keeps its last read name while its page reloads. Include the page heading and the tab in an
identity report, and say whether the page was still loading or had failed. A failed read keeps its
current unavailable state; a retained name does not enable its actions.
The active tab also shows its name. A narrow strip may truncate it; an ample strip does not hide it
because the page has a matching heading.

See [Reading views and details](../user/reading-views-and-details.md) for the current screen behavior.
