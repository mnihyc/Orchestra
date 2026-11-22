# Review captures of a running world

A tool that walks a running Orch and saves what a person would see: screenshots of every screen at two sizes in both themes, contact sheets to review them, and a receipt of the requests each screen makes. It reviews a world you already have. It starts no server, runs no build, resets nothing, intercepts no API and makes no application state. It runs one browser page at a time, and the normal CI suite does not run it.

## Run it

You need Chromium and the frontend's dependencies installed (`npm ci` in `frontend/`), a running world, and an input file (below). From `frontend/`, with the passwords already in the three environment variables the input file names:

```sh
ORCH_CAPTURE_URL=http://127.0.0.1:41800 ORCH_CAPTURE_CONFIG=.tmp/review-capture.json npx playwright test --config=e2e/capture/playwright.config.ts
```

`ORCH_CAPTURE_URL` is the world's origin and overrides `baseURL` in the input. `ORCH_CAPTURE_OUTPUT` overrides the output directory. The tool prints its planned number of screens and the lower bound `count × 30 s` before it visits anything. A full walk is deliberately large, and another language doubles it. It does not shorten the idle window or open parallel browsers to go faster.

## Write the input

Use `capture.example.json` as the format and keep your real file under the ignored `.tmp/`. Its paths show the fake world's identities: **replace them with the paths of the real review fixtures in your world.** One `project` path covers all project views and settings sections, one `provider` path covers its tabs, and so on. `TARGET_SEEDS` in `manifest.ts` documents the keys. Naming fixtures keeps the tool from choosing an arbitrary first provider or an empty thread. It never creates a missing fixture or substitutes a fake answer, and a real permission refusal stays visible in its screenshot.

- `people` gives the login names of three people (Ada, Vera, Ben) and the **names of environment variables** that hold their passwords. Passwords never go in the JSON, in an argument or in a label. The tool signs in through the real form. Only password sign-ins work: a second step or a refusal is a named failure.
- `replacements` maps path segments or ids in scope and item queries. `routes` maps an exact inventory address to a local one and wins over the target mapping. An address cannot leave the origin.
- `dialogs` adds sequences that open a form or a picker, never one that confirms:

  ```json
  {
    "id": "environment-restart-review",
    "label": "Environment restart review",
    "url": "/resources/environments/REAL-ENVIRONMENT-ID",
    "clicks": [{ "role": "button", "name": "@environmentRestart.action" }],
    "dialog": "*"
  }
  ```

  Each click has a role and an exact accessible name. `startsWith: true` handles names that contain a place's title, and `within` scopes a repeated control to a selector. `@catalogue.key` reads the shipped catalogue. `dialog: "*"` requires an actual visible dialog. Never end a list with Create, Restart, Delete, Save, Send or any confirming act.
- `states` adds address-only states (`id`, `label`, `url`, an optional expected `dialog`), such as `/thread/<id>?item=<item-id>` or a document's `?version=2`. Take the ids from your world.
- `"locales": ["en", "zh-CN"]` captures both catalogues; English is the default. Both themes always run.

## What it captures

- Every entry of `../layout-routes.ts`, the one route inventory of the [layout suite](../README.md), including every settings section and resource tab.
- More address states: organization members and activity, project activity, done and archived, topic tabs, a thread turn's permalink and its catch-up dialog, inbox filters, approval channels, machine tabs and provider-source help pages. The named thread needs a first turn for its permalink to show content.
- That thread's Conversation, Team, Code, Environment and Agent views, through the real rail or the phone Details menu, and each registered review view with its terminal list. No shell starts.
- The finder, Share, Find in conversation, the view chooser, resource creation forms, the personal token form, the keyboard shortcuts and your extra dialogs. No form is submitted.
- Ada with **As admin** Off and On, and Vera and Ben in ordinary mode. Public account routes run once per size, theme and locale in a separate anonymous context. A permission refusal for another person is useful evidence.
- Exactly 1440×900 and 400×860, light and dark, at device scale 1, as whole viewports (so clipping, mobile sheets and misplaced controls stay visible).

Every route gets a fresh browser context with cold caches and a fresh device layout store. Sign-in uses a disposable session, and no storage state, token, trace, HAR or video is written. Only the person's theme and locale change for the walk, and they are restored. A setup or cleanup failure is in `session_issues` and fails the command.

## Read the output

The command prints a run directory under `.tmp/captures/<UTC-time>-<unique-id>/`. Open `index.html`:

- `screenshots/`: full PNGs, named by actor, size, theme, locale and state.
- `sheets/<width>x<height>-<theme>-<locale>.html`: labelled thumbnails in four columns, each group with all identities and Ada's mode stated. Click a thumbnail for the original.
- `sheets/<group>-001.png`, `-002.png`, …: sixteen screens (4×4) to a page. Only the last page may have fewer.
- `requests.json`: the origin, times, planned and attempted counts, the measurements of each screen, failures, request-budget verdicts and the sheet index. Incomplete captures and captures that exceed a budget have a red border and caption.

The command finishes the walk and writes its evidence before it reports incomplete screens or budget failures with a nonzero exit. It never skips an inaccessible control quietly. A capture is evidence to read, not an approval of accessibility or layout.

## What the receipt records

Readiness needs real content in the root, the requested theme and language, loaded fonts, no visible `aria-busy` or skeleton, and no outstanding finite request for 500 ms. Every wait has a bound, and no skeleton is hidden for a picture. A failed sign-in produces a placeholder for every affected planned capture, so it cannot pass for the signed-in person.

`requests.json` is saved after every screen. For each screen it holds:

- `first_load`: page-origin `/api/` requests from a cold navigation until the screen is ready. The limit is 8.
- `opening`: requests caused by opening a view or a dialog, apart from the route load.
- `idle`: the next 30,000 ms, with the observed duration, the count, and each request's method, redacted endpoint, relative time, status, failure and completion. The limit is 0. CI has its own 60-second idle gate.
- `after_idle`: any request that settles after that.
- `live_connections`: WebSocket openings, apart. Only the app's `/api/v1/ws` is exempt from network-idle.

Headers, bodies, query values and account-link tokens are not saved. Every identity and every size and theme has its own measurements.

## The regression tests

`../capture-support.spec.ts` covers the mapping of the shared inventory to a world, invalid input, isolation of rotating cookies and device state, catalogue-backed openers, the phases, counts and redaction of requests, late views and skeleton readiness, and the four-column and sixteen-screen HTML and PNG output in both themes:

```sh
ORCH_E2E_NO_DEV_SERVER=1 npx playwright test e2e/capture-support.spec.ts --project=chromium --workers=1
```
