/**
 * Plan mode, enforced.
 *
 * dsh's plan mode is guidance to the model: a prompt section and a tool that presents the plan. Nothing stops a
 * model that ignores it. This extra turns it into a rule. It lets Orch switch plan mode on and off, refuses at the
 * tool pipeline every call that is not on a read-only list (and runs only read-only commands), and reports the state
 * and each refusal. The plan itself reaches Orch through the question extra as `_orch/plan/review`.
 */
import { ExtraOff, type Extra, type ExtraHost } from '../host.ts'
import {
  Method,
  type PlanGetParams,
  type PlanSetParams,
  type PlanSetResult,
  type PlanState,
} from '../protocol.ts'
import { isReadOnlyCommand } from './commands.ts'
import { liveAgent, paramsOf, requireOn, sessionIdOf, type Agent, type SessionEventRecord } from './shared.ts'

/** The state dsh's plan-mode service reports. */
interface PlanModeService {
  get(agent: Agent): { active: boolean; pending?: boolean }
  set(agent: Agent, active: boolean): 'committed' | 'queued' | 'cancelled' | 'noop'
}

/** The tool call a guard is asked about. */
export interface GuardedCall {
  callId: string
  name: string
  arguments?: unknown
  agent?: Agent
}

/** The same policy is checked before asking and again at the host's monotonic execution guard. */
export function planToolGuard(host: ExtraHost): { refuse: (call: GuardedCall) => string | undefined } {
  return host.shared('plan-tool-guard', () => ({ refuse: (_call: GuardedCall): string | undefined => undefined }))
}

/** The sentence the model is given when plan mode refuses a call. */
export function refusalFor(tool: string, command: string | undefined): string {
  const subject = command === undefined ? `The ${tool} tool` : `This command (${command.trim().split(/\s+/)[0] ?? tool})`
  return `${subject} would change something, and plan mode is on: only reading is allowed. Explore with read-only tools, then present the plan with exit_plan_mode.`
}

export const plan: Extra = {
  id: 'plan_mode',
  async start(host: ExtraHost) {
    const service = await host.waitService<PlanModeService>('planMode')
    if (service === undefined) throw new ExtraOff('host_api_missing', 'the plan-mode service is not loaded')
    if (typeof service.get !== 'function' || typeof service.set !== 'function') {
      throw new ExtraOff('host_api_changed', 'the plan-mode service has no get and set')
    }
    const tools = host.service<{ guard?: (guard: (call: GuardedCall) => string | undefined) => () => void }>('tools')
    if (typeof tools?.guard !== 'function') throw new ExtraOff('host_api_missing', 'the tool pipeline has no guards')
    const { readTools, commandTool, readCommands } = host.config.planMode
    const allowed = new Set(readTools)

    const policy = planToolGuard(host)
    policy.refuse = (call) => {
      if (!host.isOn('plan_mode')) return undefined
      const agent = call.agent
      const sessionId = sessionIdOf(agent)
      if (agent === undefined || sessionId === undefined) return undefined
      let active = false
      try {
        active = service.get(agent).active
      } catch (error) {
        host.failRuntime('plan_mode', error)
        return undefined
      }
      if (!active) return undefined
      if (allowed.has(call.name)) return undefined
      let command: string | undefined
      if (call.name === commandTool) {
        const given = (call.arguments as { command?: unknown } | undefined)?.command
        command = typeof given === 'string' ? given : ''
        if (isReadOnlyCommand(command, readCommands)) return undefined
      }
      const reason = refusalFor(call.name, command)
      host.channel.notify(Method.planRefused, { sessionId, callId: call.callId, tool: call.name, reason })
      return reason
    }
    tools.guard((call) => policy.refuse(call))

    const stateOf = (sessionId: string, agent: Agent): PlanState => {
      const now = service.get(agent)
      const state: PlanState = { sessionId, active: now.active }
      if (now.pending === true) state.pending = true
      return state
    }

    // A prompt that says it is a planning turn switches plan mode on before dsh reads it (it is idle, so at once).
    host.book.onPrompt(
      host.guarded('plan_mode', (sessionId: string, meta: { plan?: boolean } | undefined) => {
        if (typeof meta?.plan !== 'boolean') return
        const agent = host.service<{ get(id: string): Agent | undefined }>('agents')?.get(sessionId)
        if (agent !== undefined && service.get(agent).active !== meta.plan) {
          service.set(agent, meta.plan)
          host.channel.notify(Method.planState, stateOf(sessionId, agent))
        }
      }),
    )

    host.channel.handle(Method.planSet, (params): PlanSetResult => {
      requireOn(host, 'plan_mode')
      const input = paramsOf(params) as Partial<PlanSetParams>
      const agent = liveAgent(host, input.sessionId)
      const outcome = service.set(agent, input.active === true)
      host.channel.notify(Method.planState, stateOf(agent.id, agent))
      return { outcome }
    })
    host.channel.handle(Method.planGet, (params): PlanState => {
      requireOn(host, 'plan_mode')
      const input = paramsOf(params) as Partial<PlanGetParams>
      const agent = liveAgent(host, input.sessionId)
      return stateOf(agent.id, agent)
    })
    host.ctx.on(
      'session/event',
      host.guarded('plan_mode', (session: { id?: string; header?: { id?: string } }, event: SessionEventRecord) => {
        if (event.type !== 'plan/mode') return
        const sessionId = sessionIdOf(session)
        if (sessionId === undefined) return
        host.channel.notify(Method.planState, { sessionId, active: event.data?.active === true })
      }),
      { global: true },
    )
  },
}
