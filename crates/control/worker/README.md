# orch-worker-core

Database and outside-world delivery and upkeep; no machine orchestration.

The worker's deliveries and its upkeep are implemented here, side threads that nobody used among them. The outbox delivers its three kinds of row (a notification for a
person's route, one for a destination, and the mail that signing in writes)
through the notifier of their channel, retries on the schedule and gives up at
its end; a row is claimed by a lock that its transaction holds while the
notifier works, so two workers never take one and a dead worker's claim is
taken up. Routes, quiet hours, bot links, the purges, the retention jobs, the
certificate reminders, startup recovery and the heartbeat each run under a lock
of their own on the schedule of `schedule`, and so do the two jobs of usage: once
a day the rate catalogue is fetched from the one address a system admin set (no
redirect, at most 2 MiB), only when the days of the setting have passed, and
loaded only when its figures are newer, checked whole as an upload is; and every
hour the minute values of the metrics that have ended are folded into the hours
that are kept; and once an hour a managed provider whose runtime has been stopped for as many days as its setting says is given the job that starts the unit so that its harness renews the sign-in it holds (the engine carries it out; the worker starts nothing). Every ten minutes the archives of a place whose time has run out are removed (their bytes let go for the sweep of the store), and so are those of a person who is gone, whose row has no person left, at once; an archive whose place was purged is its person's until its time. The same run removes what an archive that was never finished left in the scratch directory of the store (`tmp/` below the storage root): the zip of a job that does not run any more, and the zip of a chat with its files, once the file is a day old, and nothing else in that directory. The rows of those removed or failed a month ago are forgotten. Every hour the commands to machines that ended more than a day ago are removed, a bounded number at each run, but for the kinds whose rows are read back as what a unit or an environment was last given, and those of a job that is not over. All of it is tested against a real database, with notifiers that answer as they are told to, the system's mail
channel against an SMTP server in a container, and the clock moved by hand.

Document notices recheck their recipient's current reading before delivery, with the shared authority gate taken before any claim. A real webhook request case verifies that a pending notice reveals no document words after reading is withdrawn.

| Module | Planned responsibility |
|--------|------------------------|
| [outbox](src/outbox.rs) | Claim transactional outbox rows and deliver each through its notifier, with retry, and a route or a destination switched off when its target is gone. |
| [routes](src/routes.rs) | A person's routes through channels: what waits for an unread delay or a frequency limit, sent as one message when its time has come, and what quiet hours held. |
| [bot_links](src/bot_links.rs) | For a channel whose kind links people: ask who followed a link, only while a code waits, and link the route. |
| [channel_setup](src/channel_setup.rs) | A channel of the system set up from its fields on the host, and a sample sent through it with no person behind it. |
| [channels](src/channels.rs) | The kinds of channel there are, built in and installed, and the channel a delivery goes through: the system's, a root org's or a person's own. |
| [render](src/render.rs) | A message as its notifier gets it: the notifications of a row, the text of the channel or the one the notifier ships, filled in for the language and time zone of the person, and cut to the length a message takes. |
| [system_mail](src/system_mail.rs) | The mail that concerns a person's own user: what signing in writes into the outbox, the shipped text of the person's language, and the blanking of what must not stay once it is delivered. |
| [quiet_hours](src/quiet_hours.rs) | Recipient-local quiet windows. |
| [schedule](src/schedule.rs) | Worker-owned schedule and per-job database locks. |
| [document_versions](src/document_versions.rs) | Cutting idle document versions through the document service's existing locked history path. |
| [document_purge](src/document_purge.rs) | Pruning document history and expired editing records while retaining every referenced file pin. |
| [purge](src/purge.rs) | Expired tokens, sign-in codes, browsers nobody signed in from for their time, counts of wrong tries whose window has passed, idempotency keys and soft-deleted database rows; an org's, a provider's and an environment's only once nothing of it is left on a host; a document's removed blocks and the versions past those it keeps. |
| [command_purge](src/command_purge.rs) | Commands to machines removed a day after they ended, a bounded number at each run. |
| [exports](src/exports.rs) | Archives of what a person may read in a place, removed when their time has run out or their person is gone, what an unfinished archive left in the scratch directory removed once it is a day old, and the rows of those that were removed or that failed forgotten after a month. |
| [retention](src/retention.rs) | Raw-event partition retention, the minutes of stored metric samples once their hours are folded, and recents trimming. |
| [catalogue](src/catalogue.rs) | The rate catalogue fetched from where a system admin said, on its schedule, and checked whole before it replaces the one in force. |
| [releases](src/releases.rs) | Optional bounded checks of the updater's configured release source, preserving dated facts on failure and one local notice per new version; checks ship off and install nothing. |
| [backups](src/backups.rs) | Publish and reconcile protected metadata from the existing completed database dump/manifest pair; metadata repair never takes another dump. |
| [backups::inventory](src/backups/inventory.rs) | A typed restore inventory from the same identity- and hash-checked manifest as the backup record. |
| [backups::storage](src/backups/storage.rs) | The updater's protected storage snapshot is read-only input, never permission to delete server files. |
| [side_threads](src/side_threads.rs) | Side threads closed when nobody used them for as long as their place says. |
| [rollup](src/rollup.rs) | The fold of metric samples into hours, which are kept. |
| [provider_renewals](src/provider_renewals.rs) | Starting a stopped provider's runtime so that its harness renews the sign-in it holds, once it has been stopped for as long as the setting says. |
| [reminders](src/reminders.rs) | Certificate expiry notification. |
| [heartbeat](src/heartbeat.rs) | Worker health, delivery metrics, and the worker's own samples of them. |
| [gauges](src/gauges.rs) | The operator's gauges the database answers (threads, jobs, runtimes, open commands, the blob store), set at the heartbeat, a series gone standing at 0. |
| [reconciliation](src/reconciliation.rs) | Recover abandoned delivery claims. |
| [spans](src/spans.rs) | Spans of time read from the configuration, bounded so that no number written there can overflow a moment. |

The minute sampler also observes harness processes, cap/lock waits, catalogue age and counted
usage by purpose and rate source. Usage totals read only committed counted ledger rows; tracing
duplicates never enter them. Credits are converted to floating point for observation only. The
ledger aggregation is one grouped read per minute; its cost on a long production ledger belongs
in the load evidence. Filesystem free bytes are the available room shared by the pinned/cache
classes. The API evaluates cap standings with its engine and stores its own process observations;
the gateway sends its aggregates through the existing internal connection, without a database.
