/**
 * Compaction phases, and a manual run.
 *
 * dsh compacts long histories on its own: it first cuts old tool output, then replaces older turns with a summary,
 * and logs each step. ACP shows none of it. This extra reads the log as it grows and reports the phases, and runs a
 * compaction when Orch asks for one.
 */
import { RpcFault } from '../channel.ts'
import { ExtraOff, type Extra, type ExtraHost } from '../host.ts'
import { ErrorCode, Method, type CompactionNotice, type CompactionRunResult } from '../protocol.ts'
import { liveAgent, paramsOf, requireOn, sessionIdOf, type Agent, type SessionEventRecord } from './shared.ts'

interface CompactionService {
  compactNow(agent: Agent, signal: AbortSignal, sourceCommandId?: string): Promise<unknown>
}

/** How long cut tool output is gathered into one `pruned` notice, in milliseconds; a prune cuts many results at once. */
const PRUNE_GATHER_MS = 100

export const compaction: Extra = {
  id: 'compaction',
  async start(host: ExtraHost) {
    const service = await host.waitService<CompactionService>('compaction')
    if (service === undefined) throw new ExtraOff('host_api_missing', 'the compaction service is not loaded')
    if (typeof service.compactNow !== 'function') throw new ExtraOff('host_api_changed', 'the compaction service has no compactNow')

    interface Run {
      turn: number | null
      pruned: number
      timer: ReturnType<typeof setTimeout> | undefined
    }
    const runs = new Map<string, Run>()
    const send = (notice: CompactionNotice): void => host.channel.notify(Method.compaction, notice)

    const flushPruned = (sessionId: string, run: Run): void => {
      if (run.timer !== undefined) clearTimeout(run.timer)
      run.timer = undefined
      if (run.pruned === 0) return
      send({ sessionId, turn: run.turn, phase: 'pruned', replacedTokens: run.pruned })
      run.pruned = 0
    }

    host.ctx.on(
      'session/event',
      host.guarded('compaction', (session: { id?: string; header?: { id?: string } }, event: SessionEventRecord) => {
        if (!event.type.startsWith('compaction/')) return
        const sessionId = sessionIdOf(session)
        if (sessionId === undefined) return
        if (event.type === 'compaction/start') {
          const turn = typeof event.data?.turn === 'number' ? event.data.turn : null
          runs.set(sessionId, { turn, pruned: 0, timer: undefined })
          send({ sessionId, turn, phase: 'started' })
          return
        }
        const run = runs.get(sessionId) ?? { turn: null, pruned: 0, timer: undefined }
        if (event.type === 'compaction/prune') {
          run.pruned += typeof event.data?.shadowedTokenCount === 'number' ? event.data.shadowedTokenCount : 0
          runs.set(sessionId, run)
          run.timer ??= setTimeout(() => flushPruned(sessionId, run), PRUNE_GATHER_MS)
        } else if (event.type === 'compaction/summary') {
          flushPruned(sessionId, run)
          const notice: CompactionNotice = { sessionId, turn: run.turn, phase: 'summarized' }
          if (typeof event.data?.shadowedTokenCount === 'number') notice.replacedTokens = event.data.shadowedTokenCount
          send(notice)
        } else if (event.type === 'compaction/end') {
          flushPruned(sessionId, run)
          const error: unknown = event.data?.error
          const notice: CompactionNotice = { sessionId, turn: run.turn, phase: error === undefined ? 'completed' : 'failed' }
          if (error !== undefined) notice.error = typeof error === 'string' ? error : JSON.stringify(error)
          send(notice)
          runs.delete(sessionId)
        }
      }),
      { global: true },
    )

    host.channel.handle(Method.compactionRun, async (params): Promise<CompactionRunResult> => {
      requireOn(host, 'compaction')
      const agent = liveAgent(host, paramsOf(params)['sessionId'])
      try {
        const result = await service.compactNow(agent, new AbortController().signal)
        return { compacted: result !== null && result !== undefined }
      } catch (error) {
        const code = (error as { code?: unknown }).code
        if ((error as { name?: string }).name === 'ManualCompactionError') {
          throw new RpcFault(ErrorCode.notNow, `compaction did not run: ${String(code ?? (error as Error).message)}`)
        }
        throw error
      }
    })
  },
}
