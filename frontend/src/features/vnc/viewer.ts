/** The rules of the VNC view that are no drawing: the socket's address and subprotocol, why a socket ended as its close frame says, and how the picture's quality follows updates that fall behind.
 *
 * How falling behind is measured: every second the view looks at how long the messages of the display's
 * socket waited before the page could take them (an event's time against the time it was handled) and at
 * how late its own one-second timer fired. Either above a quarter of a second means the page could not
 * keep up with the updates. Two such seconds running ask the server for two levels less quality; ten
 * seconds that keep up ask for one level more, back to where it started. The level is the protocol's own
 * quality setting, which the viewer sends to the server.
 */

/** The one subprotocol a display's socket speaks. */
export const DISPLAY_SUBPROTOCOL = 'orch.display.v1';

/** The close code whose text says why a display's socket ended. */
export const CLOSE_WITH_REASON = 4000;

/** The reasons a display's socket ends with, as the server writes them in its close frame. */
export const DISPLAY_ENDS = ['quit', 'closed_by_agent', 'idle', 'exited', 'environment_stopped', 'link_lost', 'switched_off', 'revoked', 'protocol'] as const;
export type DisplayEnd = (typeof DISPLAY_ENDS)[number];

/** Why a socket ended: one of the server's reasons, `unknown` for a reason it did not name, or `null` for a line that was lost and is tried again. */
export function endOf(code: number, reason: string): DisplayEnd | 'unknown' | null {
  if (code !== CLOSE_WITH_REASON) return null;
  return (DISPLAY_ENDS as readonly string[]).includes(reason) ? (reason as DisplayEnd) : 'unknown';
}

export function displayAddress(displayId: string, ticket: string): string {
  const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${scheme}//${location.host}/api/v1/ws/displays/${encodeURIComponent(displayId)}?ticket=${encodeURIComponent(ticket)}`;
}

/** The quality the viewer starts at, and the best it goes back to: the viewer's own default. */
export const QUALITY_BEST = 6;
/** The least quality asked for: below it the picture says little. */
export const QUALITY_LEAST = 0;
/** How late a message or the timer may be before the page counts as falling behind, in milliseconds. */
export const BEHIND_AFTER_MS = 250;
/** Seconds of falling behind before less is asked for: one slow second is not a trend. */
const BEHIND_SECONDS = 2;
/** Seconds of keeping up before more is asked for: slower up than down, so that a link at its limit does not swing. */
const KEEPING_UP_SECONDS = 10;
/** How many levels a step down takes. */
const STEP_DOWN = 2;

export interface QualityState {
  level: number;
  behindFor: number;
  upFor: number;
}

export const FIRST_QUALITY: QualityState = { level: QUALITY_BEST, behindFor: 0, upFor: 0 };

/** The quality after one more second, behind or not. */
export function adapt(state: QualityState, behind: boolean): QualityState {
  if (behind) {
    const behindFor = state.behindFor + 1;
    if (behindFor >= BEHIND_SECONDS && state.level > QUALITY_LEAST) return { level: Math.max(QUALITY_LEAST, state.level - STEP_DOWN), behindFor: 0, upFor: 0 };
    return { ...state, behindFor, upFor: 0 };
  }
  const upFor = state.upFor + 1;
  if (upFor >= KEEPING_UP_SECONDS && state.level < QUALITY_BEST) return { level: state.level + 1, behindFor: 0, upFor: 0 };
  return { ...state, behindFor: 0, upFor };
}

/** The line a program is started with to show on a display Orch opened. */
export function displayLine(xDisplay: string): string {
  return ['DISPLAY', xDisplay].join('=');
}

/** A size as a display takes it: width by height in pixels. */
export const GEOMETRY = /^[1-9]\d{1,4}x[1-9]\d{1,4}$/;
