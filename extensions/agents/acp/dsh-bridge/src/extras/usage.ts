/**
 * The token breakdown of each model call.
 *
 * dsh's ACP server reports only how full the context is. The log holds the provider's own counts on each assistant
 * message (and on an attempt that produced none), and this extra forwards them as `_orch/usage`, so Orch can show fresh
 * input, cache reads and writes, output and reasoning, and the cache hit rate.
 */
import type { Extra, ExtraHost } from '../host.ts'
import { Method, type Usage, type UsageNotice } from '../protocol.ts'
import { attemptOf } from './attempts.ts'
import { rootSessionId, sessionIdOf, type SessionEventRecord } from './shared.ts'

/** Copies the counts a provider reported; a count it did not report stays absent. */
export function mapUsage(raw: unknown): Usage | undefined {
  if (typeof raw !== 'object' || raw === null) return undefined
  const source = raw as Record<string, unknown>
  const count = (key: string): number | undefined => {
    const value = source[key]
    return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : undefined
  }
  const inputTokens = count('inputTokens')
  const outputTokens = count('outputTokens')
  if (inputTokens === undefined || outputTokens === undefined) return undefined
  const usage: Usage = { inputTokens, outputTokens }
  const cacheRead = count('cacheReadTokens')
  const cacheWrite = count('cacheWriteTokens')
  const reasoning = count('reasoningTokens')
  const total = count('totalTokens')
  if (cacheRead !== undefined) usage.cacheReadTokens = cacheRead
  if (cacheWrite !== undefined) usage.cacheWriteTokens = cacheWrite
  if (reasoning !== undefined) usage.reasoningTokens = reasoning
  if (total !== undefined) usage.totalTokens = total
  return usage
}

/** The usage chunk inside the compact stream of an attempt that committed no message. */
function usageOfAttempt(stream: unknown): Usage | undefined {
  if (!Array.isArray(stream)) return undefined
  for (const record of stream) {
    if (record?.type === 'chunk' && record.chunk?.type === 'usage') return mapUsage(record.chunk.usage)
  }
  return undefined
}

export const usage: Extra = {
  id: 'usage',
  start(host: ExtraHost) {
    host.ctx.on(
      'session/event',
      host.guarded('usage', (session: { id?: string; header?: { id?: string } }, event: SessionEventRecord) => {
        if (event.type !== 'assistant/message' && event.type !== 'assistant/attempt') return
        const own = sessionIdOf(session)
        if (own === undefined) return
        const sessionId = rootSessionId(host, session as never)
        const data = event.data
        const source = data.message?.source
        const notice: UsageNotice = {
          sessionId,
          turn: data.turn,
          step: data.step,
          provider: typeof source?.provider === 'string' ? source.provider : '',
          model: typeof source?.model === 'string' ? source.model : '',
        }
        const attempt = attemptOf(host, own, data.turn, data.step)
        if (attempt !== undefined) notice.attempt = attempt
        if (sessionId !== own) notice.subagent = true
        if (event.type === 'assistant/message') {
          const counted = mapUsage(data.usage)
          if (counted !== undefined) notice.usage = counted
          if (data.interrupted === true) notice.interrupted = true
        } else {
          const counted = usageOfAttempt(data.stream)
          if (counted === undefined) return
          notice.usage = counted
          notice.failed = true
        }
        host.channel.notify(Method.usage, notice)
      }),
      { global: true },
    )
  },
}
