/** Rows of a thread, built from the generated types: what a test sends through the fake socket and what the fake API starts a thread from.
 *
 * A builder gives a row as the server would send it, except for the three things the fake API itself
 * decides when a row is emitted: the thread and the segment it belongs to, and the sequence numbers.
 */
import type {
  Approval,
  CommandAction,
  ContentPart,
  FileChange,
  HarnessRequest,
  Item,
  ItemStatus,
  MarkerPayload,
  PlanStep,
  ThreadInput,
  ToolPayload,
  Turn,
  UserRef,
} from '@/generated/api-types';

/** The moment rows are made at unless a test says another. */
export const AT = '2026-10-01T10:00:00.000Z';

interface ItemOptions {
  turn?: string | null;
  status?: ItemStatus;
  author?: UserRef | null;
  at?: string;
  stream?: string;
}

function base(id: string, options: ItemOptions) {
  const at = options.at ?? AT;
  return {
    id,
    thread_id: '',
    segment_id: '',
    stream: options.stream ?? 'main',
    before_orch: false,
    turn_id: options.turn ?? null,
    parent_item_id: null,
    author: options.author ?? null,
    status: options.status ?? 'completed',
    pinned: null,
    chat_refs: 0,
    seq_created: 0,
    seq_updated: 0,
    created_at: at,
    updated_at: at,
  } as const;
}

export function userMessage(id: string, text: string, options: ItemOptions & { author: UserRef; inputId?: string; delivery?: 'queue' | 'steer' | 'interrupt'; parts?: ContentPart[]; source?: 'user' | 'system' | 'announcement' }): Item {
  return {
    ...base(id, options),
    kind: 'user_message',
    payload: { delivery: options.delivery ?? 'queue', input_id: options.inputId ?? `${id}-input`, parts: options.parts ?? [{ type: 'text', text }], source: options.source ?? 'user' },
  };
}

export function agentMessage(id: string, text: string, options: ItemOptions & { phase?: 'commentary' | 'final' } = {}): Item {
  return {
    ...base(id, options),
    kind: 'agent_message',
    payload: { phase: options.phase ?? (options.status === 'in_progress' ? 'commentary' : 'final'), text, text_len: new TextEncoder().encode(text).length },
  };
}

export function reasoning(id: string, summary: string, options: ItemOptions & { hidden?: boolean } = {}): Item {
  return { ...base(id, options), kind: 'reasoning', payload: { hidden: options.hidden ?? false, summary, text_len: summary.length } };
}

export function plan(id: string, steps: PlanStep[], options: ItemOptions & { explanation?: string } = {}): Item {
  return { ...base(id, options), kind: 'plan', payload: { explanation: options.explanation ?? null, steps } };
}

const out = (text: string, total = new TextEncoder().encode(text).length) => ({ blob: null, inline: text, total_len: total, truncated: false });

export function command(id: string, options: ItemOptions & { command: string; output?: string; exit?: number | null; background?: boolean; cwd?: string; actions?: CommandAction[]; durationMs?: number | null }): Item {
  const status = options.status ?? (options.exit === undefined || options.exit === null ? 'in_progress' : options.exit === 0 ? 'completed' : 'completed');
  return {
    ...base(id, { ...options, status }),
    kind: 'tool_call',
    payload: {
      duration_ms: options.durationMs ?? (status === 'in_progress' ? null : 14_000),
      raw: { input: out(JSON.stringify({ command: options.command })), is_error: (options.exit ?? 0) !== 0, name: 'Bash', output: options.output === undefined ? null : out(options.output) },
      started_at: options.at ?? AT,
      title: options.command,
      tool: {
        kind: 'command',
        actions: options.actions ?? [],
        background: options.background ?? false,
        command: options.command,
        cwd: options.cwd ?? '/work/web',
        exit_code: options.exit === undefined ? null : options.exit,
        output: out(options.output ?? ''),
        process_id: null,
      },
    },
  };
}

export function fileChange(id: string, options: ItemOptions & { changes: FileChange[]; durationMs?: number | null }): Item {
  const status = options.status ?? 'completed';
  return {
    ...base(id, { ...options, status }),
    kind: 'tool_call',
    payload: {
      duration_ms: options.durationMs ?? (status === 'in_progress' ? null : 900),
      raw: { input: out(JSON.stringify({ changes: options.changes.map((change) => change.path) })), is_error: false, name: 'apply_patch', output: status === 'in_progress' ? null : out('ok') },
      started_at: options.at ?? AT,
      title: options.changes.length === 1 ? `Edited ${options.changes[0]?.path ?? ''}` : `Edited ${options.changes.length} files`,
      tool: { kind: 'file_change', changes: options.changes },
    },
  };
}

export function change(path: string, diff: string | null, more: Partial<FileChange> = {}): FileChange {
  const lines = (diff ?? '').split('\n');
  return {
    added: lines.filter((line) => line.startsWith('+') && !line.startsWith('+++')).length,
    removed: lines.filter((line) => line.startsWith('-') && !line.startsWith('---')).length,
    binary: false,
    change: 'modify',
    diff,
    old_path: null,
    path,
    truncated: false,
    ...more,
  };
}

export function fileRead(id: string, options: ItemOptions & { path: string; content: string; from?: number; to?: number }): Item {
  return {
    ...base(id, options),
    kind: 'tool_call',
    payload: {
      duration_ms: 120,
      raw: { input: out(JSON.stringify({ path: options.path })), is_error: false, name: 'Read', output: out(options.content) },
      started_at: options.at ?? AT,
      title: `Read ${options.path}`,
      tool: { kind: 'file_read', content: out(options.content), end_line: options.to ?? null, media_type: null, path: options.path, start_line: options.from ?? null, total_lines: 220 },
    },
  };
}

/** Any step, by the payload of its tool. */
export function toolCall(id: string, tool: ToolPayload, options: ItemOptions & { title: string; durationMs?: number | null; name?: string; error?: boolean; output?: unknown }): Item {
  const status = options.status ?? 'completed';
  return {
    ...base(id, { ...options, status }),
    kind: 'tool_call',
    payload: {
      duration_ms: options.durationMs ?? (status === 'in_progress' ? null : 400),
      raw: { input: out(JSON.stringify({ tool: tool.kind })), is_error: options.error ?? false, name: options.name ?? tool.kind, output: status === 'in_progress' ? null : out(JSON.stringify(options.output ?? { ok: true })) },
      started_at: options.at ?? AT,
      title: options.title,
      tool,
    },
  };
}

/** A line the system or an announcement put into the conversation as a message. */
export function noticeMessage(id: string, text: string, source: 'system' | 'announcement', options: ItemOptions = {}): Item {
  return { ...base(id, options), kind: 'user_message', payload: { delivery: 'queue', input_id: `${id}-input`, parts: [{ type: 'text', text }], source } };
}

export function announcement(id: string, options: ItemOptions & { title: string; body: string; from: UserRef; severity?: 'info' | 'important'; category?: 'notice' | 'milestone'; toThread?: boolean }): Item {
  return {
    ...base(id, options),
    kind: 'announcement',
    payload: { announcement_id: `${id}-a`, body: options.body, category: options.category ?? 'notice', from: { user: options.from }, severity: options.severity ?? 'info', title: options.title, to_thread: options.toThread ?? false },
  };
}

export function otherTool(id: string, options: ItemOptions & { name: string; args?: unknown; result?: unknown }): Item {
  return {
    ...base(id, options),
    kind: 'tool_call',
    payload: {
      duration_ms: 50,
      raw: { input: out(JSON.stringify(options.args ?? {})), is_error: false, name: options.name, output: out(JSON.stringify(options.result ?? {})) },
      started_at: options.at ?? AT,
      title: options.name,
      tool: { kind: 'other', arguments: options.args ?? {}, name: options.name, result: options.result ?? {} },
    },
  };
}

export function approvalItem(id: string, approvalId: string, options: ItemOptions = {}): Item {
  return { ...base(id, { ...options, status: options.status ?? 'pending' }), kind: 'approval', payload: { approval_id: approvalId } };
}

export function marker(id: string, payload: MarkerPayload, options: ItemOptions = {}): Item {
  return { ...base(id, options), kind: 'marker', payload };
}

export function errorItem(id: string, message: string, options: ItemOptions & { retryable?: boolean } = {}): Item {
  return { ...base(id, { ...options, status: options.status ?? 'failed' }), kind: 'error', payload: { code: 'turn.failed', message, retryable: options.retryable ?? false } };
}

export function turn(id: string, options: { ordinal: number; state: Turn['state']; inputId: string; startedAt?: string | null; completedAt?: string | null; waitingSince?: string | null; waitedMs?: number; model?: string; display?: string; effort?: string | null; tokens?: number; phase?: Turn['phase']; interruptedBy?: UserRef | null; params?: Record<string, unknown>; detail?: Turn['usage_detail']; fill?: Turn['context'] }): Turn {
  return {
    completed_at: options.completedAt ?? null,
    effort: options.effort === undefined ? 'high' : options.effort,
    error: options.state === 'failed' ? { code: 'turn.failed', message: 'The model refused the request.', retryable: true, said: null, resets_at: null, window: null, incomplete: false, limit: null } : null,
    id,
    input_id: options.inputId,
    interrupted_by: options.interruptedBy ?? null,
    command_cleanup: null,
    // What the harness reported beyond the totals, and what filled the context at the turn's end.
    usage_detail: options.detail ?? null,
    context: options.fill ?? null,
    before_snapshot_id: null,
    limit_bypass: [],
    model: options.model ?? 'gpt-test',
    model_display: options.display ?? options.model ?? 'GPT Test',
    ordinal: options.ordinal,
    params: options.params ?? {},
    phase: options.phase ?? null,
    plan: false,
    segment_id: '',
    seq_updated: 0,
    started_at: options.startedAt ?? AT,
    waiting_since: options.waitingSince ?? null,
    waited_ms: options.waitedMs ?? 0,
    state: options.state,
    thread_id: '',
    usage:
      options.tokens === undefined
        ? null
        : { cached_input_tokens: Math.floor(options.tokens / 2), input_tokens: Math.floor(options.tokens * 0.8), output_tokens: Math.floor(options.tokens * 0.2), reasoning_tokens: 0, total_tokens: options.tokens },
  };
}

export function threadInput(id: string, text: string, options: { author: UserRef | null; position: string; state?: ThreadInput['state']; delivery?: ThreadInput['delivery']; source?: ThreadInput['source']; detail?: string | null; clientId?: string | null }): ThreadInput {
  return {
    native_action: null,
    author: options.author,
    client_id: options.clientId ?? null,
    content: [{ type: 'text', text }],
    created_at: AT,
    delivered_at: null,
    delivery: options.delivery ?? 'queue',
    id,
    attachment_issues: [],
    queue_held: false,
    limit_bypass: [],
    plan: false,
    position: options.position,
    run_as: null,
    seq_updated: 0,
    source: options.source ?? 'user',
    state: options.state ?? 'pending',
    state_detail: options.detail ?? null,
    thread_id: '',
    turn_id: null,
  };
}

/** A request of the harness for approval, in the shape the API gives it. */
export function harnessApproval(
  id: string,
  options: { threadId: string; itemId: string; title: string; request: HarnessRequest; canDecide?: boolean; requester: UserRef; node?: Approval['node'] },
): Approval {
  return {
    approver_scope: null,
    batch_id: null,
    can_decide: options.canDecide ?? true,
    created_at: AT,
    decided_at: null,
    decided_automatically: false,
    decided_by: null,
    decision: null,
    expires_at: null,
    id,
    item_id: options.itemId,
    node: options.node ?? null,
    object: null,
    requester: { user: options.requester },
    required_permission: 'thread.manage',
    state: 'pending',
    thread_id: options.threadId,
    title: options.title,
    kind: 'harness',
    request: options.request,
  };
}

export const ALLOW_DENY: HarnessRequest['options'] = [
  { id: 'allow_once', label: 'Allow once', scope: 'once', style: 'allow', rule: null },
  { id: 'allow_session', label: 'Allow for session', scope: 'session', style: 'allow', rule: null },
  { id: 'deny', label: 'Deny', scope: null, style: 'deny', rule: null },
];

export const LOGIN_DIFF = [
  '@@ -39,7 +39,7 @@ export function login(request: Request) {',
  '   const session = readSession(request);',
  '   if (session === null) return unauthorized();',
  '   const next = request.query.next;',
  '-  return redirect(next);',
  '+  return redirect(next ?? "/");',
  ' }',
  ' ',
  ' export function logout(request: Request) {',
].join('\n');
