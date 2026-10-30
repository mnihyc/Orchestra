/**
 * What Orch told of each session, in `_meta.orch` of `session/new`, `session/resume` and `session/prompt`.
 *
 * The messages pass through the channel, so the book watches them: a request that opens a session is remembered by its
 * id until its answer leaves (a new session gets its id there), and a prompt updates what the session was told. Extras
 * read the book, and may add a member to the answer that opens a session (plan mode adds the modes).
 */
import type { Channel } from './channel.ts'
import { isRequest, META_KEY, type RpcId, type RpcMessage, type SessionMeta } from './protocol.ts'

const OPENING = new Set(['session/new', 'session/resume', 'session/load'])

/** The `_meta.orch` object of a message's params, or undefined. */
export function orchMeta(params: unknown): SessionMeta | undefined {
  const meta = (params as { _meta?: Record<string, unknown> } | undefined)?._meta?.[META_KEY]
  return typeof meta === 'object' && meta !== null ? (meta as SessionMeta) : undefined
}

/** What a hook that decorates the answer to a session-opening request is given. */
export interface OpenedSession {
  method: string
  sessionId: string
  /** The `result` of the response; a hook may add members. */
  result: Record<string, unknown>
}

/** The book. */
export class SessionBook {
  private readonly metas = new Map<string, SessionMeta>()
  private readonly waiting = new Map<RpcId, { method: string; sessionId?: string; meta?: SessionMeta }>()
  private readonly opened: Array<(session: OpenedSession) => void> = []
  private readonly prompted: Array<(sessionId: string, meta: SessionMeta | undefined) => void> = []

  /** Watches a channel's traffic. */
  attach(channel: Channel): void {
    channel.tapIncoming((message) => this.incoming(message))
    channel.tapOutgoing((message) => this.outgoing(message))
  }

  /** What Orch last told of a session; members are replaced as newer messages bring them. */
  metaOf(sessionId: string): SessionMeta | undefined {
    return this.metas.get(sessionId)
  }

  /** Called with the answer to every request that opens a session, before it leaves. */
  onOpened(hook: (session: OpenedSession) => void): void {
    this.opened.push(hook)
  }

  /** Called with every prompt, before dsh's server reads it. */
  onPrompt(hook: (sessionId: string, meta: SessionMeta | undefined) => void): void {
    this.prompted.push(hook)
  }

  private remember(sessionId: string, meta: SessionMeta | undefined): void {
    if (meta !== undefined) this.metas.set(sessionId, { ...this.metas.get(sessionId), ...meta })
  }

  private incoming(message: RpcMessage): void {
    if (!isRequest(message)) return
    const params = message.params as { sessionId?: unknown } | undefined
    const sessionId = typeof params?.sessionId === 'string' ? params.sessionId : undefined
    if (OPENING.has(message.method) && message.id !== undefined) {
      const entry: { method: string; sessionId?: string; meta?: SessionMeta } = { method: message.method }
      if (sessionId !== undefined) entry.sessionId = sessionId
      const meta = orchMeta(message.params)
      if (meta !== undefined) entry.meta = meta
      this.waiting.set(message.id, entry)
    } else if (message.method === 'session/prompt' && sessionId !== undefined) {
      const meta = orchMeta(message.params)
      this.remember(sessionId, meta)
      for (const hook of this.prompted) hook(sessionId, meta)
    }
  }

  private outgoing(message: RpcMessage): RpcMessage {
    if (isRequest(message) || message.id === null) return message
    const entry = this.waiting.get(message.id)
    if (entry === undefined) return message
    this.waiting.delete(message.id)
    const result = message.result as Record<string, unknown> | undefined
    if (message.error !== undefined || result === undefined) return message
    const sessionId = entry.sessionId ?? (typeof result['sessionId'] === 'string' ? result['sessionId'] : undefined)
    if (sessionId === undefined) return message
    this.remember(sessionId, entry.meta)
    for (const hook of this.opened) hook({ method: entry.method, sessionId, result })
    return message
  }
}
