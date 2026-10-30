/**
 * Live token streaming.
 *
 * dsh's ACP server sends each finished assistant message whole. The agent loop also publishes every delta of a model
 * call as `agent/assistant-stream` frames; this extra batches them into `_orch/stream` notices, and tags the
 * committed text and thought updates that dsh's server writes afterwards with `_meta.orch`, so that Orch can turn the
 * streamed item into the committed one instead of showing the text twice.
 */
import type { Extra, ExtraHost } from '../host.ts'
import {
  COMMIT_META_KEY,
  isRequest,
  Method,
  type CommitMeta,
  type RpcMessage,
  type StreamFrame,
} from '../protocol.ts'
import { attemptBook } from './attempts.ts'
import { sessionIdOf, type SessionEventRecord } from './shared.ts'

interface Call {
  sessionId: string
  turn: number
  step: number
  attempt: string
  /** Position of the next frame to send. */
  next: number
  pending: StreamFrame[]
  pendingChars: number
  timer: ReturnType<typeof setTimeout> | undefined
}

/** The attempt that tags each committed message, by the message's id. */
const TAG_LIMIT = 256

export const streaming: Extra = {
  id: 'streaming',
  start(host: ExtraHost) {
    const calls = new Map<string, Call>()
    /** The latest attempt of each session, which is the one a committed message of that session comes from. */
    const book = attemptBook(host)
    const tags = new Map<string, CommitMeta>()
    const { flushMs, flushChars } = host.config.streaming

    const flush = (call: Call): void => {
      if (call.timer !== undefined) clearTimeout(call.timer)
      call.timer = undefined
      if (call.pending.length === 0) return
      const frames = call.pending
      call.pending = []
      call.pendingChars = 0
      const seq = call.next
      call.next += frames.length
      host.channel.notify(Method.stream, {
        sessionId: call.sessionId,
        turn: call.turn,
        step: call.step,
        attempt: call.attempt,
        seq,
        frames,
      })
    }

    const push = (call: Call, frame: StreamFrame, chars: number): void => {
      const last = call.pending[call.pending.length - 1]
      if (last !== undefined && frame.kind === last.kind && (frame.kind === 'text' || frame.kind === 'reasoning')) {
        const previous = last as Extract<StreamFrame, { kind: 'text' | 'reasoning' }>
        if (previous.index === frame.index) {
          previous.delta += frame.delta
          call.pendingChars += chars
          return after(call)
        }
      }
      if (last !== undefined && frame.kind === 'tool_input' && last.kind === 'tool_input' && last.callId === frame.callId) {
        last.delta += frame.delta
        if (last.name === undefined && frame.name !== undefined) last.name = frame.name
        call.pendingChars += chars
        return after(call)
      }
      call.pending.push(frame)
      call.pendingChars += chars
      after(call)
    }

    const after = (call: Call): void => {
      if (call.pendingChars >= flushChars || flushMs === 0) return flush(call)
      call.timer ??= setTimeout(() => flush(call), flushMs)
    }

    host.ctx.on(
      'agent/assistant-stream',
      host.guarded('streaming', (payload: { agent: { id: string }; frame: any }) => {
        const frame = payload.frame
        const sessionId = sessionIdOf(payload.agent)
        if (sessionId === undefined) return
        if (frame.type === 'start') {
          const call: Call = {
            sessionId,
            turn: frame.turn,
            step: frame.step,
            attempt: String(frame.attemptId),
            next: 0,
            pending: [],
            pendingChars: 0,
            timer: undefined,
          }
          calls.set(call.attempt, call)
          book.latest.set(sessionId, { attempt: call.attempt, turn: call.turn, step: call.step })
          push(call, { kind: 'start' }, 0)
          return
        }
        const call = calls.get(String(frame.attemptId))
        if (call === undefined) return
        if (frame.type === 'chunk') {
          const chunk = frame.chunk
          if (chunk.type === 'text-delta') push(call, { kind: 'text', index: chunk.index, delta: chunk.text }, chunk.text.length)
          else if (chunk.type === 'reasoning-delta') push(call, { kind: 'reasoning', index: chunk.index, delta: chunk.text }, chunk.text.length)
          else if (chunk.type === 'tool-call-delta') {
            const piece: StreamFrame = { kind: 'tool_input', index: chunk.index, callId: String(chunk.id), delta: chunk.argumentsDelta }
            if (chunk.name !== undefined) (piece as { name?: string }).name = chunk.name
            push(call, piece, chunk.argumentsDelta.length)
          }
          return
        }
        if (frame.type === 'end') {
          call.pending.push({ kind: 'end', outcome: frame.outcome.kind === 'committed' ? 'committed' : 'abandoned' })
          flush(call)
          calls.delete(call.attempt)
        }
      }),
      { global: true },
    )

    // The committed message is logged before the call's end frame, in the same turn and step as the latest attempt.
    host.ctx.on(
      'session/event',
      host.guarded('streaming', (session: { id?: string; header?: { id?: string } }, event: SessionEventRecord) => {
        if (event.type !== 'assistant/message') return
        const sessionId = sessionIdOf(session)
        const messageId: unknown = event.data?.message?.id
        if (sessionId === undefined || typeof messageId !== 'string') return
        const call = book.latest.get(sessionId)
        if (call === undefined || call.turn !== event.data.turn || call.step !== event.data.step) return
        tags.set(messageId, { attempt: call.attempt, turn: call.turn, step: call.step })
        if (tags.size > TAG_LIMIT) {
          const oldest = tags.keys().next().value
          if (oldest !== undefined) tags.delete(oldest)
        }
      }),
      { global: true },
    )

    host.channel.tapOutgoing((message: RpcMessage) => {
      if (!isRequest(message) || message.method !== 'session/update') return message
      const update = (message.params as { update?: { sessionUpdate?: string; messageId?: string; _meta?: Record<string, unknown> } } | undefined)?.update
      if (update === undefined || typeof update.messageId !== 'string') return message
      if (update.sessionUpdate !== 'agent_message_chunk' && update.sessionUpdate !== 'agent_thought_chunk') return message
      const tag = tags.get(update.messageId)
      if (tag === undefined || !host.isOn('streaming')) return message
      update._meta = { ...update._meta, [COMMIT_META_KEY]: tag }
      return message
    })
  },
}
