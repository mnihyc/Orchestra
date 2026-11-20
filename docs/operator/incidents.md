# Diagnose a problem

Find which part of an Orch installation failed, repair it without making it worse, and close the incident with proof that it is fixed. Start with the visible failure and the component behind it.

Orch runs one API process, which owns the live terminal, browser and display tickets. Keep the thread or job id, the time, the build and the component status with the incident. Credentials and reset URLs do not belong in the incident record.

## Before you start

You need shell access to the host that runs the stack, in its `deploy/` directory (Compose reads the project from there), and, for most repairs, an administrator account.

## Find your symptom

| You see | Go to |
|---|---|
| The web application does not load, or answers 502 or 503 | [Establish the state of the stack](#1-establish-the-state-of-the-stack), then [Monitor an installation](monitoring.md#what-readiness-means) |
| A machine or a thread is offline, or a turn is held | [A machine or thread went offline](#a-machine-or-thread-went-offline) |
| A provider shows missing setup, offline, exhausted or refused | [A provider needs attention](#a-provider-needs-attention) |
| Nobody can sign in | [Nobody can sign in](#nobody-can-sign-in) |
| The disk is filling | [Storage pressure](#storage-pressure) |
| An update stopped halfway | [An update was interrupted](#an-update-was-interrupted) |
| Notifications do not arrive | [A notification failed](#a-notification-failed) |
| The HTTPS front misbehaves | [Run the HTTPS front](edge.md#if-it-does-not-work) |

## 1. Establish the state of the stack

From the deployment's directory, use its own Compose project:

```sh
docker compose ps
docker compose exec -T api /opt/orchctl/orchctl doctor
./update.sh doctor                    # the same, and the edge from this host, once
docker compose logs --since 15m api gateway worker node
```

- `doctor` reports each check and exits nonzero if one failed. Add `--url` to check the real HTTPS front, large bodies and idle sockets ([Check the front](edge.md#6-check-the-front)).
- The services log to Compose. Logs rotate at 10 MiB, five files each. The 15-minute window is a starting view: widen it to the time the problem began.
- Readiness proves neither that a model answered nor that an outside notification was delivered.
- `scripts/smoke/incidents/check.sh --stack DIR --project NAME` makes the same read-only inventory of Compose and `doctor` for an existing deployment. It starts no service. Its output is one piece of evidence, not a repair.

## A machine or thread went offline

1. Read the machine's page: its connection, facts, certificate and local policy. Then read the thread's binding and its reason for being blocked.
2. Wait for the connector to reconnect: it continues with its recorded stream position and command journal. Do not enroll it again as a new machine to repair a short outage. **Enroll again**, when the page offers it, keeps the registered identity. An expired or revoked certificate needs the enrollment or renewal for that machine ([Connect a machine](../user/connect-a-machine.md)).
3. If a turn is held, look at the pending inputs and the job that holds it. **Stop** holds the queue and **Continue** releases it.
4. If a stop failed, the thread says so, and a process may have survived. Check it in the running-work, process and terminal views before you retry anything that has an effect. A remote command you retry must keep its original identity so the connector can recognize the repeat.

Only the machine that owns a stream can update it. A foreign report is acknowledged and discarded, and leaves a short administrative record. If events arrive late, check the stream's recorded machine identity. Do not weaken the source check to repair it.

On Windows, ending a process tree checks the observed child processes before it ends their parent. It leaves out a child it cannot identify and refuses the stop if the recorded times cannot tell a child from a reused process number. A child created during the final check can escape; look at the machine's processes if the stop overlapped new work.

## A provider needs attention

The provider's page tells missing setup, offline, exhausted and a refusal by the vendor apart. Read its credential expiry, its plan windows and its last safe error. An unknown or unavailable figure never means zero credit.

- A subscription renews where its sign-in is kept. A fresh token from outside is imported through the provider's normal method. Keep one renewing copy of a login where its method needs that.
- A handover of a whole sign-in must be returned before its environment can stop. If the return fails, the refusal stays visible; the manager can repair it or declare the sign-in lost, which says what that does.
- Only the provider's manager can use a reset, after the browser's confirmation names it. Retrying the same request does not use another one.

## Nobody can sign in

If the person's normal recovery cannot work, create a one-use reset link on the host:

```sh
docker compose exec -T api /opt/orchctl/orchctl admin reset-link operator
```

1. Replace `operator` with the account's username.
2. Give the printed URL to that person privately. Do not copy it to the incident log.
3. Add `--clear-second-step` only if the second step must also be removed.

Creating the link leaves an audited receipt, and using it consumes the token. A suspended person stays suspended until the suspension is lifted. The host command also has `admin grant`, `revoke`, `suspend` and `resume`; it refuses to remove or suspend the last system administrator who can sign in. Run it with the deployment's configuration and secrets, and record the act. An automation key still needs its selected operations and the account's current permissions, and cannot gain the browser's private-access override through this recovery.

`admin human-check off` switches the [human check](human-check.md) off, as the system, when its service cannot be
reached and nobody can get past it.

## Storage pressure

1. Look at the affected environment or provider, the job and the host before you reclaim anything. Workspaces, homes, provider state, browser profiles and mounts have different owners. A database backup does not include every host's directories. A removed environment may keep its volumes until its purge date on purpose, so that they can still be inspected and restored.
2. Use the product's **Remove** or **Purge** or the update recovery, with the scope it shows ([Clean up storage and uninstall](storage-cleanup.md)).
3. To list the containers of one system on a host, filter by label:

   ```sh
   docker ps -a --filter label=orch.system=example --format '{{.ID}} {{.Names}}'
   ```

   Replace `example` with the deployment's system name, and compare the `orch.runtime_id` of each with the server's resources. Containers carry `orch.system`, `orch.runtime_id` and `orch.kind`.

Never reclaim storage with a whole-host prune: the Docker daemon can hold other deployments and retained work. Reconciliation of orphans and explicit volume cleanup protect the references of a live successor. Ownership and recovery are described in [Back up and restore](backup-and-restore.md) and [Update and roll back](upgrades.md).

## An update was interrupted

Keep the update's journal and run the updater's recover operation (`./update.sh recover`). Do not move payload links by hand while a transaction is open. The release manifest checks the payload hashes; the stock image digests and the platform and dependency checks have their own. A release you packaged yourself is separate from one fetched from a release site. See [Update and roll back](upgrades.md#after-an-interruption).

## A notification failed

Look at the notification's route and channel and its safe delivery receipt. Retryable, gone, rejected and uncertain outcomes have different effects. The worker applies the private-host policy at each connection and each redirect. Fixing the channel does not invent evidence of an earlier delivery.

## Close the incident

Write down the build you checked, the cause you saw, the exact repair, and a repeated visible result showing it works. On a disposable deployment you can rehearse the four repairs: the one-use reset link, a reconnect, the updater's recovery, and a label-scoped cleanup that leaves another deployment untouched. A written checklist alone does not count as having tried them.

## Report a screen problem

If a screen opens slowly or a card does not fit, report the build, the screen, the language, the theme and the viewport width, and compare a fresh opening with a return from another screen. Include the affected control, whether advanced options were folded, and what you did with the pointer and the keyboard. For slow opening, keep request methods, route names, status and timing from the browser's network view, and remove tokens, cookies and private content. Do not turn authority checks off to speed a screen up. [Reading views and details](../user/reading-views-and-details.md) describes how screens behave.
