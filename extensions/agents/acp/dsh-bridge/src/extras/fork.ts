/**
 * Fork: a copy of a session up to the end of a turn.
 *
 * ACP has no fork, and the one dsh has is for its own sub-agents. The copy is written as a stored session whose
 * header names the source as its parent and the number of events it inherited; Orch then resumes it with the ordinary
 * `session/resume`.
 */
import { randomUUID } from 'node:crypto'
import { RpcFault } from '../channel.ts'
import { ExtraOff, type Extra, type ExtraHost } from '../host.ts'
import { ErrorCode, Method, type SessionForkResult } from '../protocol.ts'
import { copyEvents, storeSession } from './records.ts'
import { liveAgent, paramsOf, requireOn } from './shared.ts'

export const fork: Extra = {
  id: 'fork',
  async start(host: ExtraHost) {
    if ((await host.waitService('agents')) === undefined) throw new ExtraOff('host_api_missing', 'the agent registry is not loaded')
    if ((await host.waitService('sessions')) === undefined) throw new ExtraOff('host_api_missing', 'the session store is not loaded')
    host.channel.handle(Method.sessionFork, async (params): Promise<SessionForkResult> => {
      requireOn(host, 'fork')
      const input = paramsOf(params)
      const agent = liveAgent(host, input['sessionId'])
      const through = input['throughTurn']
      if (through !== undefined && (typeof through !== 'number' || !Number.isInteger(through) || through < 1)) {
        throw new RpcFault(ErrorCode.invalidParams, 'throughTurn must be a whole number of 1 or more')
      }
      const events = agent.session.snapshotEvents()
      const ends = events.filter((event) => event.type === 'turn/end')
      const boundary = through === undefined ? ends[ends.length - 1] : ends.find((event) => event.data?.turn === through)
      if (boundary === undefined) {
        throw new RpcFault(ErrorCode.notNow, through === undefined ? 'the session has no finished turn to copy' : `turn ${through} has not finished`)
      }
      const prefix = copyEvents(events.slice(0, boundary.seq + 1))
      const childId = randomUUID()
      const cwd = agent.session.header.cwd
      if (cwd === undefined) throw new RpcFault(ErrorCode.notNow, 'the session has no working directory')
      const options = (agent as unknown as { options?: { provider?: string; model?: string } }).options
      // The copy carries no parent link: dsh's ACP server refuses to resume a session that names a parent, and Orch keeps
      // the lineage of its own threads.
      const spec: Parameters<typeof storeSession>[1] = { sessionId: childId, cwd, seed: prefix }
      if (options?.provider !== undefined) spec.provider = options.provider
      if (options?.model !== undefined) spec.model = options.model
      await storeSession(host, spec)
      return { sessionId: childId, turns: ends.filter((event) => event.seq <= boundary.seq).length }
    })
  },
}
