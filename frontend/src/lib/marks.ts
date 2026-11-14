/** Named performance marks: one prefix, the same names every time, so that start-up and every switch of a tab are measured alike.
 *
 * A mark is the moment a step began or ended, named as the step is (`start/session-asked`, `start/session-answered`). The page
 * shows them in the browser's own performance tools (the User Timing track), and `tests/e2e/measure-pages.ts` reads them. A page
 * that cannot make marks works as it did.
 *
 * Start-up (`start/`): `script` the entry began; `session-asked`, `session-answered`; `shell-asked`, `shell-answered` the shell's
 * first read; `tree-asked` the navigation tree; `page-read-asked` the page's own first read; `words` the catalogue is in; `render`
 * the first draw is asked for; `shell-drawn` the frame is on the screen; `shell-ready` the frame admitted its screens.
 * Tabs (`tab/`, `switch/`): `tab/opened` a tab was opened for an address; `switch/click` a tab was chosen; `switch/shown` its
 * body was put in its place; `switch/painted` the frame after that.
 */
const PREFIX = 'orch/';
const contentReaders = new Set<string>();
const contentWaiters = new Set<{ key: string; finish: (shown: boolean) => void }>();
const contentKey = (user: string, reader: string) => JSON.stringify([user, reader]);

/** A background correctness check belongs after actual content, including a recoverable failure view. */
export function whenContentShown(user: string, reader: number | undefined, signal: AbortSignal): Promise<boolean> {
  const key = contentKey(user, String(reader ?? 0));
  if (signal.aborted) return Promise.resolve(false);
  if (contentReaders.has(key)) return Promise.resolve(true);
  return new Promise((resolve) => {
    const waiter = { key, finish: (shown: boolean) => { contentWaiters.delete(waiter); signal.removeEventListener('abort', cancelled); resolve(shown); } };
    const cancelled = () => waiter.finish(false);
    contentWaiters.add(waiter);
    signal.addEventListener('abort', cancelled, { once: true });
  });
}

/** Marks a moment, named under the one prefix. `detail` says which thing, where it matters (a tab). */
export function mark(name: string, detail?: Record<string, string>): void {
  if (name === 'start/script') {
    contentReaders.clear();
    for (const waiter of [...contentWaiters]) waiter.finish(false);
  } else if (name === 'page/content' && detail?.user !== undefined && detail.reader !== undefined) {
    const key = contentKey(detail.user, detail.reader);
    contentReaders.add(key);
    for (const waiter of [...contentWaiters]) if (waiter.key === key) waiter.finish(true);
  }
  try {
    performance.mark(`${PREFIX}${name}`, detail === undefined ? undefined : { detail });
  } catch {
    // Nothing is measured where marks cannot be made.
  }
}
