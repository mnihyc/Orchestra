/**
 * The plugin's settings, as Orch writes them into the row's `config` of the patch it starts dsh with.
 * Every number is a default that Orch can change; none is a limit of the protocol.
 */
import { EXTRA_IDS, type ApprovalMode, type ExtraId } from './protocol.ts'

/** Settings of the streaming extra. */
export interface StreamingConfig {
  /** How long deltas of one model call are gathered before a notice leaves, in milliseconds. */
  flushMs: number
  /** A notice leaves at once when the gathered text is this many characters. */
  flushChars: number
}

/** Settings of the plan mode extra. */
export interface PlanModeConfig {
  /** Tools that never change anything; plan mode lets them run. Any other tool is refused, MCP tools included. */
  readTools: string[]
  /** The tool that runs commands; plan mode lets it run only the commands in `readCommands`. */
  commandTool: string
  /** First words of shell commands that only read. */
  readCommands: string[]
}

/** Settings of the approvals extra. */
export interface ApprovalsConfig {
  /** What a session starts with. */
  mode: ApprovalMode
  /** Tools whose calls ask for approval in `ask` mode. A command tool call that only reads is let through. */
  askTools: string[]
}

/** Settings of the executor backends extra: the agent's commands and files run in Orch's environment. */
export interface BackendsConfig {
  /** Whether this process is one of a separated arrangement. Off, the extra stays off and dsh's own tools work as they are. */
  enabled: boolean
  /** The executor's address for sessions that do not bring their own `exec` in `_meta.orch`. */
  url: string | null
  /** The environment variable that holds the token for `url`; the token itself never stands in a file. */
  tokenEnv: string
  /** How long `job_output` waits for more output when it is asked to wait, in milliseconds (the executor allows 30000). */
  jobWaitMs: number
  /** How many paths `glob` returns at most. */
  globMax: number
  /** How many matching lines `grep` returns at most (the executor allows 1000). */
  grepMax: number
}

/** The resolved settings. */
export interface BridgeConfig {
  /** Which extras are switched on; an absent one is on. */
  extras: Record<ExtraId, boolean>
  streaming: StreamingConfig
  planMode: PlanModeConfig
  approvals: ApprovalsConfig
  backends: BackendsConfig
  /** Send whatever other code writes to stdout to stderr, so that nothing but protocol lines reaches Orch. */
  guardStdout: boolean
  /** The version of the dsh build, when Orch knows it (it verified the build); otherwise the plugin tries to read it. */
  hostVersion: string | null
  /** How long the plugin waits for a service of dsh at start before an extra that needs it goes off, in milliseconds. */
  serviceWaitMs: number
}

/** Plugin defaults. */
export const DEFAULTS: BridgeConfig = {
  extras: Object.fromEntries(EXTRA_IDS.map((id) => [id, true])) as Record<ExtraId, boolean>,
  // 25 ms is about 40 notices a second at most for one stream: below what a person sees as lag, and far below what a
  // line reader can follow.
  streaming: { flushMs: 25, flushChars: 4096 },
  planMode: {
    readTools: [
      'read',
      'glob',
      'grep',
      'ls',
      'web_search',
      'web_fetch',
      'skill',
      'todo_write',
      'ask_user_question',
      'exit_plan_mode',
      'read_image',
    ],
    commandTool: 'bash',
    readCommands: [
      'ls',
      'cat',
      'head',
      'tail',
      'wc',
      'file',
      'stat',
      'pwd',
      'echo',
      'grep',
      'rg',
      'find',
      'tree',
      'du',
      'df',
      'which',
      'env',
      'git status',
      'git log',
      'git diff',
      'git show',
      'git branch',
      'git ls-files',
      'git rev-parse',
    ],
  },
  approvals: { mode: 'ask', askTools: ['bash', 'pwsh', 'write', 'edit', 'str_replace_editor'] },
  // 25000 ms is the executor client's own wait step; 100 and 250 are what dsh's own glob and grep return.
  backends: { enabled: false, url: null, tokenEnv: 'ORCH_EXECUTOR_TOKEN', jobWaitMs: 25000, globMax: 100, grepMax: 250 },
  guardStdout: true,
  hostVersion: null,
  serviceWaitMs: 5000,
}

function record(value: unknown, where: string): Record<string, unknown> {
  if (value === undefined || value === null) return {}
  if (typeof value !== 'object' || Array.isArray(value)) throw new Error(`orch-dsh-bridge: ${where} must be an object`)
  return value as Record<string, unknown>
}

function positive(value: unknown, fallback: number, where: string): number {
  if (value === undefined) return fallback
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    throw new Error(`orch-dsh-bridge: ${where} must be a number of zero or more`)
  }
  return value
}

function strings(value: unknown, fallback: string[], where: string): string[] {
  if (value === undefined) return fallback
  if (!Array.isArray(value) || value.some((item) => typeof item !== 'string')) {
    throw new Error(`orch-dsh-bridge: ${where} must be a list of strings`)
  }
  return value as string[]
}

/** Reads the row's config; a wrong type throws with the name of the field. */
export function resolveConfig(raw: unknown): BridgeConfig {
  const root = record(raw, 'config')
  const extras = { ...DEFAULTS.extras }
  const given = record(root['extras'], 'extras')
  for (const [key, value] of Object.entries(given)) {
    if (!(EXTRA_IDS as readonly string[]).includes(key)) throw new Error(`orch-dsh-bridge: extras.${key} is not an extra`)
    if (typeof value !== 'boolean') throw new Error(`orch-dsh-bridge: extras.${key} must be true or false`)
    extras[key as ExtraId] = value
  }
  const streaming = record(root['streaming'], 'streaming')
  const plan = record(root['planMode'], 'planMode')
  const approvals = record(root['approvals'], 'approvals')
  const mode = approvals['mode']
  if (mode !== undefined && mode !== 'ask' && mode !== 'bypass') throw new Error("orch-dsh-bridge: approvals.mode must be 'ask' or 'bypass'")
  const backends = record(root['backends'], 'backends')
  const flag = backends['enabled']
  if (flag !== undefined && typeof flag !== 'boolean') throw new Error('orch-dsh-bridge: backends.enabled must be true or false')
  const backendUrl = backends['url']
  if (backendUrl !== undefined && backendUrl !== null && typeof backendUrl !== 'string') throw new Error('orch-dsh-bridge: backends.url must be a string')
  const tokenEnv = backends['tokenEnv']
  if (tokenEnv !== undefined && typeof tokenEnv !== 'string') throw new Error('orch-dsh-bridge: backends.tokenEnv must be a string')
  const guard = root['guardStdout']
  if (guard !== undefined && typeof guard !== 'boolean') throw new Error('orch-dsh-bridge: guardStdout must be true or false')
  const commandTool = plan['commandTool']
  if (commandTool !== undefined && typeof commandTool !== 'string') {
    throw new Error('orch-dsh-bridge: planMode.commandTool must be a string')
  }
  return {
    extras,
    streaming: {
      flushMs: positive(streaming['flushMs'], DEFAULTS.streaming.flushMs, 'streaming.flushMs'),
      flushChars: positive(streaming['flushChars'], DEFAULTS.streaming.flushChars, 'streaming.flushChars'),
    },
    planMode: {
      readTools: strings(plan['readTools'], DEFAULTS.planMode.readTools, 'planMode.readTools'),
      commandTool: commandTool ?? DEFAULTS.planMode.commandTool,
      readCommands: strings(plan['readCommands'], DEFAULTS.planMode.readCommands, 'planMode.readCommands'),
    },
    approvals: { mode: mode ?? DEFAULTS.approvals.mode, askTools: strings(approvals['askTools'], DEFAULTS.approvals.askTools, 'approvals.askTools') },
    backends: {
      enabled: flag ?? DEFAULTS.backends.enabled,
      url: backendUrl ?? DEFAULTS.backends.url,
      tokenEnv: tokenEnv ?? DEFAULTS.backends.tokenEnv,
      jobWaitMs: positive(backends['jobWaitMs'], DEFAULTS.backends.jobWaitMs, 'backends.jobWaitMs'),
      globMax: positive(backends['globMax'], DEFAULTS.backends.globMax, 'backends.globMax'),
      grepMax: positive(backends['grepMax'], DEFAULTS.backends.grepMax, 'backends.grepMax'),
    },
    guardStdout: guard ?? DEFAULTS.guardStdout,
    hostVersion: typeof root['hostVersion'] === 'string' ? root['hostVersion'] : null,
    serviceWaitMs: positive(root['serviceWaitMs'], DEFAULTS.serviceWaitMs, 'serviceWaitMs'),
  }
}
