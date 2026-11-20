# Layout rules suite

Playwright tests that walk every screen of the application and fail on layout and wording faults a person would notice: a clipped value with no way to read it, two controls with the same name, text that does not fit, a low-contrast control, an icon out of line with its row. They run with the rest of the Playwright suite (`npx playwright test` discovers them through `playwright.config.ts`).

## Run it

From `frontend/`:

```sh
ORCH_E2E_NO_DEV_SERVER=1 npx playwright test e2e/layout-rules --project=chromium --workers=1
```

The configuration starts the fake API and builds and serves the application once. It uses one worker because the fake API has one mutable world. No development gallery or outside service is needed: the production `/dev/ui` address is checked as the not-found page it ships as. `playwright.native.config.ts` is a separate configuration.

The unit and component tests of the overflow mechanism and the disclosures:

```sh
npm run test -- tests/unit/overflow-tooltips.test.ts tests/unit/overflow-previews.test.ts tests/component/overflow-values.test.tsx tests/component/overflow-disclosures.test.tsx
```

To review a running world rather than the fake one, use the [capture tool](capture/README.md).

## What it covers

`layout-routes.ts` gives a concrete fake-world address for every file-route pattern: signed-out forms, expired-link refusals, retained inspection, resource lists and details, and Management, with every organization, project and topic settings section and every provider, environment, agent, host and template tab. A coverage test reads the route declarations and section lists, so a route or section added without a case fails. The MCP and mount routes use typed browser fixtures, because the HTTP double does not implement those two resources.

Each route runs at **1440 × 900** and **400 × 900**, in light and dark, in English and Chinese: eight combinations. Signed-in walks turn on the real **As admin** switch to reach Management, and signed-out walks reset their own scenarios. Each route is a separate bounded test with its own context and a JSON receipt that names the rule, the text, the DOM and accessibility locations and the measurements. Each route attaches a full-page screenshot, including when its readiness wait fails. A heading, a settled appearance, finished loading indicators and loaded fonts are required: no readiness wait is swallowed.

The suite also opens every rendered read-only menu and listbox, hovers icon actions so resting opacity cannot hide a misalignment, and checks every cut value by hover and by keyboard focus. Negative controls prove that each rule can reject a representative fault.

## The rules

- **No overflow menus that hide nothing.** A toolbar or tab **More** overflow, or an inert hidden-item ellipsis, fails. A named row-actions menu is a different control and is checked too.
- **Names are unique.** Computed accessible names come from Chromium's accessibility tree, grouped by the nearest landmark. Separate controls with the same name fail, and page actions are compared across landmarks, icon-only and pressed buttons included. Distinct subjects need distinct names. Repeated object rows keep their exception.
- **No repeated text.** Visible direct text of three or more characters may not appear twice on a screen. List, table, menu, option and timeline rows are exempt, but their headings, filters and captions are checked. The global allow list is empty. The only class of allowance is a full-value disclosure: a tooltip may repeat its own actually clipped value.
- **Contrast.** Enabled bounded controls need 3:1 against the adjacent surface (inactive controls are the WCAG exception). Selected and highlighted text needs 4.5:1 against its own rendered ground. A ground that is an image or a gradient fails the measurement explicitly instead of passing against a guessed colour.
- **Words fit.** Wrapping labels and table cells must fit their longest word, measured in the browser's real font. Deliberate one-line ellipsis, clamps and scrolling need a full-value disclosure.
- **A cut value can be read.** An ellipsis or clamp needs actual lost space, and the whole value must be available on hover and on keyboard focus. A native `title` covers hover only.
- **Alignment.** An icon button is compared with the row's first text line, and an offset of one pixel or more fails. Floating controls are not candidates.
- **No raw keys.** Raw setting keys, fake-world slugs, routing names and wire states fail in interface copy. Code, editable data and marked user-authored Markdown keep their spelling.
- **A visible `[data-choice-description]`** must be inside a tooltip or an open menu or listbox.

## The shared overflow mechanism

`TooltipProvider` installs one document-wide handler for nested providers. It delegates pointer and focus events, keeps existing tooltip triggers, adds keyboard stops only where clipped static text needs one, and removes only the stops it owns. It watches DOM changes, resizes and font loading: there is no polling and no request. Escape dismisses it, scrolling clears stale positions, and the tooltip stays hoverable and scrollable. Full values are inserted as text.

CSS ellipsis and clamps keep the whole text in the DOM. A caller that cuts its string before rendering marks the preview with `data-overflow-text`, and shared `Select` options take `fullLabel` for the same purpose. Full-value metadata follows the same redaction and deletion decision as its visible preview.
