/**
 * Which tool call of an agent is running now: a question or a plan review is asked by a tool, and the protocol's own
 * messages about it name the call by its id, which the question service does not pass on.
 */
import type { ExtraHost } from '../host.ts'
import { sessionIdOf } from './shared.ts'

/** The calls in flight, by session and tool name. */
export interface ToolCallBook {
  /** The id of the latest call of a tool in a session that has not finished. */
  running(sessionId: string, tool: string): string | undefined
}

/** The book of this process; installs its listeners on first use. */
export function toolCallBook(host: ExtraHost): ToolCallBook {
  return host.shared('tool-calls', () => {
    const calls = new Map<string, string>()
    const key = (sessionId: string, tool: string): string => `${sessionId}\u0000${tool}`
    host.ctx.on(
      'tools/pre-execute',
      (async (call: { agent?: { id: string }; name: string; callId: string }, next: () => Promise<unknown>) => {
        const sessionId = sessionIdOf(call.agent)
        if (sessionId !== undefined) calls.set(key(sessionId, call.name), call.callId)
        return next()
      }) as never,
      { global: true },
    )
    return { running: (sessionId: string, tool: string) => calls.get(key(sessionId, tool)) }
  })
}
