/** Named performance marks: one prefix, the same names every time, so that start-up and every switch of a tab are measured alike.
 *
 * A mark is the moment a step began or ended, named as the step is (`start/session-asked`, `start/session-answered`). The page
 * shows them in the browser's own performance tools (the User Timing track), and `tests/e2e/measure-pages.mjs` reads them. A page
 * that cannot make marks works as it did.
 *
 * Start-up (`start/`): `script` the entry began; `session-asked`, `session-answered`; `shell-asked`, `shell-answered` the shell's
 * first read; `tree-asked` the navigation tree; `page-read-asked` the page's own first read; `words` the catalogue is in; `render`
 * the first draw is asked for; `shell-drawn` the frame is on the screen; `shell-ready` the frame admitted its screens.
 * Tabs (`tab/`, `switch/`): `tab/opened` a tab was opened for an address; `switch/click` a tab was chosen; `switch/shown` its
 * body was put in its place; `switch/painted` the frame after that.
 */
const PREFIX = 'orch/';

/** Marks a moment, named under the one prefix. `detail` says which thing, where it matters (a tab). */
export function mark(name: string, detail?: Record<string, string>): void {
  try {
    performance.mark(`${PREFIX}${name}`, detail === undefined ? undefined : { detail });
  } catch {
    // Nothing is measured where marks cannot be made.
  }
}
