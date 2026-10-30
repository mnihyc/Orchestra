/**
 * Writing a session into dsh's store without running it: fork and history carry both make a new session from a list of
 * events. The agent lifecycle is what attaches dsh's session writer, so the session is created as an agent with that
 * history as its seed, flushed to disk, and let go; `session/resume` then opens it like any other stored session.
 */
import { RpcFault } from '../channel.ts'
import type { ExtraHost } from '../host.ts'
import { ErrorCode } from '../protocol.ts'
import type { Agent, SessionEventRecord } from './shared.ts'

interface AgentHandle {
  readonly agent: Agent
  dispose(): Promise<void>
}

interface AgentRegistry {
  create(options: {
    sessionId: string
    meta: { cwd: string; parentSession?: string; isSeeded?: boolean }
    inheritedEventCount?: number
    seed: readonly unknown[]
    agentOptions?: { provider?: string; model?: string }
  }): Promise<AgentHandle>
  resume(options: { resumeSessionId: string; agentOptions?: { provider?: string; model?: string } }): Promise<AgentHandle>
  list(): Agent[]
}

interface SessionStore {
  flush(session: unknown): Promise<boolean>
}

/** What a stored session is made from. */
export interface StoredSessionSpec {
  sessionId: string
  cwd: string
  seed: readonly unknown[]
  parentSession?: string
  /** For a fork: how many leading events were copied from the parent. */
  inheritedEventCount?: number
  provider?: string
  model?: string
}

/**
 * The route a new session records when its caller names none: the one dsh is configured with (its default agent model),
 * or else that of a live agent of this process. dsh refuses to open a session that records a route it does not have.
 */
export function defaultRoute(host: ExtraHost): { provider: string; model: string } | undefined {
  const configured = host.service<{ provider?: string; model?: string; config?: { provider?: string; model?: string } }>('agentDefaultModel')
  const given = configured?.config ?? configured
  if (typeof given?.provider === 'string' && typeof given.model === 'string') return { provider: given.provider, model: given.model }
  const agents = host.service<AgentRegistry>('agents')
  for (const agent of agents?.list?.() ?? []) {
    const options = (agent as unknown as { options?: { provider?: string; model?: string } }).options
    if (typeof options?.provider === 'string' && typeof options.model === 'string') return { provider: options.provider, model: options.model }
  }
  return undefined
}

/** Creates the session in dsh's store and returns when it is on disk. */
export async function storeSession(host: ExtraHost, spec: StoredSessionSpec): Promise<void> {
  const agents = host.service<AgentRegistry>('agents')
  const sessions = host.service<SessionStore>('sessions')
  if (agents === undefined || sessions === undefined) throw new RpcFault(ErrorCode.notNow, 'dsh has no agent registry in this process')
  const meta: { cwd: string; parentSession?: string; isSeeded?: boolean } = { cwd: spec.cwd }
  if (spec.parentSession !== undefined) {
    meta.parentSession = spec.parentSession
    meta.isSeeded = true
  }
  const options: Parameters<AgentRegistry['create']>[0] = { sessionId: spec.sessionId, meta, seed: spec.seed }
  if (spec.inheritedEventCount !== undefined) options.inheritedEventCount = spec.inheritedEventCount
  if (spec.provider !== undefined && spec.model !== undefined) options.agentOptions = { provider: spec.provider, model: spec.model }
  const handle = await agents.create(options)
  try {
    await sessions.flush(handle.agent.session)
  } finally {
    await handle.dispose()
  }
  // The session is read back the way a resume reads it, so that Orch is never told of one that dsh cannot open.
  let reopened: AgentHandle | undefined
  try {
    const reopen: Parameters<AgentRegistry['resume']>[0] = { resumeSessionId: spec.sessionId }
    if (options.agentOptions !== undefined) reopen.agentOptions = options.agentOptions
    reopened = await agents.resume(reopen)
  } catch (error) {
    throw new RpcFault(ErrorCode.internalError, `the new session cannot be opened again: ${causeText(error)}`)
  } finally {
    await reopened?.dispose()
  }
}

/** The message of an error and of the errors that caused it. */
function causeText(error: unknown): string {
  const parts: string[] = []
  for (let at: unknown = error; at instanceof Error && parts.length < 5; at = (at as { cause?: unknown }).cause) parts.push(at.message)
  return parts.join(': ')
}

/** The events of a session, copied so that the copy may be stored. */
export function copyEvents(events: readonly SessionEventRecord[]): SessionEventRecord[] {
  return structuredClone(events) as SessionEventRecord[]
}
