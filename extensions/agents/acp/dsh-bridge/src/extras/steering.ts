/**
 * Steering and follow-up.
 *
 * ACP allows one prompt at a time and has no way to speak to a turn that is running. The agent handle of dsh does:
 * `steer` gives the model a message at its next step, `followup` queues a message for after the turn. Orch sends both
 * as requests, and learns what became of each message from `_orch/input/state`.
 */
import { randomUUID } from 'node:crypto'
import { RpcFault } from '../channel.ts'
import type { Extra, ExtraHost } from '../host.ts'
import {
  ErrorCode,
  Method,
  type ContentPart,
  type InputParams,
  type InputResult,
  type InputState,
} from '../protocol.ts'
import { liveAgent, paramsOf, requireOn, sessionIdOf, stringParam, type Agent, type UserMessage } from './shared.ts'

/** How many ids of Orch's messages are remembered; older ones were long since claimed. */
const REMEMBERED = 1024

function textOf(content: unknown): UserMessage['content'] {
  if (!Array.isArray(content) || content.length === 0) throw new RpcFault(ErrorCode.invalidParams, 'content must be a non-empty list')
  const parts = content as ContentPart[]
  const out: UserMessage['content'] = []
  for (const part of parts) {
    if (part?.type === 'text' && typeof part.text === 'string') out.push({ type: 'text', text: part.text })
    else if (part?.type === 'image') {
      throw new RpcFault(ErrorCode.invalidParams, 'an image cannot be sent to a running turn; send it with the next prompt')
    } else throw new RpcFault(ErrorCode.invalidParams, 'a content part must be text')
  }
  if (out.every((part) => part.text.trim().length === 0)) throw new RpcFault(ErrorCode.invalidParams, 'the message is empty')
  return out
}

export const steering: Extra = {
  id: 'steering',
  start(host: ExtraHost) {
    /** dsh's id of a message, by the id Orch gave it; the reverse is looked up when the state is reported. */
    const orchIdOf = new Map<string, string>()

    const send = (kind: 'steer' | 'followup') => (params: unknown): InputResult => {
      requireOn(host, 'steering')
      const input = paramsOf(params) as Partial<InputParams> & Record<string, unknown>
      const agent: Agent = liveAgent(host, input.sessionId)
      const orchId = stringParam(input, 'messageId')
      const message: UserMessage = { role: 'user', id: randomUUID(), content: textOf(input.content), source: { kind: 'user' } }
      orchIdOf.set(message.id, orchId)
      if (orchIdOf.size > REMEMBERED) {
        const oldest = orchIdOf.keys().next().value
        if (oldest !== undefined) orchIdOf.delete(oldest)
      }
      if (kind === 'steer') agent.steer(message)
      else agent.followup(message)
      return { messageId: message.id, accepted: true }
    }
    host.channel.handle(Method.inputSteer, send('steer'))
    host.channel.handle(Method.inputFollowup, send('followup'))

    const report = (state: InputState) =>
      host.guarded('steering', (payload: { agent: Agent; message: { id: string }; turn?: number }) => {
        const orchId = orchIdOf.get(payload.message.id)
        const sessionId = sessionIdOf(payload.agent)
        if (orchId === undefined || sessionId === undefined) return
        const notice: Record<string, unknown> = { sessionId, messageId: orchId, state }
        if (state === 'claimed' && typeof payload.turn === 'number') notice['turn'] = payload.turn
        host.channel.notify(Method.inputState, notice)
        if (state !== 'queued') orchIdOf.delete(payload.message.id)
      })
    host.ctx.on('agent/inbox/inserted', report('queued'), { global: true })
    host.ctx.on('agent/inbox/claimed', report('claimed'), { global: true })
    host.ctx.on('agent/inbox/discarded', report('discarded'), { global: true })
  },
}
