# Review captures of a running world

The integrator runs this tool against the retained candidate. It starts no server, invokes no build,
resets no world, intercepts no API, and constructs no application state. The separate Playwright
configuration runs one browser/page at a time. The normal CI `*.spec.ts` suite does not run the
manual `review.capture.ts` entry.

From `frontend/`, with the supplied passwords already in the three environment variables named in
the private input file, the one command is:

```sh
ORCH_CAPTURE_URL=http://127.0.0.1:41800 ORCH_CAPTURE_CONFIG=.tmp/review-capture.json npx playwright test --config=e2e/capture/playwright.config.ts
```

The URL is the running world's origin. `ORCH_CAPTURE_URL` overrides `baseURL` in the input. Chromium
and the frontend's existing dependencies must already be installed by the integrator. There is no
install/build fallback. `ORCH_CAPTURE_OUTPUT` can override the output directory.

## Input, once per retained world

Use `capture.example.json` as the input format; keep the actual file in the ignored `.tmp/` area.
Its target paths illustrate the fake world's known identities. **Replace those paths with the real
review fixtures' paths copied from the running world.** One `project` path covers all project views
and settings sections; one `provider` path covers its tabs, and so on. The keys are documented by
`TARGET_SEEDS` in `manifest.ts`. Selecting explicit fixtures avoids quietly choosing an arbitrary
first provider or an empty thread. The tool never creates missing fixtures or substitutes fake
responses. A real permission refusal or missing resource stays visible in its screenshot.

`people` supplies Ada, Vera and Ben's login names and the **names of environment variables** containing
their supplied passwords. No passwords belong in the JSON, command arguments, screenshots' labels,
or source. The tool signs in through the actual form. Password-only sign-ins are required; a second
step/refusal is a named sign-in failure, not an indefinite wait or a skipped identity.

Optional `replacements` maps complete path segments or ids in scope/item query values. Optional
`routes` maps an exact inventory URL to a local URL and wins over the target mapping. These support
unusual retained fixtures without forking the inventory. URLs cannot leave the given origin.
Built-in expired account links keep their deliberately invalid tokens; this is an existing world,
so `/setup` can show its ordinary refusal/redirect. The tool never resets it to fresh.

Optional `dialogs` extends the built-in **opening** click list, for example a world-specific item:

```json
{
  "id": "environment-restart-review",
  "label": "Environment restart review",
  "url": "/resources/environments/REAL-ENVIRONMENT-ID",
  "clicks": [{ "role": "button", "name": "@environmentRestart.action" }],
  "dialog": "*"
}
```

Each click has a role and exact accessible name; `startsWith: true` handles names containing a
place's title, and `within` scopes a repeated control to a CSS selector. `@catalogue.key` reads the
actual shipped catalogue. A literal is useful for a particular fixture's title. `dialog: "*"`
requires an actual visible dialog when its title contains that fixture's name. Click lists open
forms/pickers only: never include a final Create, Restart, Delete, Save, Send or other confirming act.
The built-in list never confirms one. Optional `states` adds further URL-only states with `id`,
`label`, `url` and an optional expected `dialog`, with no artificial click. For example, supply a real
`/thread/<id>?item=<item-id>&items=<item-id>,<second-id>` selection or a document's `?version=2`
or `?block=<block-id>` address. These identifiers come from the retained world, not generated events.

English is the default. `"locales": ["en", "zh-CN"]` captures both catalogues as well. The named
fixtures and literal custom click names must make sense in the requested locales; catalogue keys
already do. Both themes always run, independently of the person's saved preference.

## What it captures

- Every entry of `../layout-routes.ts`, including every settings section and resource tab. That is
  the layout gate's single route inventory, not a copied list. The same inventory continues to be
  checked against the source route/section declarations by the layout suite.
- More URL states: org members/activity; project activity/done/archived; topic tabs; a thread turn
  permalink and its catch-up dialog; inbox filters; approval channels; machine tabs; provider-source
  help pages. The named thread must contain a first turn for its turn permalink to show content.
- That thread's Conversation, Team, Code, Environment and Agent containers, through the actual rail
  or phone Details menu; each registered review view, including its terminal list (no shell starts).
- Finder, Share, Find in conversation, view chooser, resource creation forms, personal token form,
  keyboard shortcuts, and the extra supplied dialog-opening sequences. No form is submitted.
- Ada with As admin explicitly Off and On, Vera and Ben in ordinary mode. Public account routes
  run once per size/theme/locale in a separate anonymous context, rather than being redirected by
  an existing sign-in. Permission refusals for the other identities are useful review evidence.
  Each shell that shows the As admin switch is also checked against the requested mode; its
  observed value is retained separately from the planned mode in the JSON.
- Exactly 1440×900 and 400×860, light/dark, device scale 1. Screenshots are whole viewports so panel
  clipping, mobile sheets and misplaced controls remain visible. They are not stitched tall pages.

Every route gets a fresh browser context with cold JS/query/HTTP caches, the current rotated
refresh cookie and a fresh device layout/choice store. The shared world and its files are retained.
Sign-in uses a disposable new session; no storage-state/token/trace/HAR/video is written. Only the
person's theme/locale are temporarily changed, then restored; a simultaneous different preference
edit wins over restoration. The capture session is signed out at the end. Setup/cleanup failures
are in `session_issues` and fail the command.

## Content and request evidence

Readiness requires actual root content, the requested theme/language, loaded fonts, no visible
`aria-busy`/skeleton, and no outstanding finite request for 500 ms. Every navigation, control,
content wait and network-idle wait has a bound. No skeleton is hidden or removed for a picture.
A state-opening failure retains its actual screen and an incomplete label. A failed sign-in
produces an explicit placeholder for **every** affected planned capture; it cannot masquerade as
the signed-in person. JavaScript failures are flagged without saving credential-bearing messages.

`requests.json` is checkpointed after every screen. Each entry contains:

- `first_load`: page-origin `/api/` requests from before a cold navigation until the base screen
  reaches readiness, or until the URL state itself is ready. The first-load limit is 8.
- `opening`: requests caused by the explicit container/view/dialog opening, separately from the
  initial route. Query-opened states are part of first load; clicking an opener is not disguised
  as a new route load. The JSON preserves every request, not just GETs or successful answers.
- `idle`: the next **30,000 ms**, with its actual observed duration, count and per-request method,
  redacted endpoint, relative time, HTTP status, failure and completion. The limit is 0. This is
  the requested 30-second review observation; CI's separate 60-second idle gate remains.
- `after_idle`: any final settling requests, so a delayed refresh cannot vanish from the receipt.
- `live_connections`: actual WebSocket openings separately. Only the app's `/api/v1/ws` transport
  is exempt from network-idle; an ordinary hung request times out. Assets/cross-origin images
  affect readiness but are not counted as page-origin API requests.

Request headers, bodies, response bodies, query values and account-link tokens are not saved.
Sign-in/appearance setup requests happen outside the page meter and are not route-load requests.
All four identities and all size/theme variants get their own measurements; the tool does not
reuse a warm cache or one actor's counts for another.

## Output and review

The command prints the run directory under `.tmp/captures/<UTC-time>-<unique-id>/`. Open `index.html`:

- `screenshots/`: numbered names containing actor, dimensions, theme, locale and state; full PNGs.
- `sheets/<width>x<height>-<theme>-<locale>.html`: labelled thumbnails, **four columns**. Every group
  contains all identities, with Ada's session mode explicit. Labels wrap and are never clipped.
- `sheets/<group>-001.png`, `-002.png`, …: one **sixteen-screen (4×4)** page per PNG; only the final
  sheet may have fewer screens. Click an HTML thumbnail for its full original. Complete sheets
  appear incrementally during a long walk, and the final partial sheets are written at the end.
- `requests.json`: candidate/origin/times, expected/attempted counts, per-screen measurements,
  failures, request-budget verdicts and the sheet index. Incomplete and budget-failing captures
  have a red border/caption. A PNG failure retains its HTML/originals and fails the command.

The command completes the walk and writes its evidence before reporting incomplete screens or
budget failures with a nonzero exit. It never silently skips inaccessible controls. Read the
evidence: a capture is not an accessibility/layout approval or a mature-product verdict.

**Time:** a full route/state × four-person-mode × two-width × two-theme walk is deliberately large.
The tool prints its exact planned screen count and the lower bound `count × 30 s` before visiting
anything. Loading/captures add time; another locale doubles it. It does not weaken the requested
idle window or start parallel browsers to finish sooner. Review complete sheets while it proceeds.

## Integrator's regression gate

`../capture-support.spec.ts` covers mapping the full shared inventory to a retained world, invalid
inputs, rotating-cookie/device-state isolation, catalogue-backed openers, API phase/count/redaction
semantics, late-view/skeleton readiness and four-column/sixteen-screen HTML **and PNG** output in
both themes. It is discovered by the normal Playwright configuration. Focused command:

```sh
ORCH_E2E_NO_DEV_SERVER=1 npx playwright test e2e/capture-support.spec.ts --project=chromium --workers=1
```

The capture command uses the supplied running world; it does not start an application server or build.
