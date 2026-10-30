# Ports on the released server

`bash scripts/smoke/ports-view/run.sh` uses the owned released stack fixture from
`../link-load/world.sh`. Supply `ORCH_SMOKE_RELEASE_A` and an already-built
`ORCH_SMOKE_MOCK_MODEL`, plus the frontend's Playwright and Chromium installation.
It never packages or builds implicitly. Default stack/front/model ports are
41460–41463; concurrent runs select a different `ORCH_SMOKE_PORT`.

Chromium signs in to the candidate's actual frontend. With no intercepted requests,
it shows an unlisted target, proves the read inserted no opening, reads its stable
address again, grants `port_place` on the panel and checks the actual producer names
the giver, then takes it back. A grant itself opens nothing. Both mutations refresh
the panel without an explicit Refresh. The connected fixture has a real native
reader; its scripted model is an explicit double and is not involved in these acts.

The retained result, screenshot, exact candidate manifest, dependency evidence,
logs and phase exit live under `.tmp/deploy-smoke/evidence/<run>/ports-view`.
This walk is distinct from `frontend/tests/e2e/ports.spec.ts`, which uses the
component double world. Neither a written runner nor that double is a successful
released-stack proof. Host TCP/UDP traffic, last-hop PROXY and live record reads
have separate Rust integration cases; the inside-only peer endpoint remains a
separate requirement.
