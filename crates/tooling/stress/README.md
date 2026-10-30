# orch-stress

A seeder and a driver for a **running** Orch, at a team's scale: 10 orgs, 100 projects, 100 threads taking
turns at once, 10 providers, 10 agent profiles, 10 hosts, 1000 documents, 20 people in browsers and 5 who search, and
`--scale 2` or `--scale 10` of all of it. It speaks to the system only through its HTTP API and its realtime socket, as
a browser and a person do, with JSON values: it depends on no product crate, builds while the product is being edited,
and drives an older or a newer build alike (it reads the server's own `GET /openapi.json` and does not ask a route the
build does not have; the report names any it left out).

## The bars

At the owner's scale, over the measured window:

| Bar | Judged on |
|-----|-----------|
| Reads under 300 ms | the p95 of every `GET` route |
| Writes under 1 s | the p95 of every other route |
| No errors | every non-2xx answer or failed request, every turn refused, failed, blocked or not seen to end, every socket error, drop, resync or closed feed |
| No statement over 1 s | `pg_stat_statements`: the slowest run of every statement that ran in the window (needs `--database-url`) |
| Live-feed lag under 2 s | the p95 of every kind of lag (below) |

The verdict line says PASS only when all hold; every route, page, lag and statement is listed with its numbers, so a
FAIL says where.

## What it makes (`seed`)

Through the API alone, as a system administrator (`--login`, `--password`), up to the figures asked for:

- **people** (one per browser): made by the administrator, each choosing the password through the link the
  administrator was given; each is a developer in three orgs (`--orgs-per-person`), invited and accepting;
- **orgs** `<prefix>-org-NNN` with `sandbox.mode` (`--sandbox-mode`, `open` by default: a connected machine's own
  executor enforces only that, and the scripted harness runs nothing) and, for an `http://` model upstream,
  `providers.allow_http_upstreams`;
- one **environment** per org on the agent machine, in a directory of its own made through the file API of a running
  environment there (the carrier, `--carrier`, found when left out): a request cannot make a directory on a machine
  any other way. Where the machine accepts no file written from outside (its `environment.remote_write` policy), every
  org's environment has the carrier's own directory, which exists; the scripted harness writes nothing in it;
- **providers** (API kind, `--model-upstream`, `--model-key`, `--model`) and **agent profiles** of the scripted
  harness (`fake`), spread over the orgs;
- **projects** spread over the orgs, one **topic** in each, **threads** spread over the topics (made by a member of
  the org, who works in it), **documents** spread over the projects: every fifth rewritten twice (two versions), every
  fourth with a discussion and two answers.

What was made is kept in the state file (`--state`); a later `seed` makes only what is missing, so a larger
`--scale` grows the same world. Two things are counted, never made, because no request makes them: **hosts** (a host is
a machine whose node enrolled with `orch-node enroll`) and the **machine** the threads run on (a connector that
enrolled with the agent, environment and provider roles and the scripted harness in its `agent.harnesses` policy).
`seed` says what is missing and why.

## What it does (`drive`)

For `--duration` (10 minutes by default), after a five-second start:

- **Every thread takes turns.** Its person sends an input that is a script of the scripted harness: it thinks, plans,
  runs two commands, edits a file and answers, with pauses that stand for a model's time (`--turn-seconds`, 6 in all),
  streaming items and deltas as a harness does. The person waits for the turn to end on their socket (one per person,
  following all their threads, as a tab does), pauses (`--pause`, 3 s on average) and sends the next.
- **Browsers read pages.** Each person opens pages with the reads the web application makes for them, all at once:
  a thread (30 in 100), a document (20), a topic (15), a project (10), an org (8), home (7), the inbox (5), a search (5);
  follows the page's feed while reading (`--dwell`, 3 s on average), then goes on. On a document they sometimes write
  (a paragraph, 15 in 100; an answer in a discussion, about 5 in 100); after one page in five they type a name into the
  finder, a letter at a time, and open what it names.
- **Searchers** search the words the documents and turns are written with (half of the time within an org) and use
  the finder.

## The report

- **Summary**: one line per bar, with the worst measured against it (the worst route, the worst lag) and whether it
  holds; the verdict line above it says PASS only when every bar holds.
- **Routes**: n, p50, p95, p99 and max of every route of the window, by its template (`GET /threads/{id}/sync`), with
  its bar and its errors by status and the API's code (`404 not_found x3`). Requests made while seeding and signing
  in are listed apart and not judged.
- **Pages**: the time of a page, from its first read sent to its last answered.
- **Live feed**: from the moment the writer sent its request to the first frame on a socket that carries the change:
  `thread input, the writer's own tab`, `thread input, another person's tab` (a browser that has the thread open),
  `document block` (a paragraph a browser wrote, seen on the document's feed). The socket's ping round trip is beside it.
- **Turns**: inputs sent, how their turns ended (`completed`, `failed (code)`, `blocked (why)`), refused, not seen to end
  within `--turn-timeout`; from input sent to turn completed.
- **Sockets**: connections, frames by op, drops, resyncs, error codes.
- **Statements** (with `--database-url`): `pg_stat_statements` of the system's database is read when the window opens
  and when it closes, never reset; a statement is listed when its mean in the window, or its slowest run in the window
  (known when its `max_exec_time` rose during it), is over `--slow-ms` (100). Other clients of the same database in the
  same window are counted too.
- **Database sessions** (with `--database-url`): `pg_stat_activity` sampled every two seconds through the window, on a
  connection of the tool's own: the peak of sessions active, waiting on a lock and held idle in a transaction, and the
  statements sessions were held after (idle in a transaction for over a second) or waiting behind (a lock for over a
  second), with how often and how long. Transactions older than the window (a leader's lease) are left out.
- **Notes**: what could not be made, routes this build does not have, turns not seen to end.

`--out report.json` writes all of it as JSON as well.

## Running it against a development stack

It needs a stack whose API answers, a system administrator, an online machine with the agent, environment and
provider roles that lists the scripted harness (`agent.harnesses`: `[{"kind":"fake","path":"…"}]`) and allows the
environment directory (`environment.roots`), a running environment on it, and a model upstream for the providers (the
scripted harness makes no model call; a provider has one all the same, and the testkit's `orch-mock-llm` serves).

```sh
cargo build -p orch-stress
export ORCH_STRESS_PASSWORD='…'            # the administrator's; the people it makes get it too
export ORCH_STRESS_MODEL_KEY="$(cat <the mock model server's key file>)"
export ORCH_STRESS_DATABASE_URL='postgres://…/<the system database>'   # optional: the statements

target/debug/orch-stress --api http://127.0.0.1:41280/api/v1 --login ada --state stress-state.json \
  run --scale 1 --duration 10m --model-upstream http://127.0.0.1:41285/v1 --out stress-report.json
```

`--arrangement together` names how the threads' agent is placed; left out, the system takes its first permitted
arrangement (on some builds `separated`, which a connected machine whose executor enforces only `open` refuses: the
threads then say they are blocked, and the report counts it). `--refresh-blocked` lets the administrator refresh the
provider of a thread blocked on it (a provider that fell back to needs-setup), counting each.
`ORCH_STRESS_DEBUG=1` prints every socket's connections, readiness and closings.

`seed` and `drive` are the two halves of `run`; `drive --no-turns` drives browsers and searchers only. Every figure has
a flag (`--orgs --projects --threads --docs --providers --agents --hosts --browsers --searchers`), and `--scale`
multiplies them all. A drive at a figure lower than what was seeded drives the first of what is there.

Numbers from a debug build are not a release's: a debug build of the API is several times slower, and the tool, the
stack and the database share one machine. Compare runs of the same build and kind.
