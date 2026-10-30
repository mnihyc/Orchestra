/**
 * The messages between Orch and this plugin, carried as JSON-RPC 2.0 lines on the same stdio the Agent Client
 * Protocol uses. Every method name starts with `_orch/`, the prefix the Agent Client Protocol reserves for
 * extensions, so a peer that does not know a name answers "method not found" and nothing else is disturbed.
 *
 * Rules of the protocol:
 * - A field the reader does not know is ignored. A new field or method keeps `PROTOCOL_VERSION`; a change that
 *   would break an older reader raises it.
 * - A count the program did not report is absent, never zero.
 * - Every per-session message carries `sessionId`, the Agent Client Protocol session id (which is dsh's own).
 * - Orch to plugin and plugin to Orch are named by the comment on each type.
 */

/** The version of this protocol; announced in `_orch/hello`. */
export const PROTOCOL_VERSION = 1

/** The name the plugin announces itself with. */
export const PLUGIN_NAME = 'orch-dsh-bridge'

/** The prefix of every method of this protocol. */
export const METHOD_PREFIX = '_orch/'

/** The methods and notifications of the protocol, by name. */
export const Method = {
  /** Plugin to Orch notification after `initialize`; also an Orch to plugin request that answers the same body. */
  hello: '_orch/hello',
  /** Plugin to Orch notification: one extra changed state at run time. */
  extraState: '_orch/extra_state',
  /** Plugin to Orch notification: live deltas of one model call. */
  stream: '_orch/stream',
  /** Orch to plugin request: steer the running turn. */
  inputSteer: '_orch/input/steer',
  /** Orch to plugin request: queue a follow-up for after the running turn. */
  inputFollowup: '_orch/input/followup',
  /** Plugin to Orch notification: what became of an input message. */
  inputState: '_orch/input/state',
  /** Plugin to Orch request: ask a person questions. */
  questionAsk: '_orch/question/ask',
  /** Plugin to Orch notification: a question asked earlier is no longer waited for. */
  questionClosed: '_orch/question/closed',
  /** Orch to plugin request: switch plan mode on or off. */
  planSet: '_orch/plan/set',
  /** Orch to plugin request: read plan mode. */
  planGet: '_orch/plan/get',
  /** Plugin to Orch notification: plan mode changed. */
  planState: '_orch/plan/state',
  /** Plugin to Orch request: the model submitted a plan for review. */
  planReview: '_orch/plan/review',
  /** Plugin to Orch notification: a tool call was refused because plan mode is on. */
  planRefused: '_orch/plan/refused',
  /** Orch to plugin request: choose whether changes ask for approval (`ask`) or run (`bypass`). */
  approvalSet: '_orch/approval/set',
  /** Plugin to Orch notification: token accounting of one model call. */
  usage: '_orch/usage',
  /** Plugin to Orch notification: a phase of a compaction. */
  compaction: '_orch/compaction',
  /** Orch to plugin request: compact the session now. */
  compactionRun: '_orch/compaction/run',
  /** Orch to plugin request: rescan the skills root. */
  skillsRefresh: '_orch/skills/refresh',
  /** Orch to plugin request: copy a session at a turn boundary. */
  sessionFork: '_orch/session/fork',
  /** Orch to plugin request: write a neutral history into a new session. */
  historyImport: '_orch/history/import',
  /** Orch to plugin request: write a session's own record to a file. */
  historyExport: '_orch/history/export',
} as const

/**
 * The prefixes reserved for the separated arrangement's file and command backends. Unused: the plugin calls the executor of
 * the session itself (`SessionMeta.exec`), and these stay reserved so that no other method takes them.
 */
export const BackendPrefix = {
  /** Plugin to Orch requests that read and change files in the environment. */
  fs: '_orch/fs/',
  /** Plugin to Orch requests that run commands in the environment. */
  exec: '_orch/exec/',
} as const

/** JSON-RPC error codes the protocol uses beyond the standard ones. */
export const ErrorCode = {
  parseError: -32700,
  invalidRequest: -32600,
  methodNotFound: -32601,
  invalidParams: -32602,
  internalError: -32603,
  /** The person, or Orch for them, declined to answer. */
  declined: -32001,
  /** The extra behind the method is off in this process. */
  extraOff: -32002,
  /** The session is unknown or not live in this process. */
  unknownSession: -32003,
  /** The request cannot be done now (for example, the turn already ended). */
  notNow: -32004,
} as const

/** The extras, by the name Orch's descriptor capability is derived from. */
export type ExtraId =
  | 'streaming'
  | 'steering'
  | 'questions'
  | 'plan_mode'
  | 'approvals'
  | 'usage'
  | 'compaction'
  | 'skills'
  | 'fork'
  | 'native_carry'
  | 'executor_backends'

/** Every extra, in the order the plugin reports them. */
export const EXTRA_IDS: readonly ExtraId[] = [
  'streaming',
  'steering',
  'questions',
  'plan_mode',
  'approvals',
  'usage',
  'compaction',
  'skills',
  'fork',
  'native_carry',
  'executor_backends',
]

/** Why an extra is off. */
export type OffReason =
  /** Orch's settings switched it off. */
  | 'disabled_by_settings'
  /** An internal API of dsh that the extra needs is absent in this build. */
  | 'host_api_missing'
  /** The API is there but its check showed it behaves differently from what the extra was written against. */
  | 'host_api_changed'
  /** The extra's own startup check failed for another reason. */
  | 'self_check_failed'
  /** The extra was on and failed while running; it stays off for the rest of the process. */
  | 'runtime_failure'
  /** The process lacks something the extra needs (a writable home, a skills root). */
  | 'unsupported_environment'

/** Whether one extra works in this process. */
export type ExtraState =
  | { status: 'on' }
  | { status: 'off'; reason: OffReason; detail?: string }

/** The announcement of the plugin and its extras. */
export interface Hello {
  /** `PROTOCOL_VERSION` of the plugin. */
  protocol: number
  /** The plugin. */
  plugin: { name: string; version: string }
  /** The dsh build the plugin runs in; `version` is null when it could not be read. */
  host: { name: 'dsh'; version: string | null; tested: boolean }
  /** The state of every extra. */
  extras: Record<ExtraId, ExtraState>
  /** Methods the plugin answers (Orch to plugin requests). */
  methods: string[]
  /** Notifications and requests the plugin may send. */
  emits: string[]
}

/** Params of `_orch/extra_state`. */
export interface ExtraStateChanged {
  extra: ExtraId
  state: ExtraState
}

// ---------------------------------------------------------------------------------------------------------------------
// Input: content, steering and follow-up

/** One part of a message a person sends. */
export type ContentPart =
  | { type: 'text'; text: string }
  | { type: 'image'; mimeType: string; data: string }

/** Params of `_orch/input/steer` and `_orch/input/followup`. */
export interface InputParams {
  sessionId: string
  /** Orch's own id of the message; echoed in `_orch/input/state`. */
  messageId: string
  content: ContentPart[]
}

/** Result of the two input requests. */
export interface InputResult {
  /** The id of the message as dsh holds it in its inbox. */
  messageId: string
  /** True when the message was queued. */
  accepted: boolean
}

/** What became of a message. */
export type InputState = 'queued' | 'claimed' | 'discarded'

/** Params of `_orch/input/state` (plugin to Orch). */
export interface InputStateChanged {
  sessionId: string
  /** The `messageId` Orch gave, or the dsh id when the message did not come from Orch. */
  messageId: string
  state: InputState
  /** The turn that claimed the message; present for `claimed`. */
  turn?: number
}

// ---------------------------------------------------------------------------------------------------------------------
// Streaming

/** One live delta of a model call. */
export type StreamFrame =
  | { kind: 'start' }
  | { kind: 'text'; index: number; delta: string }
  | { kind: 'reasoning'; index: number; delta: string }
  | { kind: 'tool_input'; index: number; callId: string; name?: string; delta: string }
  | {
      kind: 'end'
      /** `committed`: dsh logged the call's result; the committed message follows as ordinary updates. */
      outcome: 'committed' | 'abandoned'
    }

/** Params of `_orch/stream`: consecutive frames of one model call. */
export interface StreamNotice {
  sessionId: string
  turn: number
  step: number
  /** Identifies the model call; the committed messages that follow carry it in `_meta`. */
  attempt: string
  /** Position of the first frame of this notice among the frames of the call, dense from zero. */
  seq: number
  frames: StreamFrame[]
}

/** The key of `_meta` in a committed `session/update` that ties it to the call that streamed it. */
export const COMMIT_META_KEY = 'orch'

/** What the plugin adds to `_meta` of a committed assistant text or thought update. */
export interface CommitMeta {
  attempt: string
  turn: number
  step: number
}

// ---------------------------------------------------------------------------------------------------------------------
// Questions and plan review

/** One choice of a question. */
export interface QuestionOption {
  label: string
  description?: string
}

/** One question. */
export interface Question {
  /** Echoed in the answer. */
  id: string
  question: string
  detail?: string
  header?: string
  options?: QuestionOption[]
  multiSelect?: boolean
}

/** Params of `_orch/question/ask`. */
export interface QuestionAskParams {
  sessionId: string
  /** The tool call that asked; groups the questions on Orch's side. */
  callId: string
  questions: Question[]
}

/** The answer to one question. */
export interface QuestionAnswer {
  id: string
  selected: string[]
  /** A free-text answer; for a single choice it replaces the selection. */
  custom?: string
}

/** Result of `_orch/question/ask`. */
export interface QuestionAskResult {
  answers: QuestionAnswer[]
}

/** Params of `_orch/question/closed`: the question is no longer waited for. */
export interface QuestionClosed {
  sessionId: string
  callId: string
  reason: 'cancelled' | 'turn_ended' | 'session_closed'
}

/** Params of `_orch/plan/review`. */
export interface PlanReviewParams {
  sessionId: string
  callId: string
  /** The plan, as markdown starting with a title. */
  plan: string
}

/** The decision on a plan. */
export type PlanDecision = 'proceed' | 'keep_planning' | 'dismiss'

/** Result of `_orch/plan/review`. */
export interface PlanReviewResult {
  decision: PlanDecision
  /** What the person wants changed; read for `keep_planning`. */
  feedback?: string
}

/** Params of `_orch/plan/set`. */
export interface PlanSetParams {
  sessionId: string
  active: boolean
}

/** What setting plan mode did. */
export type PlanSetOutcome = 'committed' | 'queued' | 'cancelled' | 'noop'

/** Result of `_orch/plan/set`. */
export interface PlanSetResult {
  outcome: PlanSetOutcome
}

/** Params of `_orch/plan/get`. */
export interface PlanGetParams {
  sessionId: string
}

/** The state of plan mode; result of `_orch/plan/get` and params of `_orch/plan/state`. */
export interface PlanState {
  sessionId: string
  active: boolean
  /** True when a change is chosen and waits for the next step of the running turn. */
  pending?: boolean
}

/** Params of `_orch/plan/refused`. */
export interface PlanRefused {
  sessionId: string
  callId: string
  tool: string
  /** The sentence the model was given. */
  reason: string
}

/** How a session treats changes: `ask` asks for approval before a change, `bypass` lets it run. */
export type ApprovalMode = 'ask' | 'bypass'

/** Params of `_orch/approval/set`. */
export interface ApprovalSetParams {
  sessionId: string
  mode: ApprovalMode
}

/** Result of `_orch/approval/set`. */
export interface ApprovalSetResult {
  mode: ApprovalMode
}

// ---------------------------------------------------------------------------------------------------------------------
// Usage and compaction

/** Token counts of one model call. Counts are disjoint: `inputTokens` is the fresh (uncached) input only. */
export interface Usage {
  inputTokens: number
  outputTokens: number
  cacheReadTokens?: number
  cacheWriteTokens?: number
  /** Part of `outputTokens`, not added to it. */
  reasoningTokens?: number
  totalTokens?: number
}

/** Params of `_orch/usage`. */
export interface UsageNotice {
  sessionId: string
  turn: number
  step: number
  /** The call's attempt id, the same as in `_orch/stream`. */
  attempt?: string
  provider: string
  model: string
  /** Absent when the provider reported none for this call. */
  usage?: Usage
  /** True when the turn was cancelled while the call streamed. */
  interrupted?: boolean
  /** True when the call produced no message (it failed or was retried). */
  failed?: boolean
  /** True for a call of a sub-agent: `sessionId` is then the session of the agent that started it. */
  subagent?: boolean
}

/** The phases of a compaction. */
export type CompactionPhase = 'started' | 'pruned' | 'summarized' | 'completed' | 'failed'

/** Params of `_orch/compaction`. */
export interface CompactionNotice {
  sessionId: string
  /** The turn it ran in; null for one run between turns. */
  turn: number | null
  phase: CompactionPhase
  /** Estimated tokens of history the phase replaced (phases `pruned` and `summarized`). */
  replacedTokens?: number
  /** The sentence of the failure (phase `failed`). */
  error?: string
}

/** Params of `_orch/compaction/run`. */
export interface CompactionRunParams {
  sessionId: string
}

/** Result of `_orch/compaction/run`. */
export interface CompactionRunResult {
  /** False when there was nothing useful to compact. */
  compacted: boolean
}

// ---------------------------------------------------------------------------------------------------------------------
// Skills, fork and history

/** Params of `_orch/skills/refresh`. */
export interface SkillsRefreshParams {
  sessionId?: string
}

/** Result of `_orch/skills/refresh`: the skills the session now sees. */
export interface SkillsRefreshResult {
  skills: Array<{ name: string; description?: string }>
}

/** Params of `_orch/session/fork`. */
export interface SessionForkParams {
  sessionId: string
  /** Fork after this turn (inclusive). Absent: after the last finished turn. */
  throughTurn?: number
}

/** Result of `_orch/session/fork`. */
export interface SessionForkResult {
  /** The new session; resume it with `session/resume`. */
  sessionId: string
  /** How many turns it holds. */
  turns: number
}

/** The format of Orch's neutral history: JSON lines of items, the main stream, oldest first. */
export const NEUTRAL_FORMAT = 'orch-neutral-jsonl-v1'

/** The format of dsh's own record of a session. */
export const NATIVE_FORMAT = 'dsh-session-v4'

/** Params of `_orch/history/import`. */
export interface HistoryImportParams {
  /** A file in `NEUTRAL_FORMAT` that dsh can read. */
  path: string
  format: typeof NEUTRAL_FORMAT
  /** The working directory the new session belongs to. */
  cwd: string
  /** Carry only the first turns. */
  throughTurn?: number
  /**
   * The route of the thread (a provider and model dsh knows): the carried assistant messages record it, and dsh
   * refuses to open a session that records a route it does not have. Optional only where a session is open in the
   * process, whose route is then taken.
   */
  provider?: string
  model?: string
}

/** How faithfully items arrived, in the words of Orch's history report. */
export interface HistoryReport {
  fidelity: 'lossless' | 'structured' | 'summary'
  itemsTotal: number
  byKind: Record<string, { kept: number; converted: number; dropped: number }>
  dropped: Array<{ kind: string; count: number; reason: string }>
  warnings: string[]
}

/** Result of `_orch/history/import`. */
export interface HistoryImportResult {
  sessionId: string
  report: HistoryReport
}

/** Params of `_orch/history/export`. */
export interface HistoryExportParams {
  sessionId: string
  format: typeof NATIVE_FORMAT
  /** Where to write the file; its directory exists. */
  path: string
}

/** Result of `_orch/history/export`. */
export interface HistoryExportResult {
  format: typeof NATIVE_FORMAT
  path: string
  size: number
}

// ---------------------------------------------------------------------------------------------------------------------
// What Orch tells the plugin of a session

/** The key of `_meta` that holds Orch's members. */
export const META_KEY = 'orch'

/**
 * The `_meta.orch` of `session/new`, `session/resume` and `session/prompt`: what Orch tells the plugin of a session.
 * A member that is absent changes nothing. The same things can be set with the plugin's own requests
 * (`_orch/approval/set`, `_orch/plan/set`); whichever came last wins.
 */
export interface SessionMeta {
  version?: number
  thread_id?: string
  segment_id?: string
  session_key?: string
  arrangement?: string
  /** `ask`, `on_request`, `auto` or `bypass`: `bypass` asks for nothing, any other mode asks before a change. */
  approval_mode?: string
  sandbox_mode?: string
  model?: string
  /** The directory of the session's skills (`<name>/SKILL.md` folders). */
  skills_dir?: string | null
  instructions?: string
  /** Where the program's tools reach the environment, when the arrangement is separated. */
  exec?: { url: string; token: string; tool_set?: string }
  // Of a prompt:
  turn_id?: string
  /** True when the turn is a planning turn. */
  plan?: boolean
  native_turn_ref?: string
}

// ---------------------------------------------------------------------------------------------------------------------
// The wire

/** A JSON-RPC id. The plugin's own requests use strings starting with `orch-`. */
export type RpcId = string | number

/** A JSON-RPC request or notification. */
export interface RpcRequest {
  jsonrpc: '2.0'
  id?: RpcId
  method: string
  params?: unknown
}

/** A JSON-RPC error object. */
export interface RpcError {
  code: number
  message: string
  data?: unknown
}

/** A JSON-RPC response. */
export interface RpcResponse {
  jsonrpc: '2.0'
  id: RpcId | null
  result?: unknown
  error?: RpcError
}

/** Any message on the wire. */
export type RpcMessage = RpcRequest | RpcResponse

/** Whether a message is a request or notification (it names a method). */
export function isRequest(message: RpcMessage): message is RpcRequest {
  return typeof (message as RpcRequest).method === 'string'
}

/** Whether a method belongs to this protocol. */
export function isOrchMethod(method: string): boolean {
  return method.startsWith(METHOD_PREFIX)
}
