/** A screen that waits for something the live connection is meant to tell it reads again by itself.
 *
 * The live connection brings news (a machine enrolled, a job moved) and brings it once: a hint that is lost, or a connection that
 * never comes up behind a front that does not pass WebSocket upgrades, leaves such a screen waiting for ever. Reading again is the
 * way out that needs nothing of the connection: often while the connection is not up, seldom while it is, and not at all when the
 * wait is over. A browser pauses these reads for a page nobody is looking at.
 */

/** Reads again this often while the live connection is not up. Three seconds is about as long as a person watches a waiting dialog before wondering whether it works, and one small read is all it costs. */
export const READ_AGAIN_DOWN_MS = 3_000;

/** Reads again this often while the live connection is up: only a hint that was lost needs repairing, which is rare, and half a minute repairs it before anyone has left the page. */
export const READ_AGAIN_UP_MS = 30_000;

let connectionIsUp: () => boolean = () => true;

/** The socket's module says how the connection stands. Until it does, the connection counts as up, so a screen in a test reads seldom. */
export function readAgainFollows(probe: () => boolean): void {
  connectionIsUp = probe;
}

/** Whether the live connection is up now. */
export function liveConnectionIsUp(): boolean {
  return connectionIsUp();
}

/** How long to wait before the next read of a wait that is still on; a wait that must not stay stale for half a minute names its own pause while the connection is up. */
export function readAgainAfter(whileUpMs: number = READ_AGAIN_UP_MS): number {
  return connectionIsUp() ? whileUpMs : READ_AGAIN_DOWN_MS;
}

/**
 * The `refetchInterval` of a query that a wait depends on: `waiting` says from what the query holds whether the wait is still on, and
 * the reads stop with it. With `whileUp` false the query is read again only while the connection is not up (for a query whose
 * answer is the next live snapshot, which a read while the connection is up would only wait for).
 */
export function readAgain<Data>(waiting: (data: Data | undefined) => boolean, whileUp = true): (query: { state: { data: Data | undefined } }) => number | false {
  return (query) => {
    if (!waiting(query.state.data)) return false;
    return connectionIsUp() ? (whileUp ? READ_AGAIN_UP_MS : false) : READ_AGAIN_DOWN_MS;
  };
}
