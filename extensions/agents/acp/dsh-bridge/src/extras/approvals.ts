/**
 * Approvals for changes.
 *
 * With its file sandbox open (the only choice inside a container), dsh never asks before running a command or writing a
 * file: its approval prompts exist for sandbox escalations. This extra adds the ask. In `ask` mode a call of a changing
 * tool (and a command that is not on the read-only list) is put to dsh's approval service, whose answerer in the ACP
 * server turns it into the standard permission request. In `bypass` mode nothing asks. Orch chooses the mode of each
 * session.
 */
import { ExtraOff, type Extra, type ExtraHost } from '../host.ts'
import { Method, type ApprovalMode, type ApprovalSetParams, type ApprovalSetResult } from '../protocol.ts'
import { isReadOnlyCommand } from './commands.ts'
import { planToolGuard } from './plan.ts'
import { liveAgent, paramsOf, requireOn, sessionIdOf, stringParam, type Agent } from './shared.ts'

interface ApprovalService {
  setPolicy(agent: Agent, policy: 'ask' | 'never'): void
}

interface PreCall {
  callId: string
  name: string
  arguments?: unknown
  agent?: Agent
}

export const approvals: Extra = {
  id: 'approvals',
  async start(host: ExtraHost) {
    const service = await host.waitService<ApprovalService>('approval')
    if (service === undefined) throw new ExtraOff('host_api_missing', 'the approval service is not loaded')
    if (typeof service.setPolicy !== 'function') throw new ExtraOff('host_api_changed', 'the approval service has no setPolicy')
    const { askTools } = host.config.approvals
    const { commandTool, readCommands } = host.config.planMode
    const asking = new Set(askTools)
    const modes = new Map<string, ApprovalMode>()
    const modeOf = (sessionId: string): ApprovalMode => modes.get(sessionId) ?? host.config.approvals.mode

    /** dsh refuses every ask while its policy is `never`, so a session in `ask` mode needs the policy `ask`. */
    const apply = (agent: Agent, mode: ApprovalMode): void => {
      service.setPolicy(agent, mode === 'ask' ? 'ask' : 'never')
    }

    host.ctx.on(
      'agent/created',
      host.guarded('approvals', (payload: { agent: Agent }) => {
        const agent = payload.agent
        const sessionId = sessionIdOf(agent)
        if (sessionId !== undefined) apply(agent, modeOf(sessionId))
      }),
      { global: true },
    )

    // What Orch says of a session in `_meta.orch` counts like `_orch/approval/set`: the later of the two wins.
    const fromMeta = (sessionId: string, meta: { approval_mode?: string } | undefined): void => {
      if (typeof meta?.approval_mode !== 'string') return
      const mode: ApprovalMode = meta.approval_mode === 'bypass' ? 'bypass' : 'ask'
      modes.set(sessionId, mode)
      const agent = host.service<{ get(id: string): Agent | undefined }>('agents')?.get(sessionId)
      if (agent !== undefined) apply(agent, mode)
    }
    host.book.onOpened(host.guarded('approvals', ({ sessionId }: { sessionId: string }) => fromMeta(sessionId, host.book.metaOf(sessionId))))
    host.book.onPrompt(host.guarded('approvals', fromMeta))

    host.ctx.on(
      'tools/pre-execute',
      (async (call: PreCall, next: () => Promise<unknown>) => {
        // The host asks before running its monotonic guards. Refuse here as well, so a changing
        // call forbidden by plan mode never offers an approval that could appear to permit it.
        const refusal = planToolGuard(host).refuse(call)
        if (refusal !== undefined) return { kind: 'deny', reason: refusal }
        if (!host.isOn('approvals')) return next()
        const sessionId = sessionIdOf(call.agent)
        if (sessionId === undefined || modeOf(sessionId) !== 'ask') return next()
        if (!asking.has(call.name)) return next()
        if (call.name === commandTool) {
          const given = (call.arguments as { command?: unknown } | undefined)?.command
          if (typeof given === 'string' && isReadOnlyCommand(given, readCommands)) return next()
        }
        return { kind: 'ask', reason: `Run ${call.name}` }
      }) as never,
      { global: true, prepend: true },
    )

    host.channel.handle(Method.approvalSet, (params): ApprovalSetResult => {
      requireOn(host, 'approvals')
      const input = paramsOf(params) as Partial<ApprovalSetParams> & Record<string, unknown>
      const agent = liveAgent(host, input.sessionId)
      const mode = stringParam(input, 'mode')
      if (mode !== 'ask' && mode !== 'bypass') throw new Error("mode must be 'ask' or 'bypass'")
      modes.set(agent.id, mode)
      apply(agent, mode)
      return { mode }
    })
  },
}
