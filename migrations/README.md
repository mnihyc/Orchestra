# Database migrations

These files make the PostgreSQL 17 schema of a fresh Orch database. The server applies them in order, each in one
transaction, when it starts (`crates/control/db/src/migrations.rs`). Typed queries, transactions and seeding live in
`crates/control/db`.

The first release ships the schema as one structured set, one file for each area. Each table is written once, in its
final form: its columns, primary key, unique and check constraints, and its references to tables made earlier, followed
by its indexes, triggers and comments. A reference to a table made in a later file is added right after that table.

| File | What it holds |
|---|---|
| `0001_foundation.sql` | The `pg_trgm` extension, the functions that tables, triggers and the engine call, and three aggregates. Function bodies are checked when they first run, so a function may name tables from later files. |
| `0002_identity.sql` | People: users, their identities, profiles and state, sign-in (identity providers, codes, second steps, throttles, known devices), sessions, API and access tokens, invitations, enrollment tokens and public names. |
| `0003_organizations_and_places.sql` | Organizations and their members, the tree of places (`nodes`: projects, topics, threads), roles held in places, groups, tags, favorites and recents. |
| `0004_access.sql` | Permission rows and entries, authorization epochs and the commit work that keeps them consistent, access changes, shares and the audit trail. |
| `0005_settings_and_operations.sql` | Settings and secrets, jobs and their leases, idempotency keys, plans waiting for confirmation, installation records and component updates. |
| `0006_files.sql` | Blobs and uploads, snapshots and the file cache. |
| `0007_programs_and_providers.sql` | Agent programs, their builds and reports, harnesses, agents and profile pins, providers and their models, skills, capabilities, summaries and model call recovery. |
| `0008_environments_and_machines.sql` | Hosts, machines and runtimes, environment templates and environments, mounts, MCP servers, ports, terminals, displays, browsers and commands. |
| `0009_threads.sql` | Threads: turns, items and their revisions, segments, inputs and schedules, approvals and their rules, goals, continuations, prompt versions, tool calls, history cuts and native guards. |
| `0010_chats.sql` | Chats, their members and messages, and what messages refer to. |
| `0011_documents.sql` | Documents: blocks, operations, versions, comments and discussions, files, containers and tree operations, summaries and notices. |
| `0012_links_and_search.sql` | References between things and related items, and search: sources, chunks, generations, sessions and the views it reads. The optional pieces are below. |
| `0013_notifications.sql` | Announcements, notifications and their operations, channels, routes, destinations, preferences, subscriptions and the outbox. |
| `0014_usage.sql` | The usage ledger, its daily and monthly rollups, caps and their notices, bypasses, the rate catalogue and resource use. |
| `0015_imports_and_exports.sql` | Packages exported and imported, what each import made (`import_origins`, so that a later import updates instead of duplicating and an import can be undone) and its report. |
| `0016_metrics.sql` | Stored metric samples, in partitions by day, and the hours they fold into. |

## Optional pieces

Two parts of search depend on what the database offers, and are decided when the schema is made, not written in:

- **Search by meaning** needs pgvector (the extension `vector`). `orch_search_vectors_setup()` creates its tables
  where the extension can be had, now or after an operator installs it later. Where it cannot, the migration says so
  once, and everything else works.
- **Short text** uses native bigrams where `pg_bigm` can be had, and portable exact Unicode grams everywhere else.

## Changing the schema

At each release tag, the release integrator records `<SHA-384 hex>  <filename.sql>` for every released file in
`RELEASED`, and the `orch-db` regression checks those files' exact bytes. A released file is never edited: a later
change to the schema goes into a new numbered file. New migrations must keep data, survive concurrent writers and
recover from an interrupted upgrade, and their tests must prove it on a database with rows.
