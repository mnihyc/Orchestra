/** Small helpers the extras share. */
import { RpcFault } from '../channel.ts'
import type { ExtraHost } from '../host.ts'
import { ErrorCode, type ExtraId } from '../protocol.ts'

/** A live agent of dsh, as far as the extras use it. */
export interface Agent {
  readonly id: string
  readonly status: 'idle' | 'running'
  readonly session: Session
  readonly ctx: unknown
  steer(message: UserMessage): void
  followup(message: UserMessage): void
  cancel(cause: { kind: string; reason?: string }, options?: { keepInbox?: boolean }): void
  whenIdle(): Promise<void>
  runMaintenance?<T>(task: (signal: AbortSignal) => Promise<T>): Promise<T>
}

/** A session of dsh, as far as the extras use it. */
export interface Session {
  readonly id: string
  readonly header: { id: string; cwd?: string }
  readonly seq: number
  snapshotEvents(from?: number, to?: number): readonly SessionEventRecord[]
}

/** One entry of a session's log. */
export interface SessionEventRecord {
  type: string
  seq: number
  time: number
  data: any
}

/** The session a client opened that a session belongs to: a sub-agent's work is reported under the session that started it. */
export function rootSessionId(host: ExtraHost, session: Session): string {
  const sessions = host.service<{ get(id: string): Session | undefined }>('sessions')
  let at: Session = session
  for (let hops = 0; hops < 16; hops += 1) {
    const parent = (at.header as { parentSession?: string }).parentSession
    const next = parent === undefined ? undefined : sessions?.get(parent)
    if (next === undefined) break
    at = next
  }
  return at.header.id
}

/** A user message as dsh's inbox holds it. */
export interface UserMessage {
  role: 'user'
  id: string
  content: Array<{ type: 'text'; text: string }>
  source: { kind: string; [key: string]: unknown }
}

/** The session id of the object a listener was given: a session, or an agent. */
export function sessionIdOf(value: { id?: string; header?: { id?: string } } | undefined): string | undefined {
  return value?.header?.id ?? value?.id
}

/** The live agent of a session, or a fault the caller sends back. */
export function liveAgent(host: ExtraHost, sessionId: unknown): Agent {
  if (typeof sessionId !== 'string') throw new RpcFault(ErrorCode.invalidParams, 'sessionId must be a string')
  const agents = host.service<{ get(id: string): Agent | undefined }>('agents')
  const agent = agents?.get(sessionId)
  if (agent === undefined) throw new RpcFault(ErrorCode.unknownSession, `session ${sessionId} is not live in this process`)
  return agent
}

/** Refuses a request of an extra that is off. */
export function requireOn(host: ExtraHost, extra: ExtraId): void {
  if (!host.isOn(extra)) throw new RpcFault(ErrorCode.extraOff, `${extra} is off in this process`)
}

/** Reads params as an object. */
export function paramsOf(params: unknown): Record<string, unknown> {
  if (typeof params !== 'object' || params === null || Array.isArray(params)) {
    throw new RpcFault(ErrorCode.invalidParams, 'params must be an object')
  }
  return params as Record<string, unknown>
}

/** A required string param. */
export function stringParam(params: Record<string, unknown>, key: string): string {
  const value = params[key]
  if (typeof value !== 'string' || value.length === 0) throw new RpcFault(ErrorCode.invalidParams, `${key} must be a non-empty string`)
  return value
}
