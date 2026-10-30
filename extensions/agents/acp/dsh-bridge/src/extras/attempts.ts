/**
 * Which model call is the current one of a session. The streaming extra records each call when its first frame
 * passes; the committed message and its usage are logged in the same turn and step, so they find their call here.
 */
import type { ExtraHost } from '../host.ts'

/** The latest call of each session. */
export interface AttemptBook {
  latest: Map<string, { attempt: string; turn: number; step: number }>
}

/** The book of this process. */
export function attemptBook(host: ExtraHost): AttemptBook {
  return host.shared('attempts', () => ({ latest: new Map() }))
}

/** The attempt id of the call a message of this turn and step came from, when streaming saw it. */
export function attemptOf(host: ExtraHost, sessionId: string, turn: number, step: number): string | undefined {
  const call = attemptBook(host).latest.get(sessionId)
  return call !== undefined && call.turn === turn && call.step === step ? call.attempt : undefined
}
