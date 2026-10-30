# Layout rules

The integrator runs this suite; writers do not run any build, test, formatter, linter or generator.
CI's existing `npx playwright test` command discovers these files through `playwright.config.ts`.
For this complete batch alone, from `frontend/`:

```sh
ORCH_E2E_NO_DEV_SERVER=1 npx playwright test e2e/layout-rules --project=chromium --workers=1
```

The existing Playwright configuration starts the fake API and builds/serves the application once.
It uses one worker because the fake API has one mutable world. No development gallery or external
service is needed. The production `/dev/ui` address is checked as the deliberate not-found page it
ships as. `playwright.native.config.ts` remains separate.

The unit and component sources for the overflow mechanism and disclosures are:

```sh
npm run test -- tests/unit/overflow-tooltips.test.ts tests/unit/overflow-previews.test.ts tests/component/overflow-values.test.tsx tests/component/overflow-disclosures.test.tsx
```

These commands are supplied for the integrator and were not executed by the writer.

For the retained running-world review, [capture/README.md](capture/README.md) gives the separate
capture command, private sign-in/fixture input, 1440×900/400×860 light/dark screenshots, 4×4 contact
sheets and per-page first-load/30-second-idle request receipts. It starts no server or build.

## Coverage and evidence

`layout-routes.ts` supplies concrete fake-world URLs for every file-route pattern, including signed-out
forms, expired-link refusals, retained inspection, ordinary resource detail/list pages and Management.
It expands org/project/topic settings and provider/environment/agent/host/template tabs. A coverage test
reads the route declarations and section arrays; a newly added route or section without a case fails.
The MCP and mount routes use typed browser fixtures, as their existing browser proofs do, because the
HTTP double does not implement those two resources. No production endpoint is replaced.

Each route runs at **1440 × 900** and **400 × 900**, in light/dark and English/Chinese: eight combinations.
Signed-in walks explicitly enable the real As admin switch so Management is reached. Signed-out walks
reset their own scenarios. Each route is a separate bounded test with a JSON receipt naming the rule,
text, DOM/AX locations and measurements. Every route attaches a full-page screenshot, including failed
readiness waits while the page remains available, so the integrator can review the whole matrix.
There are no swallowed readiness waits: a heading, settled appearance, finished loading indicators and
loaded fonts are required. Shared-state contexts close in `afterAll`.

The suite opens every currently rendered read-only menu/listbox, including guided select pickers, and
hovers icon actions so resting opacity cannot conceal a bad alignment. It checks every cut value by
hover and keyboard focus. Negative controls prove that each rule can reject a representative fault;
the geometry control distinguishes a half-pixel offset from the owner's one-pixel failure threshold.
The audit also walks the currently rendered options with Home/ArrowDown and pointer hover, without
activating an act. A native `>` palette case exercises its keyboard and pointer states at every matrix
entry. Picker closure is bounded by its open layers plus the possible tooltip; it stops when the last
picker closes so it cannot dismiss an underlying dialog.

## Rules and the duplicate allow-list

- Toolbar/tab **More** overflow and inert hidden-item ellipses fail. A named row-actions menu is a
  different control; it is opened and checked too.
- Computed accessible names come from Chromium's accessibility tree, grouped by the nearest landmark.
  A heading containing its own link/button is one occurrence. Separate controls with the same name fail.
- Page-action names are compared globally, including icon-only and pressed buttons, across
  landmarks. The old direct-text rule could see only the labelled Star; the AX rule checks its nearest
  landmark rather than the whole page. A regression reproduces this gap and requires the icon/text
  pair to fail, including the two-character Chinese label. Distinct subjects must have distinct names;
  repeated object rows keep their existing exception. Radio values, switches, comboboxes and tabs are
  choices/navigation rather than page actions; the existing text/AX rules still check them.
- The duplicate rule compares visible direct text of **three or more Unicode characters** across the whole screen,
  not only one landmark. Counting its direct owner avoids counting the same text again on each wrapper.
  List, table, menu/option and timeline rows are the specified repeated-row exception; their surrounding
  headings, filters and captions are checked.
- **There are no allow-listed in-page duplicate strings.** `DUPLICATE_ALLOW_LIST` is empty. Its supported
  entries require exact text, one exact selector per occurrence and a nonempty reason. A third occurrence
  fails. The only enabled class allowance is a **full-value disclosure**: a tooltip may repeat its
  own actually clipped value. The detector verifies one original, real clipping and ARIA ownership;
  an unrelated tooltip, an uncut original or a second original does not qualify. Each applied allowance
  and its reason is recorded in the receipt. This is required because full-value tooltips make an otherwise hidden
  value readable, rather than adding another in-page label or action.
- Enabled bounded controls require **3:1** against the adjacent surface. The browser resolves CSS
  colours into sRGB and alpha-composites them; a segmented selector's shared border is measured too.
  Inactive controls are the WCAG exception. Ghost actions need not acquire an invented border.
- Ordinary wrapping labels and table cells must fit their longest language-segmented word, measured
  in the browser's actual font with letter spacing and padding/borders removed. One CSS pixel allows
  fractional glyph/box rounding. This covers unmarked text, not only shared Choice spans. Deliberate
  one-line ellipsis, clamps and scrolling retain their separate full-value disclosure requirements; literal code and
  editable/user-authored content retain their native wrapping policy.
- Selected and highlighted text requires **4.5:1** against its own rendered ground, including child
  backgrounds, transparent colours and ancestor opacity. ARIA checked/selected/pressed/current,
  Radix state/highlight and cmdk `data-selected` plus option hover/focus are measured. Visible labels
  remain readable even when aria-hidden or selected-but-disabled. An image/gradient ground fails
  measurement explicitly, instead of passing against a guessed flat colour. Negative controls cover
  both themes/languages, plain table cells, opacity, keyboard-only/hover-only faults, and the 4.54/4.48
  boundary. The original shared-control width/contrast regressions remain in the suite.
- A visible `[data-choice-description]` must be inside a tooltip or an open menu/listbox.
- Raw setting/key patterns and known fake-world slugs, routing names and wire states fail in UI copy.
  Code, editable/source data and explicitly marked user-authored Markdown retain their actual spelling.
  That scope does not exempt their text from the separate duplicate rule.
- Ellipsis/clamp detection requires actual lost space, or an explicitly retained pre-shortened value.
  Every cut must disclose the whole available value by hover and focus. A complete native title covers
  hover; it still needs a keyboard equivalent. An existing custom tooltip is exercised, not assumed good.
- An icon button is compared with the row's **first text line**, not its multi-line height. An offset
  of **1 pixel or more** fails. Floating/absolutely positioned controls are not row-alignment candidates.

## The shared overflow mechanism

`TooltipProvider` retains one document installation across nested providers. It delegates pointer/focus
events, preserves existing tooltip triggers, adds keyboard stops only where clipped static text needs
one, and removes only stops/descriptions it owns. It observes DOM changes, candidate resizes and font
completion; there is no polling and no request. Escape dismisses, scrolling/layout changes clear stale
positions, and the tooltip itself remains hoverable/scrollable. Full values are always inserted as text.

CSS ellipsis/clamps keep the complete text in the DOM. A caller that deliberately cuts its string before
rendering records `data-overflow-text` on that preview; shared Select options accept `fullLabel` for the
same purpose. A manually shortened directory with an existing full native title also gets keyboard
coverage. Full-value metadata must follow the same redaction/deletion decision as its visible preview.
The inline folder-path ellipsis and folded JSON string are actual buttons that reveal their contents.
The JSON disclosure keeps the value outside its compact button. Thread descriptions and tool summaries
use the shared reader only when cut; their uncut text is not repeated in another tooltip. Automatic
description timestamps remain on the existing status icon and in the button's accessible description.
The environment's runtime facts stay in Overview, without repeating them in the page header; its
state appears once in the header, or in the active job banner while busy, with no second Overview copy.
